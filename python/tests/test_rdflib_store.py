from __future__ import annotations

from pathlib import Path

import pytest
from rdflib import Dataset, Graph, Literal, URIRef

from ontoenv import (
    OntoEnv,
    OntoEnvStore,
)
from ontoenv.rdflib_store import refresh_dataset_from_env


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

    dataset = persistent_env.get_dataset()
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


def test_rdf5d_store_streams_grouped_contexts(persistent_env: OntoEnv, tmp_path: Path) -> None:
    triple = (URIRef("urn:shared:s"), URIRef("urn:shared:p"), Literal("shared"))
    for name in ("one", "two"):
        path = tmp_path / f"{name}.ttl"
        path.write_text(
            f'<urn:graph:{name}> a <http://www.w3.org/2002/07/owl#Ontology> .\n'
            '<urn:shared:s> <urn:shared:p> "shared" .\n'
        )
        persistent_env.add(str(path))
    persistent_env.flush()

    backend = persistent_env.get_dataset().store._backend
    rows = list(backend.triples(*triple, None))
    assert len(rows) == 1
    assert rows[0][0] == triple
    assert {str(context) for context in rows[0][1]} == {"urn:graph:one", "urn:graph:two"}

    one = list(backend.triples(*triple, URIRef("urn:graph:one")))
    assert len(one) == 1
    assert {str(context) for context in one[0][1]} == {"urn:graph:one"}


def test_dataset_from_env_auto_falls_back_to_copy_for_temporary_env(temporary_env: OntoEnv) -> None:
    temporary_env.add(str(DEMO_TTL))

    dataset = temporary_env.get_dataset()
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
        temporary_env.snapshot_as_dataset(backend="rdf5d")

    store = DictGraphStore()
    external_env = OntoEnv(graph_store=store, temporary=True)
    try:
        with pytest.raises(ValueError, match="backend='rdf5d'"):
            external_env.snapshot_as_dataset(backend="rdf5d")
    finally:
        external_env.close()


def test_refresh_dataset_is_explicit(persistent_env: OntoEnv) -> None:
    persistent_env.add(str(FIRST_TTL))
    persistent_env.flush()

    dataset = persistent_env.get_dataset()
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

    refresh_dataset_from_env(dataset, persistent_env)
    rows = list(
        dataset.query(
            "SELECT ?label WHERE { GRAPH <urn:example:second> { <urn:example:second> <urn:example:hasLabel> ?label } }"
        )
    )
    assert [row.label for row in rows] == [Literal("Second")]


def test_get_graph_view_survives_unrelated_snapshot_rewrite(persistent_env: OntoEnv) -> None:
    persistent_env.add(str(FIRST_TTL))
    persistent_env.flush()

    graph = persistent_env.get_graph("urn:example:first")
    before = set(graph.triples((None, None, None)))

    persistent_env.add(str(SECOND_TTL))
    persistent_env.flush()

    assert set(graph.triples((None, None, None))) == before


def test_get_graph_view_observes_post_flush_mutation(
    persistent_env: OntoEnv, tmp_path: Path
) -> None:
    persistent_env.add(str(FIRST_TTL))
    persistent_env.flush()

    graph = persistent_env.get_graph("urn:example:first")
    sentinel = (
        URIRef("urn:example:first"),
        URIRef("urn:example:hasLabel"),
        Literal("First-Updated"),
    )
    assert sentinel not in set(graph.triples((None, None, None)))

    replacement = tmp_path / "first-updated.ttl"
    replacement.write_text(
        "\n".join(
            [
                "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
                "@prefix ex: <urn:example:> .",
                "",
                "<urn:example:first> a owl:Ontology .",
                'ex:first ex:hasLabel "First-Updated" .',
                "",
            ]
        )
    )
    persistent_env.add(str(replacement), overwrite=True)
    persistent_env.flush()

    assert sentinel in set(graph.triples((None, None, None)))


def test_dataset_from_env_with_other_store_forces_copy(persistent_env: OntoEnv) -> None:
    persistent_env.add(str(DEMO_TTL))
    persistent_env.flush()

    with pytest.raises(ValueError, match="requires an OntoEnvStore"):
        persistent_env.snapshot_as_dataset(backend="rdf5d", store=Graph().store)

    dataset = persistent_env.copy_dataset()
    rows = list(
        dataset.query(
            "SELECT ?label WHERE { GRAPH <urn:example:demo> { <urn:example:ahu1> <urn:example:hasLabel> ?label } }"
        )
    )
    assert [row.label for row in rows] == [Literal("AHU-1")]


def test_bound_namespaces_persist_across_reopen(tmp_path: Path) -> None:
    env = OntoEnv(path=tmp_path, recreate=True, offline=True)
    try:
        env.add(str(DEMO_TTL))
        env.bind_namespace("demo", "urn:example:")
        env.bind_namespace("custom", "urn:custom#")
        assert env.get_bound_namespaces() == {"demo": "urn:example:", "custom": "urn:custom#"}
        # The user prefix replaces the ontology's `ex:` for the same namespace.
        namespaces = env.get_namespaces()
        assert namespaces["demo"] == "urn:example:"
        assert "ex" not in namespaces
    finally:
        env.close()

    env = OntoEnv(path=tmp_path, offline=True)
    try:
        assert env.get_bound_namespaces() == {"demo": "urn:example:", "custom": "urn:custom#"}
        dataset = env.get_dataset()
        bound = {prefix: str(ns) for prefix, ns in dataset.namespaces()}
        assert bound["demo"] == "urn:example:"
        assert bound["custom"] == "urn:custom#"
        assert "@prefix demo: <urn:example:>" in dataset.serialize(format="trig")

        assert env.unbind_namespace("custom") is True
        assert env.unbind_namespace("custom") is False
    finally:
        env.close()

    env = OntoEnv(path=tmp_path, offline=True)
    try:
        assert env.get_bound_namespaces() == {"demo": "urn:example:"}
    finally:
        env.close()


@pytest.mark.parametrize(
    ("prefix", "existing_namespace"),
    [("schema", None), ("chosen", "urn:old#")],
)
def test_copy_dataset_saved_prefix_replaces_collision(
    persistent_env: OntoEnv, prefix: str, existing_namespace: str | None
) -> None:
    persistent_env.add(str(DEMO_TTL))
    persistent_env.bind_namespace(prefix, "urn:example:")

    destination = None
    if existing_namespace is not None:
        destination = Dataset()
        destination.bind(prefix, URIRef(existing_namespace))

    dataset = persistent_env.copy_dataset(destination)
    if destination is not None:
        assert dataset is destination
    bound = dict(dataset.namespaces())
    assert bound[prefix] == URIRef("urn:example:")
    assert bound.get(f"{prefix}1") != URIRef("urn:example:")
    serialized = dataset.serialize(format="trig")
    assert f"@prefix {prefix}: <urn:example:>" in serialized
    assert f"{prefix}:ahu1" in serialized


def test_refresh_resets_to_env_bindings(persistent_env: OntoEnv) -> None:
    persistent_env.add(str(DEMO_TTL))
    persistent_env.flush()
    dataset = persistent_env.get_dataset()
    dataset.bind("session", URIRef("urn:session#"))

    persistent_env.bind_namespace("persisted", "urn:persisted#")
    refresh_dataset_from_env(dataset, persistent_env)

    bound = {prefix: str(ns) for prefix, ns in dataset.namespaces()}
    assert "session" not in bound
    assert bound["persisted"] == "urn:persisted#"
    assert bound["ex"] == "urn:example:"


def _write_import_pair(directory: Path) -> tuple[Path, Path]:
    root = directory / "root.ttl"
    root.write_text(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
        "@prefix rt: <urn:root#> .\n"
        "<urn:root> a owl:Ontology ; owl:imports <urn:dep> .\n"
        "rt:a rt:p rt:b .\n"
    )
    dep = directory / "dep.ttl"
    dep.write_text(
        "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
        "@prefix dep: <urn:dep#> .\n"
        "<urn:dep> a owl:Ontology .\n"
        "dep:x dep:p dep:y .\n"
    )
    return root, dep


@pytest.mark.parametrize("temporary", [False, True])
def test_views_carry_env_namespaces(tmp_path: Path, temporary: bool) -> None:
    src = tmp_path / "src"
    src.mkdir()
    root, dep = _write_import_pair(src)
    if temporary:
        env = OntoEnv(temporary=True, offline=True)
    else:
        env = OntoEnv(path=tmp_path / "env", recreate=True, offline=True)
    try:
        env.add(str(dep))
        env.add(str(root))
        env.flush()
        if not temporary:
            env.bind_namespace("mine", "urn:mine#")

        closure, _ = env.get_closure("urn:root")
        assert closure.namespace("rt") == "urn:root#"
        assert closure.namespace("dep") == "urn:dep#"
        if not temporary:
            assert closure.namespace("mine") == "urn:mine#"
        turtle = closure.serialize(format="turtle")
        assert "@prefix dep: <urn:dep#>" in turtle
        assert "@prefix rt: <urn:root#>" in turtle

        union, _ = env.get_union(["urn:dep"])
        assert union.namespace("dep") == "urn:dep#"
        assert union.namespace("rt") is None
    finally:
        env.close()

