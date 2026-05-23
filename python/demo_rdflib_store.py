from pathlib import Path

from ontoenv import OntoEnv, dataset_from_env, refresh_dataset_from_env, version
from rdflib import Literal, URIRef


ROOT = Path(__file__).resolve().parents[1]
BRICK_DIR = ROOT / "brick"
BRICK_FILE = BRICK_DIR / "Brick.ttl"
DEMO_ENV = ROOT / "python" / ".demo-env"


print(version)

print("Make env")
env = OntoEnv(
    path=DEMO_ENV,
    recreate=True,
    strict=False,
    offline=True,
    search_directories=[str(BRICK_DIR)],
)
print(env)

print("add brick and persist rdf5d store")
brick_name = env.add(str(BRICK_FILE))
env.update()
env.flush()

# Build an rdflib.Dataset backed directly by .ontoenv/store.r5tu when possible.
dataset = dataset_from_env(env, mode="rdf5d")
print("dataset backend", dataset.store._backend.backend_kind())

print("graphs in dataset")
for graph in list(dataset.graphs())[:5]:
    print(f"{graph.identifier} -> {len(graph)} triples")

print("query brick labels")
for row in dataset.query(
    """
    SELECT ?entity ?label
    WHERE {
      GRAPH <https://brickschema.org/schema/1.4/Brick> {
        ?entity <http://www.w3.org/2000/01/rdf-schema#label> ?label .
      }
    }
    LIMIT 5
    """
):
    print(row.entity, row.label)

print("graph access")
brick_graph = dataset.graph(URIRef(brick_name))
print(len(brick_graph))

# Snapshot datasets stay stable until you explicitly refresh them.
print("add one more ontology without refreshing snapshot")
extra_ttl = DEMO_ENV / "demo-extra.ttl"
extra_ttl.write_text(
    "\n".join(
        [
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .",
            "@prefix ex: <urn:demo:> .",
            "<urn:demo:extra> a owl:Ontology .",
            'ex:item ex:label "demo value" .',
        ]
    ),
    encoding="utf-8",
)
env.add(str(extra_ttl))
env.flush()

rows = list(
    dataset.query(
        """
        SELECT ?label
        WHERE {
          GRAPH <urn:demo:extra> {
            <urn:demo:item> <urn:demo:label> ?label .
          }
        }
        """
    )
)
print("rows before refresh", rows)

print("refresh snapshot")
refresh_dataset_from_env(dataset, env)
rows = list(
    dataset.query(
        """
        SELECT ?label
        WHERE {
          GRAPH <urn:demo:extra> {
            <urn:demo:item> <urn:demo:label> ?label .
          }
        }
        """
    )
)
print("rows after refresh", [row.label for row in rows])

print("snapshot is read-only")
try:
    brick_graph.add((URIRef("urn:demo:s"), URIRef("urn:demo:p"), Literal("x")))
except Exception as err:
    print(type(err).__name__, err)

env.close()
