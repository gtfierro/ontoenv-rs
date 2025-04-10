//! Defines constant NamedNodeRefs for commonly used RDF terms and predicates,
//! primarily from OWL, RDFS, DCTERMS, VAEM, and SHACL vocabularies.

use oxigraph::model::NamedNodeRef;

pub const ONTOLOGY: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/2002/07/owl#Ontology");
pub const IMPORTS: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/2002/07/owl#imports");
pub const TYPE: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");

// uris for ontology versioning
// owl
pub const VERSION_INFO: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/2002/07/owl#versionInfo");
pub const VERSION_IRI: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/2002/07/owl#versionIRI");
// rdfs
pub const DEFINED_BY: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#isDefinedBy");
pub const SEE_ALSO: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#seeAlso");
pub const LABEL: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/2000/01/rdf-schema#label");
// dcterms
pub const CREATED: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://purl.org/dc/terms/created");
pub const MODIFIED: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://purl.org/dc/terms/modified");
pub const HAS_VERSION: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://purl.org/dc/terms/hasVersion");
pub const TITLE: NamedNodeRef<'_> = NamedNodeRef::new_unchecked("http://purl.org/dc/terms/title");
// vaem
pub const HAS_GRAPH_METADATA: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.linkedmodel.org/schema/vaem#hasGraphMetadata");
pub const REVISION: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.linkedmodel.org/schema/vaem#revision");
// shacl
pub const PREFIXES: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#prefixes");
pub const DECLARE: NamedNodeRef<'_> =
    NamedNodeRef::new_unchecked("http://www.w3.org/ns/shacl#declare");

pub const ONTOLOGY_VERSION_IRIS: [NamedNodeRef<'_>; 10] = [
    VERSION_INFO,
    VERSION_IRI,
    DEFINED_BY,
    SEE_ALSO,
    CREATED,
    MODIFIED,
    HAS_VERSION,
    LABEL,
    TITLE,
    REVISION,
];
