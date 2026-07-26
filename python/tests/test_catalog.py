from pathlib import Path

import pytest
from rdflib import Graph, URIRef
from rdflib.namespace import OWL, RDF

from ontoenv import (
    CatalogRecoveryError,
    ExternalStoreChangedError,
    OntoEnv,
    StoreCapabilityError,
    UnresolvedImportError,
)


class RevisionStore:
    def __init__(self) -> None:
        self.graphs: dict[str, Graph] = {}
        self.revisions: dict[str, int] = {}
        self.revision = 0
        self.get_calls = 0

    def add_graph(self, iri: str, graph: Graph, overwrite: bool = False) -> None:
        if overwrite or iri not in self.graphs:
            self.graphs[iri] = graph
            self.revision += 1
            self.revisions[iri] = self.revision

    def get_graph(self, iri: str) -> Graph:
        self.get_calls += 1
        return self.graphs[iri]

    def remove_graph(self, iri: str) -> None:
        self.graphs.pop(iri, None)
        self.revisions.pop(iri, None)
        self.revision += 1

    def graph_ids(self) -> list[str]:
        return list(self.graphs)

    def store_state(self) -> dict[str, str]:
        return {"id": "revision-store", "revision": str(self.revision)}

    def graph_revisions(self) -> dict[str, str]:
        return {iri: str(revision) for iri, revision in self.revisions.items()}


class StateOnlyStore:
    """Versioned globally, but unable to identify which graph changed."""

    def __init__(self) -> None:
        self.graphs: dict[str, Graph] = {}
        self.revision = 0

    def add_graph(self, iri: str, graph: Graph, overwrite: bool = False) -> None:
        if overwrite or iri not in self.graphs:
            self.graphs[iri] = graph
            self.revision += 1

    def get_graph(self, iri: str) -> Graph:
        return self.graphs[iri]

    def remove_graph(self, iri: str) -> None:
        self.graphs.pop(iri, None)
        self.revision += 1

    def graph_ids(self) -> list[str]:
        return list(self.graphs)

    def store_state(self) -> dict[str, str]:
        return {"id": "state-only-store", "revision": str(self.revision)}


class MutatingReadStore(RevisionStore):
    """Store that changes its revision during the first catalog scan."""

    def get_graph(self, iri: str) -> Graph:
        graph = super().get_graph(iri)
        if self.get_calls == 1:
            self.revision += 1
            self.revisions[iri] = self.revision
        return graph


class FailingReadStore(RevisionStore):
    """Store that exposes an ID but cannot read its graph."""

    def get_graph(self, iri: str) -> Graph:
        raise RuntimeError(f"simulated read failure for {iri}")


class ToggleFailStore(RevisionStore):
    """Store whose reads can fail after a successful initial adoption."""

    fail_reads = False

    def get_graph(self, iri: str) -> Graph:
        if self.fail_reads:
            raise RuntimeError(f"simulated read failure for {iri}")
        return super().get_graph(iri)


class MisleadingErrorStore(RevisionStore):
    """Store error text that resembles a typed OntoEnv error."""

    def graph_ids(self) -> list[str]:
        raise RuntimeError("ExternalStoreChangedError is merely backend text")


def ontology_graph(iri: str) -> Graph:
    graph = Graph()
    graph.add((URIRef(iri), RDF.type, OWL.Ontology))
    return graph


def test_warm_open_uses_catalog_without_get_graph(tmp_path: Path) -> None:
    store = RevisionStore()
    iri = "https://example.org/catalog-warm"
    store.add_graph(iri, ontology_graph(iri))

    adopted = OntoEnv.adopt(tmp_path, store)
    assert iri in adopted.get_ontology_names()
    adopted.close()
    calls_after_adoption = store.get_calls

    reopened = OntoEnv.open(tmp_path, graph_store=store)
    assert iri in reopened.get_ontology_names()
    assert store.get_calls == calls_after_adoption
    report = reopened.refresh_from_store()
    assert report.mode == "incremental"
    assert report.unchanged == [iri]
    reopened.close()


def test_external_revision_drift_is_detected_on_open(tmp_path: Path) -> None:
    store = RevisionStore()
    first = "https://example.org/first"
    store.add_graph(first, ontology_graph(first))
    adopted = OntoEnv.adopt(tmp_path, store)
    adopted.close()

    second = "https://example.org/second"
    store.add_graph(second, ontology_graph(second))
    with pytest.raises(ExternalStoreChangedError):
        OntoEnv.open(tmp_path, graph_store=store)


def test_targeted_refresh_leaves_unrelated_changes_pending(tmp_path: Path) -> None:
    store = RevisionStore()
    first = "https://example.org/first"
    second = "https://example.org/second"
    store.add_graph(first, ontology_graph(first))
    store.add_graph(second, ontology_graph(second))

    environment = OntoEnv.adopt(tmp_path, store)
    store.add_graph(first, ontology_graph(first), overwrite=True)
    store.add_graph(second, ontology_graph(second), overwrite=True)

    report = environment.refresh_from_store(graphs=[first])
    assert report.mode == "targeted"
    assert report.changed == [first]
    assert report.still_pending == [second]
    environment.close()

    # The global revision deliberately remains unsynchronized until all
    # changed graphs have been reconciled.
    with pytest.raises(ExternalStoreChangedError):
        OntoEnv.open(tmp_path, graph_store=store)


def test_full_refresh_resynchronizes_global_revision(tmp_path: Path) -> None:
    store = RevisionStore()
    first = "https://example.org/first"
    second = "https://example.org/second"
    store.add_graph(first, ontology_graph(first))
    environment = OntoEnv.adopt(tmp_path, store)

    store.add_graph(second, ontology_graph(second))
    report = environment.refresh_from_store(full=True)
    assert report.mode == "full"
    assert report.added == [second]
    assert report.still_pending == []
    environment.close()

    reopened = OntoEnv.open(tmp_path, graph_store=store)
    assert set(reopened.get_ontology_names()) == {first, second}
    reopened.close()


def test_incremental_refresh_removes_deleted_graph(tmp_path: Path) -> None:
    store = RevisionStore()
    first = "https://example.org/first"
    second = "https://example.org/second"
    store.add_graph(first, ontology_graph(first))
    store.add_graph(second, ontology_graph(second))
    environment = OntoEnv.adopt(tmp_path, store)

    store.remove_graph(second)
    report = environment.refresh_from_store()
    assert report.mode == "incremental"
    assert report.removed == [second]
    assert report.still_pending == []
    environment.close()

    reopened = OntoEnv.open(tmp_path, graph_store=store)
    assert reopened.get_ontology_names() == [first]
    reopened.close()


def test_adopt_does_not_fetch_missing_imports(tmp_path: Path) -> None:
    store = RevisionStore()
    iri = "https://example.org/root"
    graph = ontology_graph(iri)
    graph.add(
        (
            URIRef(iri),
            OWL.imports,
            URIRef("https://example.invalid/missing-import"),
        )
    )
    store.add_graph(iri, graph)

    environment = OntoEnv.adopt(tmp_path, store)
    assert environment.get_ontology_names() == [iri]
    assert store.get_calls == 1
    environment.close()


def test_copy_graph_distinguishes_unresolved_import(tmp_path: Path) -> None:
    store = RevisionStore()
    iri = "https://example.org/root"
    missing = "https://example.invalid/missing-import"
    graph = ontology_graph(iri)
    graph.add((URIRef(iri), OWL.imports, URIRef(missing)))
    store.add_graph(iri, graph)

    environment = OntoEnv.adopt(tmp_path, store)
    with pytest.raises(UnresolvedImportError, match=missing):
        environment.copy_graph(missing)
    with pytest.raises(ValueError, match="Failed to resolve graph"):
        environment.copy_graph("https://example.invalid/not-an-import")
    environment.close()


def test_recover_rebuilds_catalog_without_manual_file_deletion(tmp_path: Path) -> None:
    store = RevisionStore()
    iri = "https://example.org/root"
    store.add_graph(iri, ontology_graph(iri))
    environment = OntoEnv.adopt(tmp_path, store)
    environment.close()

    pending = tmp_path / ".ontoenv" / "catalog.pending"
    pending.write_text('{"mutation_id":"test","graphs":[]}')
    with pytest.raises(CatalogRecoveryError):
        OntoEnv.open(tmp_path, graph_store=store)

    recovered = OntoEnv.recover(tmp_path, graph_store=store)
    assert recovered.get_ontology_names() == [iri]
    assert not pending.exists()
    assert CatalogRecoveryError.__doc__
    recovered.close()


def test_recover_failure_retains_marker_and_old_catalog(tmp_path: Path) -> None:
    store = FailingReadStore()
    iri = "https://example.org/root"
    # Populate without calling the overridden reader.
    RevisionStore.add_graph(store, iri, ontology_graph(iri))
    good_store = RevisionStore()
    good_store.graphs = store.graphs
    good_store.revisions = store.revisions
    good_store.revision = store.revision
    environment = OntoEnv.adopt(tmp_path, good_store)
    environment.close()
    original_catalog = (tmp_path / ".ontoenv" / "catalog.r5tu").read_bytes()

    pending = tmp_path / ".ontoenv" / "catalog.pending"
    pending.write_text('{"mutation_id":"test","graphs":[]}')
    with pytest.raises(ValueError, match="simulated read failure"):
        OntoEnv.recover(tmp_path, graph_store=store)

    assert pending.exists(), "failed recovery must remain retryable"
    assert (tmp_path / ".ontoenv" / "catalog.r5tu").read_bytes() == original_catalog


def test_failed_full_refresh_preserves_live_and_saved_catalog(tmp_path: Path) -> None:
    store = ToggleFailStore()
    iri = "https://example.org/root"
    store.add_graph(iri, ontology_graph(iri))
    environment = OntoEnv.adopt(tmp_path, store)
    original_catalog = (tmp_path / ".ontoenv" / "catalog.r5tu").read_bytes()

    store.fail_reads = True
    with pytest.raises(ValueError, match="simulated read failure"):
        environment.refresh_from_store(full=True)

    assert environment.get_ontology_names() == [iri]
    assert (tmp_path / ".ontoenv" / "catalog.r5tu").read_bytes() == original_catalog
    environment.close()


def test_adopt_rejects_backend_mutation_during_scan(tmp_path: Path) -> None:
    store = MutatingReadStore()
    iri = "https://example.org/root"
    store.add_graph(iri, ontology_graph(iri))

    with pytest.raises(ExternalStoreChangedError, match="changed while rebuilding"):
        OntoEnv.adopt(tmp_path, store)
    assert not (tmp_path / ".ontoenv" / "catalog.r5tu").exists()


def test_error_types_are_not_selected_from_message_text(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="merely backend text"):
        OntoEnv.adopt(tmp_path, MisleadingErrorStore())


def test_brick_style_non_strict_imports_commit_cleanly(tmp_path: Path) -> None:
    """Exercise the consumer path with eight intentionally unavailable imports."""
    store = RevisionStore()
    root = "https://brickschema.org/schema/Brick"
    missing = [f"https://example.invalid/brick-import-{index}" for index in range(8)]
    graph = ontology_graph(root)
    for imported in missing:
        graph.add((URIRef(root), OWL.imports, URIRef(imported)))

    environment = OntoEnv.create(
        tmp_path,
        graph_store=store,
        strict=False,
        offline=True,
    )
    assert environment.add(graph) == root
    assert set(environment.missing_imports(root)) == set(missing)
    assert not (tmp_path / ".ontoenv" / "catalog.pending").exists()
    environment.close()

    reopened = OntoEnv.connect(tmp_path, graph_store=store)
    assert reopened.get_ontology_names() == [root]
    assert set(reopened.missing_imports(root)) == set(missing)
    reopened.close()


def test_import_dependencies_tracks_all_transient_unresolved_imports(
    tmp_path: Path,
) -> None:
    """BuildingMOTIF's import path commits cleanly and gives every miss one type."""
    store = RevisionStore()
    root = "https://brickschema.org/schema/Brick"
    missing = [f"https://example.invalid/brick-import-{index}" for index in range(8)]
    graph = ontology_graph(root)
    for imported in missing:
        graph.add((URIRef(root), OWL.imports, URIRef(imported)))

    environment = OntoEnv.create(
        tmp_path,
        graph_store=store,
        strict=False,
        offline=True,
    )

    assert environment.import_dependencies(graph, fetch_missing=True) == []
    assert not (tmp_path / ".ontoenv" / "catalog.pending").exists()
    assert set(environment.missing_imports()) == set(missing)
    for imported in missing:
        with pytest.raises(UnresolvedImportError, match=imported):
            environment.copy_graph(imported)

    environment.close()


def test_connect_creates_empty_custom_environment(tmp_path: Path) -> None:
    store = RevisionStore()

    environment = OntoEnv.connect(tmp_path, graph_store=store)

    assert environment.get_ontology_names() == []
    assert (tmp_path / ".ontoenv" / "catalog.r5tu").exists()
    assert store.get_calls == 0
    environment.close()


def test_connect_adopts_populated_store_then_warm_opens(tmp_path: Path) -> None:
    store = RevisionStore()
    iri = "https://example.org/connect-adopt"
    store.add_graph(iri, ontology_graph(iri))

    environment = OntoEnv.connect(tmp_path, graph_store=store)
    assert environment.get_ontology_names() == [iri]
    assert store.get_calls == 1
    environment.close()

    reopened = OntoEnv.connect(tmp_path, graph_store=store)
    assert reopened.get_ontology_names() == [iri]
    assert store.get_calls == 1
    reopened.close()


def test_connect_auto_incrementally_reconciles_closed_store(tmp_path: Path) -> None:
    store = RevisionStore()
    first = "https://example.org/first"
    second = "https://example.org/second"
    store.add_graph(first, ontology_graph(first))
    environment = OntoEnv.connect(tmp_path, graph_store=store)
    environment.close()
    calls_after_adoption = store.get_calls

    store.add_graph(second, ontology_graph(second))
    reconciled = OntoEnv.connect(tmp_path, graph_store=store)

    assert set(reconciled.get_ontology_names()) == {first, second}
    assert store.get_calls == calls_after_adoption + 1
    reconciled.close()

    # The automatic reconciliation published the current global revision.
    reopened = OntoEnv.open(tmp_path, graph_store=store)
    assert set(reopened.get_ontology_names()) == {first, second}
    reopened.close()


def test_connect_catalog_trusts_stale_metadata_without_graph_reads(tmp_path: Path) -> None:
    store = RevisionStore()
    first = "https://example.org/first"
    second = "https://example.org/second"
    store.add_graph(first, ontology_graph(first))
    environment = OntoEnv.connect(tmp_path, graph_store=store)
    environment.close()
    calls_after_adoption = store.get_calls

    store.add_graph(second, ontology_graph(second))
    catalog_only = OntoEnv.connect(tmp_path, graph_store=store, sync="catalog")

    assert catalog_only.get_ontology_names() == [first]
    assert store.get_calls == calls_after_adoption
    catalog_only.close()

    # Catalog-only mode does not falsely mark the external revision as synced.
    with pytest.raises(ExternalStoreChangedError):
        OntoEnv.open(tmp_path, graph_store=store)


def test_connect_full_rebuilds_unversioned_store(tmp_path: Path) -> None:
    store = StateOnlyStore()
    first = "https://example.org/first"
    second = "https://example.org/second"
    store.add_graph(first, ontology_graph(first))
    environment = OntoEnv.connect(tmp_path, graph_store=store)
    environment.close()

    store.add_graph(second, ontology_graph(second))
    with pytest.raises(StoreCapabilityError, match='sync="full"'):
        OntoEnv.connect(tmp_path, graph_store=store)

    rebuilt = OntoEnv.connect(tmp_path, graph_store=store, sync="full")
    assert set(rebuilt.get_ontology_names()) == {first, second}
    rebuilt.close()


def test_unversioned_targeted_refresh_does_not_claim_global_sync(tmp_path: Path) -> None:
    store = StateOnlyStore()
    first = "https://example.org/first"
    second = "https://example.org/second"
    store.add_graph(first, ontology_graph(first))
    environment = OntoEnv.connect(tmp_path, graph_store=store)

    store.add_graph(second, ontology_graph(second))
    report = environment.refresh_from_store(graphs=[second])
    assert report.added == [second]
    environment.close()

    # Without per-graph revisions OntoEnv cannot prove that the targeted graph
    # was the only external change, so the catalog remains globally stale.
    with pytest.raises(ExternalStoreChangedError):
        OntoEnv.open(tmp_path, graph_store=store)


def test_connect_builtin_create_and_read_only_warm_open(tmp_path: Path) -> None:
    ontology = tmp_path / "site.ttl"
    iri = "https://example.org/site"
    ontology.write_text(
        "\n".join(
            [
                "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
                f"<{iri}> a owl:Ontology .",
            ]
        )
    )

    environment = OntoEnv.connect(tmp_path)
    environment.add(str(ontology))
    environment.close()

    reopened = OntoEnv.connect(tmp_path, read_only=True)
    assert reopened.get_ontology_names() == [iri]
    reopened.close()


def test_connect_reuses_persisted_configuration(tmp_path: Path) -> None:
    environment = OntoEnv.connect(tmp_path, offline=True, strict=True)
    environment.close()

    reopened = OntoEnv.connect(tmp_path)
    assert reopened.is_offline() is True
    assert reopened.is_strict() is True
    reopened.close()

    custom_root = tmp_path / "custom"
    store = RevisionStore()
    custom = OntoEnv.connect(
        custom_root,
        graph_store=store,
        offline=True,
        strict=True,
    )
    custom.close()

    custom_reopened = OntoEnv.connect(custom_root, graph_store=store)
    assert custom_reopened.is_offline() is True
    assert custom_reopened.is_strict() is True
    custom_reopened.close()


def test_connect_rejects_invalid_policy_and_temporary_mode(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="invalid sync policy"):
        OntoEnv.connect(tmp_path, sync="sometimes")
    with pytest.raises(ValueError, match="persistent environments"):
        OntoEnv.connect(tmp_path, temporary=True)
