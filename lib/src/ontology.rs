//! Defines the core data structures for representing ontologies and their metadata within the OntoEnv.
//! Includes `Ontology`, `GraphIdentifier`, and `OntologyLocation`.

use crate::consts::*;
use crate::util::{read_file, read_url};
use anyhow::Result;
use chrono::prelude::*;
use log::{debug, info, warn};
use oxigraph::model::{
    Graph as OxigraphGraph, GraphName, NamedNode, NamedNodeRef, Subject, SubjectRef, TermRef,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, DeserializeAs, SerializeAs};
use std::collections::HashMap;
use std::hash::Hash;
use std::path::PathBuf;
//
// custom derive for NamedNode
fn namednode_ser<S>(namednode: &NamedNode, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(namednode.as_str())
}

fn namednode_de<'de, D>(deserializer: D) -> Result<NamedNode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    NamedNode::new(s).map_err(serde::de::Error::custom)
}

#[derive(Serialize, Deserialize, Eq, Debug, Clone)]
pub struct GraphIdentifier {
    location: OntologyLocation,
    #[serde(serialize_with = "namednode_ser", deserialize_with = "namednode_de")]
    name: NamedNode,
}

// equality for GraphIdentifier is based on the name and location
impl PartialEq for GraphIdentifier {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.location == other.location
    }
}

impl Hash for GraphIdentifier {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.location.hash(state);
    }
}

impl std::fmt::Display for GraphIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} @ {}", self.name, self.location)
    }
}

impl Into<NamedNode> for GraphIdentifier {
    fn into(self) -> NamedNode {
        self.name
    }
}

impl<'a> Into<NamedNodeRef<'a>> for &'a GraphIdentifier {
    fn into(self) -> NamedNodeRef<'a> {
        (&self.name).into()
    }
}

impl GraphIdentifier {
    pub fn new(name: NamedNodeRef) -> Self {
        // location is same as name
        GraphIdentifier {
            location: OntologyLocation::from_str(name.as_str()).unwrap(),
            name: name.into(),
        }
    }
    pub fn location(&self) -> &OntologyLocation {
        &self.location
    }

    pub fn name(&self) -> NamedNodeRef {
        self.name.as_ref()
    }

    pub fn to_filename(&self) -> String {
        let name = self.name.as_str().replace(':', "+");
        let location = self.location.as_str().replace("file://", "");
        format!("{}-{}", name, location).replace('/', "_")
    }
    pub fn graphname(&self) -> Result<GraphName> {
        Ok(GraphName::NamedNode(self.name.clone()))
    }
}

#[derive(Serialize, Deserialize, Hash, Clone, Eq, PartialEq, Debug)]
pub enum OntologyLocation {
    #[serde(rename = "file")]
    File(PathBuf),
    #[serde(rename = "url")]
    Url(String),
}

// impl display for OntologyLocation
impl std::fmt::Display for OntologyLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OntologyLocation::File(p) => write!(f, "file://{}", p.to_str().unwrap_or_default()),
            OntologyLocation::Url(u) => write!(f, "{}", u),
        }
    }
}

// impl default for OntologyLocation
impl Default for OntologyLocation {
    fn default() -> Self {
        OntologyLocation::File(PathBuf::new())
    }
}

impl OntologyLocation {
    pub fn as_str(&self) -> &str {
        match self {
            OntologyLocation::File(p) => p.to_str().unwrap_or_default(),
            OntologyLocation::Url(u) => u.as_str(),
        }
    }

    pub fn graph(&self) -> Result<OxigraphGraph> {
        match self {
            OntologyLocation::File(p) => read_file(p),
            OntologyLocation::Url(u) => read_url(u),
        }
    }

    pub fn is_file(&self) -> bool {
        match self {
            OntologyLocation::File(_) => true,
            OntologyLocation::Url(_) => false,
        }
    }

    pub fn is_url(&self) -> bool {
        match self {
            OntologyLocation::File(_) => false,
            OntologyLocation::Url(_) => true,
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        if s.starts_with("http") || s.starts_with("<http") {
            Ok(OntologyLocation::Url(s.to_string()))
        } else {
            // remove any leading file://
            let s = s.trim_start_matches("file://");
            let mut p = PathBuf::from(s);
            // make sure p is absolute
            if !p.is_absolute() {
                p = std::env::current_dir()?.join(p);
            }
            Ok(OntologyLocation::File(p))
        }
    }

    pub fn to_iri(&self) -> NamedNode {
        // if it is a file, convert it to a file:// IRI
        match self {
            OntologyLocation::File(p) => {
                let p = p.to_str().unwrap_or_default();
                NamedNode::new(format!("file://{}", p)).unwrap()
            }
            OntologyLocation::Url(u) => NamedNode::new(u.clone()).unwrap(),
        }
    }

    pub fn as_path(&self) -> Option<&PathBuf> {
        match self {
            OntologyLocation::File(p) => Some(p),
            OntologyLocation::Url(_) => None,
        }
    }
}

struct LocalType;

impl SerializeAs<NamedNode> for LocalType {
    fn serialize_as<S>(value: &NamedNode, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        namednode_ser(value, serializer)
    }
}

impl<'de> DeserializeAs<'de, NamedNode> for LocalType {
    fn deserialize_as<D>(deserializer: D) -> Result<NamedNode, D::Error>
    where
        D: Deserializer<'de>,
    {
        namednode_de(deserializer)
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Debug)]
pub struct Ontology {
    id: GraphIdentifier,
    #[serde(serialize_with = "namednode_ser", deserialize_with = "namednode_de")]
    name: NamedNode,
    #[serde_as(as = "Vec<LocalType>")]
    pub imports: Vec<NamedNode>,
    location: Option<OntologyLocation>,
    pub last_updated: Option<DateTime<Utc>>,
    #[serde_as(as = "HashMap<LocalType, _>")]
    version_properties: HashMap<NamedNode, String>,
    namespace_map: HashMap<String, String>,
}

// impl display; name + location + last updated, then indented version properties
impl std::fmt::Display for Ontology {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Ontology: {}\nLocation: {}\nVersion Properties:\n",
            self.name,
            self.id.location.as_str()
        )?;
        for (k, v) in self.version_properties.iter() {
            writeln!(f, "  {}: {}", k, v)?;
        }
        Ok(())
    }
}

// impl default for Ontology
impl Default for Ontology {
    fn default() -> Self {
        Ontology {
            id: GraphIdentifier {
                location: OntologyLocation::File(PathBuf::new()),
                name: NamedNode::new("<n/a>").unwrap(),
            },
            name: NamedNode::new("<n/a>").unwrap(),
            imports: vec![],
            location: None,
            last_updated: None,
            version_properties: HashMap::new(),
            namespace_map: HashMap::new(),
        }
    }
}

impl Ontology {
    pub fn with_location(&mut self, location: OntologyLocation) {
        self.location = Some(location);
    }

    pub fn with_last_updated(&mut self, last_updated: DateTime<Utc>) {
        self.last_updated = Some(last_updated);
    }

    pub fn id(&self) -> &GraphIdentifier {
        &self.id
    }

    pub fn exists(&self) -> bool {
        match &self.location {
            Some(OntologyLocation::File(p)) => p.exists(),
            Some(OntologyLocation::Url(u)) => {
                // check if the URL is reachable
                let res = reqwest::blocking::get(u);
                match res {
                    Ok(r) => r.status().is_success(),
                    Err(_) => false,
                }
            }
            None => false,
        }
    }

    pub fn version_properties(&self) -> &HashMap<NamedNode, String> {
        &self.version_properties
    }

    pub fn location(&self) -> Option<&OntologyLocation> {
        self.location.as_ref()
    }

    pub fn graph(&self) -> Result<OxigraphGraph> {
        if let Some(location) = &self.location {
            return location.graph();
        }
        return OntologyLocation::from_str(self.name.as_str()).and_then(|loc| loc.graph());
    }

    ///// Returns the graph for this ontology from the OntoEnv
    //pub fn graph(&self, env: &OntoEnv) -> Result<LightGraph> {
    //    if let Some(location) = &self.location {
    //        return location.graph();
    //    }
    //    return OntologyLocation::from_str(self.name.as_str()).and_then(|loc| loc.graph());
    //}

    pub fn name(&self) -> NamedNode {
        self.name.clone()
    }

    pub fn dump(&self) -> String {
        serde_json::to_string_pretty(self).unwrap()
    }

    pub fn namespace_map(&self) -> &HashMap<String, String> {
        &self.namespace_map
    }

    pub fn from_graph(
        graph: &OxigraphGraph,
        location: OntologyLocation,
        require_ontology_names: bool,
    ) -> Result<Self> {
        // get the rdf:type owl:Ontology declarations
        let mut decls: Vec<SubjectRef> = graph
            .subjects_for_predicate_object(TYPE, ONTOLOGY)
            .collect::<Vec<_>>();

        // if decls is empty, then find all subjets of sh:declare
        if decls.is_empty() {
            decls.extend(graph.triples_for_predicate(DECLARE).map(|t| t.subject));
        }

        // ontology_name is the subject of the first declaration
        let ontology_name: Subject = match decls.first() {
            Some(decl) => match decl {
                SubjectRef::NamedNode(s) => Subject::NamedNode((*s).into()),
                _ => return Err(anyhow::anyhow!("Ontology name is not an IRI")),
            },
            None => {
                if require_ontology_names {
                    return Err(anyhow::anyhow!(
                        "No ontology declaration found in {}",
                        location
                    ));
                }
                warn!(
                    "No ontology declaration found in {}. Using this as the ontology name",
                    location
                );
                Subject::NamedNode(location.to_iri())
            }
        };
        debug!("got ontology name: {}", ontology_name);

        let mut namespace_map = HashMap::new();

        let declare_prop = NamedNode::new_unchecked("http://www.w3.org/ns/shacl#declare");
        let prefix_prop = NamedNode::new_unchecked("http://www.w3.org/ns/shacl#prefix");
        let namespace_prop = NamedNode::new_unchecked("http://www.w3.org/ns/shacl#namespace");

        for decl_obj_ref in
            graph.objects_for_subject_predicate(ontology_name.as_ref(), declare_prop.as_ref())
        {
            let decl_subj: SubjectRef = match decl_obj_ref {
                TermRef::NamedNode(n) => n.into(),
                TermRef::BlankNode(b) => b.into(),
                _ => continue,
            };

            let prefix_term = graph.object_for_subject_predicate(decl_subj, prefix_prop.as_ref());
            let namespace_term =
                graph.object_for_subject_predicate(decl_subj, namespace_prop.as_ref());

            if let (Some(TermRef::Literal(prefix_lit)), Some(TermRef::Literal(namespace_lit))) =
                (prefix_term, namespace_term)
            {
                namespace_map.insert(
                    prefix_lit.value().to_string(),
                    namespace_lit.value().to_string(),
                );
            }
        }

        let imports: Vec<TermRef> = graph
            .objects_for_subject_predicate(ontology_name.as_ref(), IMPORTS)
            .collect::<Vec<_>>();

        // get each of the ONNTOLOGY_VERSION_IRIS values, if they exist on the ontology
        let mut version_properties: HashMap<NamedNode, String> =
            ONTOLOGY_VERSION_IRIS
                .iter()
                .fold(HashMap::new(), |mut acc, &iri| {
                    if let Some(o) = graph.object_for_subject_predicate(ontology_name.as_ref(), iri)
                    {
                        match o {
                            TermRef::NamedNode(s) => {
                                acc.insert(iri.into(), s.to_string());
                            }
                            TermRef::Literal(lit) => {
                                acc.insert(iri.into(), lit.to_string());
                            }
                            _ => (),
                        }
                    }
                    acc
                });

        // check if any of the ONTOLOGY_VERSION_IRIS exist on the other side of a
        // vaem:hasGraphMetadata predicate
        let graph_metadata: Vec<TermRef> = graph
            .objects_for_subject_predicate(ontology_name.as_ref(), HAS_GRAPH_METADATA)
            .collect::<Vec<_>>();
        for value in graph_metadata {
            let graph_iri = match value {
                TermRef::NamedNode(s) => s,
                _ => continue,
            };
            for iri in ONTOLOGY_VERSION_IRIS.iter() {
                if let Some(value) = graph.object_for_subject_predicate(graph_iri, *iri) {
                    match value {
                        TermRef::NamedNode(s) => {
                            version_properties.insert((*iri).into(), s.to_string());
                        }
                        TermRef::Literal(lit) => {
                            version_properties.insert((*iri).into(), lit.to_string());
                        }
                        _ => (),
                    }
                }
            }
        }
        // dump version properties
        for (k, v) in version_properties.iter() {
            debug!("{}: {}", k, v);
        }

        info!(
            "Fetched graph {} from location: {:?}",
            ontology_name, location
        );

        let ontology_name: NamedNode = match ontology_name {
            Subject::NamedNode(s) => s,
            _ => panic!("Ontology name is not an IRI"),
        };

        let imports: Vec<NamedNode> = imports
            .iter()
            .map(|t| match t {
                TermRef::NamedNode(s) => Ok(NamedNode::new(s.as_str())?),
                _ => panic!("Import is not an IRI"),
            })
            .collect::<Result<Vec<NamedNode>>>()?;

        Ok(Ontology {
            id: GraphIdentifier {
                location: location.clone(),
                name: ontology_name.clone(),
            },
            name: ontology_name,
            imports,
            location: Some(location),
            version_properties,
            last_updated: None,
            namespace_map,
        })
    }

    pub fn from_str(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use oxigraph::model::NamedNode;

    #[test]
    fn test_ontology_location() {
        let url = "http://example.com/ontology.ttl";
        let file = "/tmp/ontology.ttl";
        let url_location = OntologyLocation::from_str(url).unwrap();
        let file_location = OntologyLocation::from_str(file).unwrap();
        assert!(url_location.is_url());
        assert!(!url_location.is_file());
        assert!(!file_location.is_url());
        assert!(file_location.is_file());
    }

    #[test]
    fn test_ontology_location_display() {
        let url = "http://example.com/ontology.ttl";
        let file = "/tmp/ontology.ttl";
        let url_location = OntologyLocation::from_str(url).unwrap();
        let file_location = OntologyLocation::from_str(file).unwrap();
        assert_eq!(url_location.to_string(), url);
        assert_eq!(file_location.to_string(), format!("file://{}", file));
    }

    #[test]
    fn test_ontology_location_to_iri() {
        let url = "http://example.com/ontology.ttl";
        let file = "/tmp/ontology.ttl";
        let url_location = OntologyLocation::from_str(url).unwrap();
        let file_location = OntologyLocation::from_str(file).unwrap();
        assert_eq!(url_location.to_iri(), NamedNode::new(url).unwrap());
        assert_eq!(
            file_location.to_iri(),
            NamedNode::new(format!("file://{}", file)).unwrap()
        );
    }
}
