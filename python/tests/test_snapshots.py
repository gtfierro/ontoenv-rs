"""The normal Python API must pin catalog, namespaces and RDF to one version."""
from datetime import datetime
import gc
import json
from pathlib import Path

import pytest

from rdflib import Graph, Literal, URIRef
from rdflib.namespace import OWL, RDF

from ontoenv import EnvironmentSnapshot, OntoEnv

NAME = URIRef("urn:snapshot:test")
PREDICATE = URIRef("urn:value")


def graph(value: str) -> Graph:
    result = Graph(identifier=NAME)
    result.add((NAME, RDF.type, OWL.Ontology))
    result.add((NAME, PREDICATE, Literal(value)))
    return result


def test_reader_refresh_and_views_pin_catalog_config_and_data(tmp_path: Path):
    writer = OntoEnv.connect(tmp_path, offline=True)
    writer.add(graph("old"), fetch_imports=False)
    writer.bind_namespace("chosen", "urn:old:")
    reader = OntoEnv.open(tmp_path, read_only=True)
    old = reader.snapshot()
    assert isinstance(old, EnvironmentSnapshot)
    before = old.info
    datetime.fromisoformat(before["committed_at"])
    datetime.fromisoformat(before["opened_at"])
    dataset = reader.get_dataset()
    view = reader.get_graph(str(NAME))
    closure, _ = reader.get_closure(str(NAME))
    writer.add(graph("new"), overwrite=True, fetch_imports=False)
    writer.add_alias("urn:alias", str(NAME))
    writer.bind_namespace("chosen", "urn:new:")
    assert reader.snapshot().info == before
    assert reader.resolve_alias("urn:alias") is None
    assert (NAME, PREDICATE, Literal("old")) in reader.get_graph(str(NAME))
    assert reader.refresh() is True
    assert reader.refresh() is False
    assert reader.snapshot().info["generation"] > before["generation"]
    assert reader.resolve_alias("urn:alias") == str(NAME)
    assert (NAME, PREDICATE, Literal("new")) in reader.get_graph("urn:alias")
    assert reader.get_bound_namespaces()["chosen"] == "urn:new:"
    assert old.info == before
    assert old.get_bound_namespaces()["chosen"] == "urn:old:"
    reader.close()
    writer.close()
    for pinned in (view, closure, old.get_graph(str(NAME)), dataset.graph(NAME)):
        assert (NAME, PREDICATE, Literal("old")) in pinned
        assert (NAME, PREDICATE, Literal("new")) not in pinned
    assert old.get_ontology_names() == [str(NAME)]
    assert [ontology.id for ontology in old.get_ontologies()] == [str(NAME)]


def test_explicit_dataset_rebind_keeps_inflight_iterator_and_its_term_cache(tmp_path: Path):
    writer = OntoEnv.connect(tmp_path, offline=True)
    old_graph = graph("old")
    for index in range(50):
        old_graph.add((URIRef(f"urn:old:{index}"), PREDICATE, Literal(index)))
    writer.add(old_graph, fetch_imports=False)
    dataset = writer.get_dataset()
    iterator = dataset.graph(NAME).triples((None, None, None))
    first = next(iterator)
    expected = set(old_graph)
    writer.add(graph("new"), overwrite=True, fetch_imports=False)
    writer.refresh_dataset(dataset)
    assert {first, *iterator} == expected
    assert (NAME, PREDICATE, Literal("new")) in dataset.graph(NAME)
    assert (NAME, PREDICATE, Literal("old")) not in dataset.graph(NAME)
    writer.close()


def test_add_with_rename_has_one_publication_and_invalid_rename_writes_nothing(tmp_path: Path):
    env = OntoEnv.connect(tmp_path, offline=True)
    before = env.snapshot().info["generation"]
    env.add(graph("value"), rename="urn:renamed", fetch_imports=False)
    assert env.snapshot().info["generation"] == before + 1
    assert env.get_ontology_names() == ["urn:renamed"]
    import pytest
    with pytest.raises(ValueError):
        env.add(graph("discard"), rename="not an iri", fetch_imports=False)
    assert env.snapshot().info["generation"] == before + 1
    assert env.get_ontology_names() == ["urn:renamed"]
    env.close()


def test_snapshot_context_releases_only_its_handle_and_view_owns_old_files(tmp_path: Path):
    with OntoEnv.connect(tmp_path, offline=True) as writer:
        writer.add(graph("old"), fetch_imports=False)
    with OntoEnv.read_snapshot(tmp_path) as snapshot:
        directory = tmp_path / ".ontoenv-snapshots"
        head = json.loads((directory / "CURRENT").read_text())
        manifest = json.loads((directory / "manifests" / head["manifest"]).read_text())
        old_path = directory / "objects" / manifest["object_id"]
        view = snapshot.get_graph(str(NAME))
        iterator = view.triples((None, None, None))
        first = next(iterator)
    assert snapshot.closed
    snapshot.close()  # idempotent
    for operation in (lambda: snapshot.info, snapshot.get_dataset, snapshot.get_ontology_names):
        with pytest.raises(ValueError, match="closed"):
            operation()
    with pytest.raises(ValueError, match="closed"):
        with snapshot:
            pass
    with OntoEnv.open(tmp_path) as writer:
        writer.add(graph("new"), overwrite=True, fetch_imports=False)
        writer.add(graph("newest"), overwrite=True, fetch_imports=False)
        assert (NAME, PREDICATE, Literal("old")) in view
        assert old_path.exists()
        assert writer.prune()["pinned_objects"] == 1
        del view
        gc.collect()
        assert writer.prune()["pinned_objects"] == 1
        assert {first, *iterator} == set(graph("old"))
        del iterator
        gc.collect()
        writer.prune()
        assert not old_path.exists()


def test_snapshot_context_closes_on_exception(tmp_path: Path):
    OntoEnv.connect(tmp_path, offline=True).close()
    snapshot = OntoEnv.read_snapshot(tmp_path)
    with pytest.raises(RuntimeError, match="expected"):
        with snapshot:
            raise RuntimeError("expected")
    assert snapshot.closed


def test_cleanup_requires_writer(tmp_path: Path):
    OntoEnv.connect(tmp_path, offline=True).close()
    with OntoEnv.open(tmp_path, read_only=True) as reader:
        with pytest.raises(ValueError, match="writer"):
            reader.prune()
