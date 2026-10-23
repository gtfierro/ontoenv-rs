"""The normal Python API must pin catalog, namespaces and RDF to one version."""
from datetime import datetime
from pathlib import Path

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
