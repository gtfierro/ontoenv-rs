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


def test_update_all_remains_a_deprecated_force_alias() -> None:
    env = OntoEnv(temporary=True)

    with pytest.warns(DeprecationWarning, match=r"use update\(force=\.\.\.\)"):
        env.update(all=True)

    with pytest.warns(DeprecationWarning, match=r"use update\(force=\.\.\.\)"):
        env.update(False)

    env.close()


def test_update_rejects_location_with_legacy_all(tmp_path: Path) -> None:
    source = tmp_path / "site.ttl"
    write_ontology(source, "value")
    env = OntoEnv(temporary=True)

    with pytest.warns(DeprecationWarning):
        with pytest.raises(TypeError, match="cannot combine a location"):
            env.update(source, all=True)

    env.close()
