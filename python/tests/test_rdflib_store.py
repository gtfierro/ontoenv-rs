from __future__ import annotations

from pathlib import Path

import pytest
from rdflib import Dataset, Graph, Literal, URIRef

from ontoenv import OntoEnv, OntoEnvStore, refresh_dataset_from_env


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


def _write_ttl(path: Path, ontology_iri: str, triples: str = "") -> None:
    path.write_text(
        "\n".join(
            [
                "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
                "@prefix ex: <urn:example:> .",
                f"<{ontology_iri}> a owl:Ontology .",
                triples,
            ]
        ),
        encoding="utf-8",
    )


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


def test_dataset_from_env_auto_uses_rdf5d_for_persistent_env(tmp_path: Path) -> None:
    ttl = tmp_path / "demo.ttl"
    ontology_name = "urn:example:demo"
    _write_ttl(ttl, ontology_name, 'ex:ahu1 ex:hasLabel "AHU-1" .')

    env = OntoEnv(path=tmp_path, recreate=True, offline=True)
    try:
        env.add(str(ttl))
        env.flush()

        dataset = env.snapshot_as_dataset()
        assert dataset.store._backend.backend_kind() == "rdf5d"

        rows = list(
            dataset.query(
                "SELECT ?label WHERE { GRAPH <urn:example:demo> { <urn:example:ahu1> <urn:example:hasLabel> ?label } }"
            )
        )
        assert [row.label for row in rows] == [Literal("AHU-1")]
        assert len(dataset.graph(URIRef(ontology_name))) == 2

        with pytest.raises(ValueError, match="read-only snapshot"):
            dataset.graph(URIRef(ontology_name)).add(
                (URIRef("urn:example:ahu2"), URIRef("urn:example:hasLabel"), Literal("AHU-2"))
            )
    finally:
        env.close()


def test_dataset_from_env_auto_falls_back_to_copy_for_temporary_env(tmp_path: Path) -> None:
    ttl = tmp_path / "demo.ttl"
    _write_ttl(ttl, "urn:example:demo", 'ex:ahu1 ex:hasLabel "AHU-1" .')

    env = OntoEnv(temporary=True, offline=True)
    try:
        env.add(str(ttl))
        dataset = env.snapshot_as_dataset()
        assert dataset.store._backend.backend_kind() == "copy"
        rows = list(
            dataset.query(
                "SELECT ?label WHERE { GRAPH <urn:example:demo> { <urn:example:ahu1> <urn:example:hasLabel> ?label } }"
            )
        )
        assert [row.label for row in rows] == [Literal("AHU-1")]
    finally:
        env.close()


def test_backend_rdf5d_rejects_temporary_and_graph_store_envs(tmp_path: Path) -> None:
    ttl = tmp_path / "demo.ttl"
    _write_ttl(ttl, "urn:example:demo", 'ex:ahu1 ex:hasLabel "AHU-1" .')

    temp_env = OntoEnv(temporary=True, offline=True)
    try:
        temp_env.add(str(ttl))
        with pytest.raises(ValueError, match="backend='rdf5d'"):
            temp_env.snapshot_as_dataset(backend="rdf5d")
    finally:
        temp_env.close()

    store = DictGraphStore()
    external_env = OntoEnv(graph_store=store, temporary=True, init_from_store=True)
    try:
        with pytest.raises(ValueError, match="backend='rdf5d'"):
            external_env.snapshot_as_dataset(backend="rdf5d")
    finally:
        external_env.close()


def test_refresh_dataset_from_env_is_explicit(tmp_path: Path) -> None:
    first = tmp_path / "first.ttl"
    second = tmp_path / "second.ttl"
    _write_ttl(first, "urn:example:first", 'ex:first ex:hasLabel "First" .')
    _write_ttl(second, "urn:example:second", 'ex:second ex:hasLabel "Second" .')

    env = OntoEnv(path=tmp_path, recreate=True, offline=True)
    try:
        env.add(str(first))
        env.flush()

        dataset = env.snapshot_as_dataset()
        assert dataset.store._backend.backend_kind() == "rdf5d"
        assert list(
            dataset.query(
                "SELECT ?label WHERE { GRAPH <urn:example:second> { <urn:example:second> <urn:example:hasLabel> ?label } }"
            )
        ) == []

        env.add(str(second))
        env.flush()

        assert list(
            dataset.query(
                "SELECT ?label WHERE { GRAPH <urn:example:second> { <urn:example:second> <urn:example:hasLabel> ?label } }"
            )
        ) == []

        refresh_dataset_from_env(dataset, env)
        rows = list(
            dataset.query(
                "SELECT ?label WHERE { GRAPH <urn:example:second> { <urn:example:second> <urn:example:hasLabel> ?label } }"
            )
        )
        assert [row.label for row in rows] == [Literal("Second")]
    finally:
        env.close()


def test_dataset_from_env_with_other_store_forces_copy(tmp_path: Path) -> None:
    ttl = tmp_path / "demo.ttl"
    _write_ttl(ttl, "urn:example:demo", 'ex:ahu1 ex:hasLabel "AHU-1" .')

    env = OntoEnv(path=tmp_path, recreate=True, offline=True)
    try:
        env.add(str(ttl))
        env.flush()

        with pytest.raises(ValueError, match="requires an OntoEnvStore"):
            env.snapshot_as_dataset(backend="rdf5d", store=Graph().store)

        dataset = env.snapshot_as_dataset(store=Graph().store)
        rows = list(
            dataset.query(
                "SELECT ?label WHERE { GRAPH <urn:example:demo> { <urn:example:ahu1> <urn:example:hasLabel> ?label } }"
            )
        )
        assert [row.label for row in rows] == [Literal("AHU-1")]
    finally:
        env.close()
