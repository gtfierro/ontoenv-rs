from ontoenv import OntoEnv, version
from rdflib import Literal, URIRef


print(version)

print("Make env")
env = OntoEnv(
    path=".demo-env",
    recreate=True,
    strict=False,
    offline=True,
    search_directories=["../brick"],
)
print(env)

print("add brick and persist rdf5d store")
brick_name = env.add("../brick/Brick.ttl")
env.update()
env.flush()

# Build an rdflib.Dataset backed directly by .ontoenv/store.r5tu when possible.
dataset = env.as_dataset(backend="rdf5d")
print("dataset backend", dataset.store._backend.backend_kind())

print("graphs in dataset")
for graph in list(dataset.graphs())[:5]:
    print(f"{graph.identifier} -> {len(graph)} triples")

print("query brick labels")
for row in dataset.query(
    """
    SELECT ?entity ?label
    WHERE {
      ?entity <http://www.w3.org/2000/01/rdf-schema#label> ?label .
    }
    LIMIT 5
    """
):
    print(row.entity, row.label)

print("graph access")
brick_graph = dataset.graph(URIRef(brick_name))
print(len(brick_graph))

# Snapshot datasets stay stable until you explicitly refresh them.
# Note that this is *separate* from updating the environment.
env.refresh_dataset(dataset)

env.close()
