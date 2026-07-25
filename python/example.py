from ontoenv import OntoEnv, version
from rdflib import Graph


print(version)

print("Connect: create on first use and graph-free warm-open later.")
env = OntoEnv.connect(
    ".",
    search_directories=["../brick"],
    strict=False,
    offline=False,
)
if len(env) == 0:
    env.update(force=True)
print(env)

# returns fast, read-only closure graph plus the contributing ontology names
g, closure_names = env.get_closure("https://brickschema.org/schema/1.4/Brick")
# get an in-memory copy of the closure graph, which is mutable
# g, closure_names = env.copy_closure("https://brickschema.org/schema/1.4/Brick")
print(f"Brick closure has {len(g)} triples and includes the following ontologies: {closure_names}")

print("Working with local Brick")
brick = Graph()
brick.parse("../brick/Brick.ttl", format="turtle")
# use the env to get the dependencies of the graph, using the current content of the env as the source of truth for what is imported
env.import_dependencies(brick)
print(f"Brick graph has {len(brick)} triples after importing dependencies")
# add a new graph to the env
brick_name = env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")
print(f"Added {brick_name} to env")
env.close()

# Reopening reads catalog metadata and should be fast regardless of triple count.
env2 = OntoEnv.connect(".")
print(env2.store_path())

print("get brick again from URL")
# Gets a copy of the Brick graph from the env. We are using copy_graph
# so we can edit the graph below (with import_graph).
brick = env2.copy_graph("https://brickschema.org/schema/1.4/Brick")
# yo ucan get a mutable copy of the graph like so:
# brick = env2.copy_graph("https://brickschema.org/schema/1.4/Brick")
print(f"Brick graph has {len(brick)} triples")
print(brick)
print(type(brick))

# list the ontology URIs of the closure of Brick. This should include all the ontologies that are imported by Brick, directly or indirectly.
print("brick closure", env2.list_closure("https://brickschema.org/schema/1.4/Brick"))

# import a graph by name into a target graph. This will add all the triples
# from the source graph to the target graph, and it will also add all the
# triples from the closure of the source graph.
env2.import_graph(brick, "https://w3id.org/rec")
brick.serialize("test.ttl", format="turtle")

# list the ontologies that import a given ontology. This should include all the ontologies that import the given ontology, directly or indirectly.
print("qudtqk deps", env2.get_importers("http://qudt.org/2.1/vocab/quantitykind"))

# get an rdflib.Dataset (https://rdflib.readthedocs.io/en/stable/apidocs/rdflib.html#rdflib.Dataset)
# this is a fast, read-only dataset. use copy_dataset to get a mutable copy of the dataset.
ds = env2.get_dataset()
for graph in list(ds.graphs()):
    print(f"Graph {graph.identifier} has {len(graph)} triples")

env2.close()
