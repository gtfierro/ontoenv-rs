from __future__ import annotations

from pathlib import Path

import pytest
from rdflib import Dataset, Graph, Literal, URIRef

from ontoenv import (
    OntoEnv,
    OntoEnvStore,
)


FIXTURES = Path(__file__).parent / "fixtures" / "rdflib_store"
DEMO_TTL = FIXTURES / "demo.ttl"
FIRST_TTL = FIXTURES / "first.ttl"
SECOND_TTL = FIXTURES / "second.ttl"


class DictGraphStore:
    def __init__(self) -> None:
        self.graphs: dict[str, Graph] = {}

    def add_graph(self, iri: str, graph: Graph, overwrite: bool = False) -> None:
        if not overwrite and iri in self.graphs:
            return
        self.graphs[iri] = graph

    def get_graph(self, iri: str) -> Graph:
        return self.graphs[iri]

    def graph_ids(self) -> list[str]:
        return list(self.graphs.keys())


@pytest.fixture
def persistent_env(tmp_path: Path):
    env = OntoEnv(path=tmp_path, recreate=True, offline=True)
    try:
        yield env
    finally:
        env.close()


@pytest.fixture
def temporary_env():
    env = OntoEnv(temporary=True, offline=True)
    try:
        yield env
    finally:
        env.close()


def test_standalone_store_is_empty_read_only() -> None:
    store = OntoEnvStore()
    graph = Graph(store=store, identifier=URIRef("urn:g"))
    s = URIRef("urn:s")
    p = URIRef("urn:p")
    o = Literal("value")
    with pytest.raises(ValueError, match="read-only snapshot"):
        graph.add((s, p, o))
    assert len(list(graph.query("SELECT ?o WHERE { <urn:s> <urn:p> ?o }"))) == 0
    assert len(list(store.contexts())) == 0


def test_dataset_from_env_auto_uses_rdf5d_for_persistent_env(persistent_env: OntoEnv) -> None:
    persistent_env.add(str(DEMO_TTL))
    persistent_env.flush()

    dataset = persistent_env.as_dataset()
    assert dataset.store._backend.backend_kind() == "rdf5d"

    rows = list(
        dataset.query(
            "SELECT ?label WHERE { GRAPH <urn:example:demo> { <urn:example:ahu1> <urn:example:hasLabel> ?label } }"
        )
    )
    assert [row.label for row in rows] == [Literal("AHU-1")]
    assert len(dataset.graph(URIRef("urn:example:demo"))) == 2

    with pytest.raises(ValueError, match="read-only snapshot"):
        dataset.graph(URIRef("urn:example:demo")).add(
            (URIRef("urn:example:ahu2"), URIRef("urn:example:hasLabel"), Literal("AHU-2"))
        )


def test_dataset_from_env_auto_falls_back_to_copy_for_temporary_env(temporary_env: OntoEnv) -> None:
    temporary_env.add(str(DEMO_TTL))

    dataset = temporary_env.as_dataset()
    assert dataset.store._backend.backend_kind() == "copy"
    rows = list(
        dataset.query(
            "SELECT ?label WHERE { GRAPH <urn:example:demo> { <urn:example:ahu1> <urn:example:hasLabel> ?label } }"
        )
    )
    assert [row.label for row in rows] == [Literal("AHU-1")]


def test_backend_rdf5d_rejects_temporary_and_graph_store_envs(temporary_env: OntoEnv) -> None:
    temporary_env.add(str(DEMO_TTL))
    with pytest.raises(ValueError, match="backend='rdf5d'"):
        temporary_env.as_dataset(backend="rdf5d")

    store = DictGraphStore()
    external_env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)
    try:
        with pytest.raises(ValueError, match="backend='rdf5d'"):
            external_env.as_dataset(backend="rdf5d")
    finally:
        external_env.close()


def test_refresh_dataset_is_explicit(persistent_env: OntoEnv) -> None:
    persistent_env.add(str(FIRST_TTL))
    persistent_env.flush()

    dataset = persistent_env.as_dataset()
    assert dataset.store._backend.backend_kind() == "rdf5d"
    assert list(
        dataset.query(
            "SELECT ?label WHERE { GRAPH <urn:example:second> { <urn:example:second> <urn:example:hasLabel> ?label } }"
        )
    ) == []

    persistent_env.add(str(SECOND_TTL))
    persistent_env.flush()

    assert list(
        dataset.query(
            "SELECT ?label WHERE { GRAPH <urn:example:second> { <urn:example:second> <urn:example:hasLabel> ?label } }"
        )
    ) == []

    persistent_env.refresh_dataset(dataset)
    rows = list(
        dataset.query(
            "SELECT ?label WHERE { GRAPH <urn:example:second> { <urn:example:second> <urn:example:hasLabel> ?label } }"
        )
    )
    assert [row.label for row in rows] == [Literal("Second")]


def test_dataset_from_env_with_other_store_forces_copy(persistent_env: OntoEnv) -> None:
    persistent_env.add(str(DEMO_TTL))
    persistent_env.flush()

    with pytest.raises(ValueError, match="requires an OntoEnvStore"):
        persistent_env.as_dataset(backend="rdf5d", store=Graph().store)

    dataset = persistent_env.as_dataset(store=Graph().store)
    rows = list(
        dataset.query(
            "SELECT ?label WHERE { GRAPH <urn:example:demo> { <urn:example:ahu1> <urn:example:hasLabel> ?label } }"
        )
    )
    assert [row.label for row in rows] == [Literal("AHU-1")]
