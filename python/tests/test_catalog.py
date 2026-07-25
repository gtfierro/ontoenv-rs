from pathlib import Path

import pytest
from rdflib import Graph, URIRef
from rdflib.namespace import OWL, RDF

from ontoenv import ExternalStoreChangedError, OntoEnv, StoreCapabilityError


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
