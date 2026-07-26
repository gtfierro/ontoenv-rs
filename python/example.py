"""End-to-end example of persistent OntoEnv and dependency resolution."""

from ontoenv import OntoEnv, UnresolvedImportError, version
from rdflib import Graph


BRICK = "https://brickschema.org/schema/1.4/Brick"

print(f"OntoEnv {version}")
print("Connect: create on first use and graph-free warm-open later.")
env = OntoEnv.connect(
    ".",
    search_directories=["../brick"],
    strict=False,
    offline=False,
)
# update the catalog if it is empty, which will fetch and cache Brick and its
# dependencies.
if len(env) == 0:
    env.update(force=True)
print(env)

# Return a fast, read-only closure view plus the contributing ontology names.
g, closure_names = env.get_closure(BRICK)
print(
    f"Brick closure has {len(g)} triples and includes "
    f"the following ontologies: {closure_names}"
)

print("Working with local Brick")
brick = Graph()
brick.parse("../brick/Brick.ttl", format="turtle")

# Resolve the graph's owl:imports using cached ontologies first, fetching any
# missing targets. In non-strict mode, unavailable imports are recorded and
# skipped. A completed best-effort call does not leave catalog.pending.
imported = env.import_dependencies(brick, fetch_missing=True)
print(
    f"Brick graph has {len(brick)} triples after importing "
    f"{len(imported)} ontologies"
)

# copy_graph gives every currently known unresolved import one catchable type.
missing = sorted(env.missing_imports())
print(f"Unresolved imports: {missing}")
if missing:
    try:
        env.copy_graph(missing[0])
    except UnresolvedImportError as error:
        print(f"Expected unresolved import: {error}")

# Add a new graph to the environment.
brick_name = env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")
print(f"Added {brick_name} to env")
env.close()

# Reopening reads catalog metadata and should be fast regardless of triple count.
print("Reopen OntoEnv. Shoudl be warm read now")
env2 = OntoEnv.connect(".")
print(env2.store_path())

print("get brick again from URL")
# get_graph returns an immutable, read-only graph view of the named ontology.
brick = env2.get_graph(BRICK)
print(f"Brick graph has {len(brick)} triples")
# copy_graph returns a mutable in-memory graph, unlike the read-only get_graph.
brick = env2.copy_graph(BRICK)
print(f"Brick graph has {len(brick)} triples")

# List ontology IRIs imported by Brick, directly or indirectly.
print("brick closure", env2.list_closure(BRICK))

# import a graph by name into a target graph. This will add all the triples
# from the source graph to the target graph, and it will also add all the
# triples from the closure of the source graph.
env2.import_graph(brick, "https://w3id.org/rec")
brick.serialize("test.ttl", format="turtle")

# List ontologies that import a given ontology.
print("qudtqk deps", env2.get_importers("http://qudt.org/2.1/vocab/quantitykind"))

# get_dataset returns a fast, read-only rdflib.Dataset. Use copy_dataset when
# a mutable in-memory dataset is needed.
ds = env2.get_dataset()
for graph in list(ds.graphs()):
    print(f"Graph {graph.identifier} has {len(graph)} triples")

env2.close()
