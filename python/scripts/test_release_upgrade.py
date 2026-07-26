"""Exercise persistent-environment upgrades from released OntoEnv wheels.

CI runs ``create`` with an older wheel, installs the current source into the
same virtual environment, and then runs ``verify``. Both phases share this
small fixture so the migration test stays deterministic and easy to extend.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from rdflib import Graph, Literal, URIRef
from rdflib.namespace import OWL, RDF

ROOT_IRI = "https://example.org/upgrade-root"
PREDICATE = URIRef("https://example.org/value")
VALUE = Literal("created-by-older-release")


def create(root: Path) -> None:
    """Create a minimal persistent environment with the installed release."""
    from ontoenv import OntoEnv

    graph = Graph()
    subject = URIRef(ROOT_IRI)
    graph.add((subject, RDF.type, OWL.Ontology))
    graph.add((subject, PREDICATE, VALUE))
    environment = OntoEnv(path=root, recreate=True, offline=True)
    environment.add(graph, fetch_imports=False)
    environment.close()


def verify(root: Path) -> None:
    """Open and validate the old environment with the current source."""
    from ontoenv import OntoEnv

    environment = OntoEnv.open(root)
    assert ROOT_IRI in environment.get_ontology_names()
    graph = environment.copy_graph(ROOT_IRI)
    assert (URIRef(ROOT_IRI), PREDICATE, VALUE) in graph
    environment.close()
    assert (root / ".ontoenv" / "catalog.r5tu").exists()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("phase", choices=("create", "verify"))
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    if args.phase == "create":
        create(args.root)
    else:
        verify(args.root)


if __name__ == "__main__":
    main()
