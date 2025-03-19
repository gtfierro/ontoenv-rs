from ontoenv import Config, OntoEnv, version
from rdflib import Graph
print(version)


cfg = Config(["../brick"], strict=False, offline=False)

print("Make env")
env = OntoEnv(cfg)
print(env)
print("get brick")
g = Graph()
env.get_closure("https://brickschema.org/schema/1.4-rc1/Brick", g)
print(len(g))

print("get brick 2")
brick = Graph()
brick.parse("../brick/Brick.ttl", format="turtle")
env.import_dependencies(brick)
print(len(brick))
env.add("https://brickschema.org/schema/1.4/Brick.ttl")

print("new env")
env2 = OntoEnv()

print("get brick again from URL")
brick = env2.get_graph("https://brickschema.org/schema/1.4.2/Brick")
print(len(brick))
print(brick)
print(type(brick))

print(env2.list_closure("https://brickschema.org/schema/1.4.2/Brick"))

env2.import_graph(brick, "https://w3id.org/rec")
brick.serialize("test.ttl", format="turtle")

print(env2.get_dependents('https://brickschema.org/schema/1.4.2/Brick'))

# get an rdflib.Dataset (https://rdflib.readthedocs.io/en/stable/apidocs/rdflib.html#rdflib.Dataset)
ds = env2.to_rdflib_dataset()
for graphname in ds.graphs():
    graph = ds.graph(graphname)
    print(f"Graph {graphname} has {len(graph)} triples")
