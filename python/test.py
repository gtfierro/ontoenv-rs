from ontoenv import Config, OntoEnv
from rdflib import Graph

cfg = Config(
    root=".",
    search_directories=["../brick"],
    includes = [],
    excludes = [],
    require_ontology_names = True,
    resolution_policy = 'default',
    )

env = OntoEnv(cfg)
print(env)
#env.update()
g = Graph()
env.get_closure("https://brickschema.org/schema/1.4-rc1/Brick", g)
print(len(g))

brick = Graph()
brick.parse("../brick/Brick.ttl", format="turtle")
env.import_dependencies(brick)
print(len(brick))
