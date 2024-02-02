import rdflib
import json

# RDF namespaces
BRICK = rdflib.Namespace("https://brickschema.org/schema/Brick#")
REC = rdflib.Namespace("https://w3id.org/rec#")
OWL = rdflib.OWL
XSD = rdflib.XSD

# Load the data from the Turtle file (assuming you have saved this to 'data.ttl')
graph = rdflib.Graph()
graph.parse("brickpatches.ttl", format="turtle") # Replace 'turtle_data' with the actual Turtle data

# Query for deprecated classes including their properties and replacedBy information
qres = graph.query(
    """
    PREFIX brick: <https://brickschema.org/schema/Brick#>
    PREFIX owl: <http://www.w3.org/2002/07/owl#>
    SELECT ?deprecatedClass ?version ?message ?replacement
    WHERE {
        ?deprecatedClass owl:deprecated "true"^^xsd:boolean .
        OPTIONAL { ?deprecatedClass brick:deprecatedInVersion ?version . }
        OPTIONAL { ?deprecatedClass brick:deprecationMitigationMessage ?message . }
        OPTIONAL { ?deprecatedClass brick:isReplacedBy ?replacement . }
    }
    """
)

# Convert the query results to a JSON structure
deprecated_classes = []
for row in qres:
    class_info = {
        "deprecatedClass": str(row.deprecatedClass),
        "deprecatedInVersion": row.version.toPython() if row.version else None,
        "deprecationMitigationMessage": row.message.toPython() if row.message else None,
    }
    # Add 'replacedBy' only if it exists
    if row.replacement:
        class_info["isReplacedBy"] = str(row.replacement)

    deprecated_classes.append(class_info)

# Output the JSON
print(json.dumps(deprecated_classes, indent=4))
