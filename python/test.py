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
del env

print("new env")
env2 = OntoEnv()
print(env2.store_path())

print("get brick again from URL")
brick = env2.get_graph("https://brickschema.org/schema/1.4/Brick")
print(len(brick))
print(brick)
print(type(brick))

print("brick closure", env2.list_closure("https://brickschema.org/schema/1.4/Brick"))

env2.import_graph(brick, "https://w3id.org/rec")
brick.serialize("test.ttl", format="turtle")

print("qudtqk deps", env2.get_dependents('http://qudt.org/2.1/vocab/quantitykind'))

# get an rdflib.Dataset (https://rdflib.readthedocs.io/en/stable/apidocs/rdflib.html#rdflib.Dataset)
ds = env2.to_rdflib_dataset()
for graphname in ds.graphs():
    graph = ds.graph(graphname)
    print(f"Graph {graphname} has {len(graph)} triples")
