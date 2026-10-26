"""The normal Python API must pin catalog, namespaces and RDF to one version."""
from datetime import datetime
import gc
import json
from pathlib import Path

import pytest

from rdflib import BNode, Graph, Literal, URIRef
from rdflib.namespace import OWL, RDF, RDFS

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
    for pinned in (view, closure, old.view_graph(str(NAME)), dataset.graph(NAME)):
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
        view = snapshot.view_graph(str(NAME))
        iterator = view.triples((None, None, None))
        first = next(iterator)
    assert snapshot.closed
    snapshot.close()  # idempotent
    for operation in (lambda: snapshot.info, snapshot.view_dataset, snapshot.get_ontology_names):
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


def test_scoped_dataset_view_copy_contexts_and_sparql(tmp_path: Path):
    a, b, outside, empty = map(URIRef, ("urn:a", "urn:b", "urn:outside", "urn:empty"))
    shared = (URIRef("urn:shared"), PREDICATE, Literal("shared"))
    with OntoEnv.connect(tmp_path, offline=True) as writer:
        for name in (a, b, outside):
            value = Graph(identifier=name)
            value.add((name, RDF.type, OWL.Ontology))
            value.add(shared)
            if name == a:
                node = BNode()
                value.add((a, PREDICATE, node))
                value.add((node, PREDICATE, Literal("blank")))
                value.add((a, RDFS.subClassOf, b))
            if name == outside:
                value.add((b, RDFS.subClassOf, outside))
            writer.add(value, fetch_imports=False)
        writer.add(Graph(), rename=str(empty), fetch_imports=False)
        writer.add_alias("urn:alias", str(a))
        snapshot = writer.snapshot()
    view = snapshot.view_dataset(["urn:alias", str(b), str(a), str(empty)])
    copy = snapshot.copy_dataset([str(a), str(b), str(empty)])
    assert set(view.quads()) == set(copy.quads())
    assert shared in view.graph(a) and shared in view.graph(b)
    assert {g.identifier for g in view.graphs()} >= {a, b, empty}
    assert outside not in {g.identifier for g in view.graphs()}
    assert empty in {g.identifier for g in copy.graphs()}
    assert len(view) == len(copy)
    assert set(view.graph(outside)) == set()
    assert list(view.query(f"SELECT ?s WHERE {{ GRAPH <{outside}> {{ ?s ?p ?o }} }}")) == []
    assert list(view.query(f"SELECT ?s FROM <{outside}> WHERE {{ ?s ?p ?o }}")) == []
    assert list(view.query(f"SELECT ?s FROM <{outside}> WHERE {{ ?s <{RDFS.subClassOf}>+ ?o }}")) == []
    assert list(view.query("SELECT * WHERE { ?s ?p ?o }")) == []
    assert list(view.query(f"SELECT * WHERE {{ ?s <{RDFS.subClassOf}>+ ?o }}")) == []
    assert list(view.query(f"SELECT ?s FROM <{a}> WHERE {{ ?s <{RDFS.subClassOf}> <{b}> }}")) == [(a,)]
    assert list(view.query(f"SELECT ?s WHERE {{ GRAPH <{a}> {{ ?s <{RDFS.subClassOf}>+ <{outside}> }} }}")) == []
    rows = list(view.query(f"SELECT ?g WHERE {{ GRAPH ?g {{ <urn:shared> <{PREDICATE}> \"shared\" }} }}"))
    assert {row.g for row in rows} == {a, b}
    count = next(iter(view.query("SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }"))).n
    assert int(count) == len(list(copy.quads()))
    for dataset in (snapshot.view_dataset([]), snapshot.copy_dataset([])):
        assert len(dataset) == 0
        assert list(dataset.quads()) == []
        assert list(dataset.query("SELECT * WHERE { GRAPH ?g { ?s ?p ?o } }")) == []
    for method in (snapshot.view_dataset, snapshot.copy_dataset):
        with pytest.raises(ValueError, match="No graph"):
            method([str(a), "urn:missing"])
    with pytest.raises(ValueError, match="read-only"):
        view.graph(a).add((a, PREDICATE, Literal("changed")))
    copy.graph(a).add((a, PREDICATE, Literal("changed")))
    assert (a, PREDICATE, Literal("changed")) not in view.graph(a)


def test_snapshot_union_projection_and_closure_pairs(tmp_path: Path):
    a, b = URIRef("urn:a"), URIRef("urn:b")
    shared = (URIRef("urn:shared"), PREDICATE, Literal("shared"))
    with OntoEnv.connect(tmp_path, offline=True) as writer:
        root = Graph(identifier=a)
        root.add((a, RDF.type, OWL.Ontology))
        root.add((a, OWL.imports, URIRef("urn:alias")))
        root.add(shared)
        dependency = Graph(identifier=b)
        dependency.add((b, RDF.type, OWL.Ontology))
        dependency.add(shared)
        writer.add(dependency, fetch_imports=False)
        writer.add_alias("urn:alias", str(b))
        writer.add(root, fetch_imports=False)
        snapshot = writer.snapshot()
        for remove_imports in (False, True):
            for rewrite in (False, True):
                options = dict(remove_owl_imports=remove_imports, rewrite_sh_prefixes=rewrite)
                view, names = snapshot.view_closure(str(a), **options)
                copy, copied_names = snapshot.copy_closure(str(a), **options)
                existing, _ = writer.get_closure(str(a), **options)
                assert names == copied_names == snapshot.closure_iris(str(a)) == [str(a), str(b)]
                assert set(view) == set(copy) == set(existing)
                assert len(view) == len(copy)
        assert snapshot.get_importers("urn:alias") == [str(a)]
        assert snapshot.get_import_paths("urn:alias") == [[str(a), str(b)]]
        assert snapshot.explain_import("urn:alias") == {"present": True, "paths": [[str(a), str(b)]]}
        assert snapshot.explain_import("urn:unknown") == {"present": False, "paths": []}
        view, names = snapshot.view_union([str(a), str(b), "urn:alias"])
        copy, copied_names = snapshot.copy_union([str(a), str(b)])
        assert names == copied_names == [str(a), str(b)]
        assert set(view) == set(copy) == set(root) | set(dependency)
        assert len(list(view)) == len(view) == len(copy)
        assert int(next(iter(view.query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"))).n) == len(copy)
        assert list(view.triples(shared)) == [shared]
        assert len(list(snapshot.view_dataset([str(a), str(b)]).quads())) == len(root) + len(dependency)
        empty, names = snapshot.view_union([])
        assert names == [] and len(empty) == 0 and list(empty) == []
        assert list(empty.query("SELECT * WHERE { ?s ?p ?o }")) == []
        assert shared not in empty
        assert list(empty.subjects()) == []


def test_snapshot_queries_and_escaped_results_keep_one_catalog(tmp_path: Path):
    a, b = URIRef("urn:a"), URIRef("urn:b")
    with OntoEnv.connect(tmp_path, offline=True) as writer:
        writer.add(Graph(), rename=str(b), fetch_imports=False)
        root = Graph(identifier=a)
        root.add((a, RDF.type, OWL.Ontology))
        root.add((a, OWL.imports, URIRef("urn:alias")))
        writer.add_alias("urn:alias", str(b))
        writer.add(root, fetch_imports=False)
        writer.bind_namespace("chosen", "urn:old:")
        old = writer.snapshot()
        view, names = old.view_closure(str(a))
        dataset = old.view_dataset([str(a)])
        iterator = dataset.quads()
        first = next(iterator)
        replacement = Graph(identifier=a)
        replacement.add((a, RDF.type, OWL.Ontology))
        writer.add(replacement, overwrite=True, fetch_imports=False)
        writer.remove_alias("urn:alias")
        writer.bind_namespace("chosen", "urn:new:")
        assert writer.snapshot().closure_iris(str(a)) == [str(a)]
        assert old.closure_iris(str(a)) == [str(a), str(b)]
        assert old.get_importers("urn:alias") == [str(a)]
        assert old.get_namespaces()["chosen"] == "urn:old:"
        old.close()
    with OntoEnv.open(tmp_path) as writer:
        writer.prune()
        assert names == [str(a), str(b)]
        # The existing transform removes selected canonical import IRIs;
        # the explicit alias term is preserved.
        assert set(view) == set(root)
        assert {first, *iterator} == {(*triple, a) for triple in root}


def test_temporary_union_counts_graph_projection():
    a, b = URIRef("urn:a"), URIRef("urn:b")
    with OntoEnv(temporary=True, offline=True) as env:
        for name in (a, b):
            graph = Graph(identifier=name)
            graph.add((name, RDF.type, OWL.Ontology))
            graph.add((URIRef("urn:shared"), PREDICATE, Literal("shared")))
            env.add(graph, fetch_imports=False)
        view, _ = env.get_union([str(a), str(b)])
        assert len(list(view)) == len(view) == 3
        assert int(next(iter(view.query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"))).n) == 3
        dataset = env.get_dataset()
        dataset.default_union = True
        assert int(next(iter(dataset.query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"))).n) == 3


@pytest.mark.parametrize("temporary", [False, True])
def test_multiple_from_merges_default_graph_without_duplicating_triples(tmp_path: Path, temporary: bool):
    env = OntoEnv(temporary=True, offline=True) if temporary else OntoEnv.connect(tmp_path, offline=True)
    a, b, outside = map(URIRef, ("urn:a", "urn:b", "urn:outside"))
    shared = (URIRef("urn:shared"), PREDICATE, Literal("shared"))
    for name, parent in ((a, b), (b, outside), (outside, URIRef("urn:other"))):
        value = Graph(identifier=name)
        value.add((name, RDF.type, OWL.Ontology))
        value.add(shared)
        value.add((name, RDFS.subClassOf, parent))
        env.add(value, fetch_imports=False)
    views = [env.get_dataset(), env.get_union([str(a), str(b)])[0]]
    if not temporary:
        snapshot = env.snapshot()
        views += [snapshot.view_dataset([str(a), str(b)]), snapshot.view_union([str(a), str(b)])[0]]
    copied = env.copy_dataset()
    queries = [
        ("FROM <urn:a> FROM <urn:b>", "?s ?p ?o", 5),
        ("FROM <urn:a> FROM <urn:a> FROM <urn:b>", "?s ?p ?o", 5),
        ("FROM <urn:a>", "?s ?p ?o", 3),
        ("FROM NAMED <urn:a> FROM NAMED <urn:b>", "?s ?p ?o", 0),
        ("FROM <urn:a> FROM <urn:b> FROM NAMED <urn:a> FROM NAMED <urn:b>",
         "GRAPH ?g { ?s ?p ?o }", 6),
        ("FROM <urn:a> FROM <urn:b>", "GRAPH ?g { ?s ?p ?o }", 0),
    ]
    for clauses, pattern, expected in queries:
        query = f"SELECT (COUNT(*) AS ?n) {clauses} WHERE {{ {pattern} }}"
        assert int(next(iter(copied.query(query))).n) == expected
        for view in views:
            assert int(next(iter(view.query(query))).n) == expected
    path = f"SELECT ?o FROM <urn:a> FROM <urn:b> WHERE {{ <urn:a> <{RDFS.subClassOf}>+ ?o }}"
    for view in views:
        assert set(view.query(path)) == {(b,), (outside,)}
    env.close()


def test_graph_copies_keep_captured_prefixes_and_destination_preferences(tmp_path: Path):
    source = tmp_path / "prefixes.ttl"
    source.write_text('''@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix declared: <urn:declared:> .
<urn:a> a owl:Ontology; declared:p "parsed"; <urn:old:p> "old" .
''')
    with OntoEnv.connect(tmp_path, offline=True) as writer:
        writer.add(source, fetch_imports=False)
        writer.bind_namespace("chosen", "urn:old:")
        snapshot = writer.snapshot()
        view = snapshot.view_graph("urn:a")
        snapshot_copy = snapshot.copy_graph("urn:a")
        live_copy = writer.copy_graph("urn:a")
        destination = Graph()
        destination.bind("chosen", URIRef("urn:destination:"))
        destination.bind("declared", URIRef("urn:destination:"))
        destination.bind("preferred", URIRef("urn:declared:"))
        assert writer.copy_graph("urn:a", destination) is destination
        writer.bind_namespace("chosen", "urn:new:")
        snapshot.close()
    for value in (view, snapshot_copy, live_copy, destination):
        assert dict(value.namespaces())["chosen"] == URIRef("urn:old:")
        assert list(value.query('SELECT ?o WHERE { <urn:a> chosen:p ?o }')) == [(Literal("old"),)]
        assert "chosen:p" in value.serialize(format="turtle")
    for value in (snapshot_copy, live_copy):
        assert dict(value.namespaces())["declared"] == URIRef("urn:declared:")
    assert dict(destination.namespaces())["declared"] == URIRef("urn:destination:")
    assert destination.store.prefix(URIRef("urn:declared:")) == "preferred"
