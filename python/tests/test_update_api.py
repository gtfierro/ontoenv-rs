from pathlib import Path

import pytest
from rdflib import Literal, URIRef

from ontoenv import OntoEnv


def write_ontology(path: Path, value: str) -> None:
    path.write_text(
        f"""
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ex: <https://example.org/> .

ex:site a owl:Ontology ;
    ex:value "{value}" .
"""
    )


def test_update_location_replaces_existing_graph(tmp_path: Path) -> None:
    source = tmp_path / "site.ttl"
    write_ontology(source, "before")

    env = OntoEnv(temporary=True, use_cached_ontologies=True)
    ontology = env.add(source)

    write_ontology(source, "after")
    env.update(source)

    graph = env.get_graph(ontology)
    assert (
        URIRef("https://example.org/site"),
        URIRef("https://example.org/value"),
        Literal("after"),
    ) in graph
    assert (
        URIRef("https://example.org/site"),
        URIRef("https://example.org/value"),
        Literal("before"),
    ) not in graph
    env.close()


def test_update_force_refetches_known_sources(tmp_path: Path) -> None:
    source = tmp_path / "site.ttl"
    write_ontology(source, "before")
    env = OntoEnv(temporary=True, use_cached_ontologies=True)
    ontology = env.add(source)
    write_ontology(source, "after")
    env.update(force=True)
    assert (
        URIRef("https://example.org/site"),
        URIRef("https://example.org/value"),
        Literal("after"),
    ) in env.get_graph(ontology)
    env.close()


def test_update_rejects_positional_force() -> None:
    env = OntoEnv(temporary=True)
    with pytest.raises(TypeError, match="use force=True"):
        env.update(True)
    env.close()
