//! Defines the core data structures for representing ontologies and their metadata within the OntoEnv.
//! Includes `Ontology`, `GraphIdentifier`, and `OntologyLocation`.

use crate::consts::*;
use crate::util::{read_file, read_url};
use anyhow::Result;
use chrono::prelude::*;
use log::{debug, info, warn};
use oxigraph::model::{
    Graph as OxigraphGraph, GraphName, GraphNameRef, NamedNode, NamedNodeRef, NamedOrBlankNode,
    NamedOrBlankNodeRef, Term,
};
use oxigraph::store::Store;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, DeserializeAs, SerializeAs};
use std::collections::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use url::Url;
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

impl From<GraphIdentifier> for NamedNode {
    fn from(val: GraphIdentifier) -> Self {
        val.name
    }
}

impl<'a> From<&'a GraphIdentifier> for NamedNodeRef<'a> {
    fn from(val: &'a GraphIdentifier) -> Self {
        (&val.name).into()
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
    pub fn new_with_location(name: NamedNodeRef, location: OntologyLocation) -> Self {
        GraphIdentifier {
            location,
            name: name.into(),
        }
    }
    pub fn location(&self) -> &OntologyLocation {
        &self.location
    }

    pub fn name(&self) -> NamedNodeRef<'_> {
        self.name.as_ref()
    }

    pub fn to_filename(&self) -> String {
        let name = self.name.as_str().replace(':', "+");
        let location = self.location.as_str().replace("file://", "");
        format!("{name}-{location}").replace('/', "_")
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
    #[serde(rename = "in-memory")]
    InMemory { identifier: String },
}

// impl display for OntologyLocation
impl std::fmt::Display for OntologyLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OntologyLocation::Url(url) => write!(f, "{}", url),
            OntologyLocation::File(path) => {
                let effective_path = Self::normalized_file_path(path);
                if let Some(url) = Self::file_url_for(&effective_path) {
                    write!(f, "{}", url)
                } else {
                    write!(f, "{}", effective_path.display())
                }
            }
            OntologyLocation::InMemory { identifier } => {
                write!(f, "in-memory:{}", identifier)
            }
        }
    }
}

// impl default for OntologyLocation
impl Default for OntologyLocation {
    fn default() -> Self {
        OntologyLocation::File(Self::normalized_file_path(Path::new("")))
    }
}

impl OntologyLocation {
    pub fn as_str(&self) -> &str {
        match self {
            OntologyLocation::File(p) => p.to_str().unwrap_or_default(),
            OntologyLocation::Url(u) => u.as_str(),
            OntologyLocation::InMemory { identifier } => identifier.as_str(),
        }
    }

    pub fn graph(&self) -> Result<OxigraphGraph> {
        match self {
            OntologyLocation::File(p) => read_file(p),
            OntologyLocation::Url(u) => read_url(u),
            OntologyLocation::InMemory { .. } => Err(anyhow::anyhow!(
                "In-memory ontology locations cannot be refreshed from an external source"
            )),
        }
    }

    pub fn is_file(&self) -> bool {
        match self {
            OntologyLocation::File(_) => true,
            OntologyLocation::Url(_) => false,
            OntologyLocation::InMemory { .. } => false,
        }
    }

    pub fn is_url(&self) -> bool {
        match self {
            OntologyLocation::File(_) => false,
            OntologyLocation::Url(_) => true,
            OntologyLocation::InMemory { .. } => false,
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
        match self {
            OntologyLocation::File(p) => {
                let effective_path = Self::normalized_file_path(p);
                if let Some(url) = Self::file_url_for(&effective_path) {
                    let iri: String = url.into();
                    return NamedNode::new(iri.clone())
                        .unwrap_or_else(|_| NamedNode::new_unchecked(iri));
                }

                let fallback_iri = format!("file://{}", effective_path.display());
                NamedNode::new(fallback_iri.clone())
                    .unwrap_or_else(|_| NamedNode::new_unchecked(fallback_iri))
            }
            OntologyLocation::Url(u) => {
                // Strip angle brackets if present (e.g., "<http://...>")
                let iri = if u.starts_with('<') && u.ends_with('>') && u.len() >= 2 {
                    u[1..u.len() - 1].to_string()
                } else {
                    u.clone()
                };
                NamedNode::new(iri).unwrap()
            }
            OntologyLocation::InMemory { identifier } => NamedNode::new(identifier.clone())
                .unwrap_or_else(|_| NamedNode::new_unchecked(identifier.clone())),
        }
    }

    pub fn as_path(&self) -> Option<&PathBuf> {
        match self {
            OntologyLocation::File(p) => Some(p),
            OntologyLocation::Url(_) => None,
            OntologyLocation::InMemory { .. } => None,
        }
    }

    fn normalized_file_path(path: &Path) -> PathBuf {
        if path.as_os_str().is_empty() {
            return std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        }

        if path.is_relative() {
            let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            return base.join(path);
        }

        path.to_path_buf()
    }

    fn file_url_for(path: &Path) -> Option<Url> {
        Url::from_file_path(path)
            .ok()
            .or_else(|| Url::from_directory_path(path).ok())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_location_matches_path(display: &str, iri: &NamedNode, expected: &Path) {
        if let Some(url) = OntologyLocation::file_url_for(expected) {
            let expected_url: String = url.into();
            assert_eq!(display, expected_url, "display should equal file URL");
            assert_eq!(iri.as_str(), expected_url, "iri should equal file URL");
        } else {
            let expected_str = expected.to_string_lossy().into_owned();
            assert!(
                display.contains(&expected_str),
                "display should contain normalized path"
            );
            assert!(
                iri.as_str().contains(&expected_str),
                "iri should contain normalized path"
            );
        }
    }

    #[test]
    fn file_location_with_empty_path_uses_current_dir() {
        let cwd = std::env::current_dir().unwrap();
        let expected = OntologyLocation::normalized_file_path(Path::new(""));
        assert_eq!(expected, cwd);
        let location = OntologyLocation::File(PathBuf::new());

        let display = location.to_string();
        let iri = location.to_iri();

        assert!(!display.is_empty());
        assert_location_matches_path(&display, &iri, &expected);
    }

    #[test]
    fn file_location_normalizes_relative_paths() {
        let relative = PathBuf::from("some/relative/path");
        let location = OntologyLocation::File(relative.clone());

        let expected = OntologyLocation::normalized_file_path(&relative);
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(expected, cwd.join(&relative));
        let display = location.to_string();
        let iri = location.to_iri();

        assert_location_matches_path(&display, &iri, &expected);
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
    #[serde(default)]
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
            writeln!(f, "  {k}: {v}")?;
        }
        Ok(())
    }
}

// impl default for Ontology
impl Default for Ontology {
    fn default() -> Self {
        Ontology {
            id: GraphIdentifier {
                location: OntologyLocation::default(),
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
                let opts = crate::fetch::FetchOptions::default();
                crate::fetch::head_exists(u, &opts).unwrap_or(false)
            }
            Some(OntologyLocation::InMemory { .. }) => false,
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
        OntologyLocation::from_str(self.name.as_str()).and_then(|loc| loc.graph())
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

    fn build_from_subject_in_store(
        store: &Store,
        graph_name: GraphNameRef,
        ontology_subject: NamedOrBlankNode,
        location: OntologyLocation,
    ) -> Result<Self> {
        debug!("got ontology name: {ontology_subject}");

        let mut namespace_map = HashMap::new();

        let declare_prop = NamedNode::new_unchecked("http://www.w3.org/ns/shacl#declare");
        let prefix_prop = NamedNode::new_unchecked("http://www.w3.org/ns/shacl#prefix");
        let namespace_prop = NamedNode::new_unchecked("http://www.w3.org/ns/shacl#namespace");

        let ontology_subject_ref = ontology_subject.as_ref();

        for decl_obj in store
            .quads_for_pattern(
                Some(ontology_subject_ref),
                Some(declare_prop.as_ref()),
                None,
                Some(graph_name),
            )
            .filter_map(Result::ok)
            .map(|q| q.object)
        {
            let decl_subj = match &decl_obj {
                Term::NamedNode(n) => NamedOrBlankNode::NamedNode(n.clone()),
                Term::BlankNode(b) => NamedOrBlankNode::BlankNode(b.clone()),
                _ => continue,
            };

            let prefix_term = store
                .quads_for_pattern(
                    Some(decl_subj.as_ref()),
                    Some(prefix_prop.as_ref()),
                    None,
                    Some(graph_name),
                )
                .filter_map(Result::ok)
                .map(|q| q.object)
                .next();
            let namespace_term = store
                .quads_for_pattern(
                    Some(decl_subj.as_ref()),
                    Some(namespace_prop.as_ref()),
                    None,
                    Some(graph_name),
                )
                .filter_map(Result::ok)
                .map(|q| q.object)
                .next();

            if let (Some(Term::Literal(prefix_lit)), Some(Term::Literal(namespace_lit))) =
                (prefix_term, namespace_term)
            {
                namespace_map.insert(
                    prefix_lit.value().to_string(),
                    namespace_lit.value().to_string(),
                );
            }
        }

        let imports: Vec<Term> = store
            .quads_for_pattern(
                Some(ontology_subject_ref),
                Some(IMPORTS),
                None,
                Some(graph_name),
            )
            .filter_map(Result::ok)
            .map(|q| q.object)
            .collect::<Vec<_>>();

        // get each of the ONNTOLOGY_VERSION_IRIS values, if they exist on the ontology
        let mut version_properties: HashMap<NamedNode, String> =
            ONTOLOGY_VERSION_IRIS
                .iter()
                .fold(HashMap::new(), |mut acc, &iri| {
                    if let Some(o) = store
                        .quads_for_pattern(
                            Some(ontology_subject_ref),
                            Some(iri),
                            None,
                            Some(graph_name),
                        )
                        .filter_map(Result::ok)
                        .map(|q| q.object)
                        .next()
                    {
                        match o {
                            Term::NamedNode(s) => {
                                acc.insert(iri.into(), s.to_string());
                            }
                            Term::Literal(lit) => {
                                acc.insert(iri.into(), lit.to_string());
                            }
                            _ => (),
                        }
                    }
                    acc
                });

        // check if any of the ONTOLOGY_VERSION_IRIS exist on the other side of a
        // vaem:hasGraphMetadata predicate
        let graph_metadata: Vec<Term> = store
            .quads_for_pattern(
                Some(ontology_subject_ref),
                Some(HAS_GRAPH_METADATA),
                None,
                Some(graph_name),
            )
            .filter_map(Result::ok)
            .map(|q| q.object)
            .collect::<Vec<_>>();
        for value in graph_metadata {
            let graph_iri = match value {
                Term::NamedNode(s) => s,
                _ => continue,
            };
            for iri in ONTOLOGY_VERSION_IRIS.iter() {
                if let Some(value) = store
                    .quads_for_pattern(
                        Some(NamedOrBlankNodeRef::NamedNode(graph_iri.as_ref())),
                        Some(*iri),
                        None,
                        Some(graph_name),
                    )
                    .filter_map(Result::ok)
                    .map(|q| q.object)
                    .next()
                {
                    match value {
                        Term::NamedNode(s) => {
                            version_properties.insert((*iri).into(), s.to_string());
                        }
                        Term::Literal(lit) => {
                            version_properties.insert((*iri).into(), lit.to_string());
                        }
                        _ => (),
                    }
                }
            }
        }
        // dump version properties
        for (k, v) in version_properties.iter() {
            debug!("{k}: {v}");
        }

        info!("Fetched graph {ontology_subject} from location: {location:?}");

        let ontology_name: NamedNode = match ontology_subject {
            NamedOrBlankNode::NamedNode(s) => s,
            _ => panic!("Ontology name is not an IRI"),
        };

        let imports: Vec<NamedNode> = imports
            .iter()
            .map(|t| match t {
                Term::NamedNode(s) => s,
                _ => panic!("Import is not an IRI"),
            })
            .filter(|s| **s != ontology_name)
            .cloned()
            .collect();

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

    /// Creates an `Ontology` from a graph in a `Store`.
    pub fn from_store(
        store: &Store,
        id: &GraphIdentifier,
        require_ontology_names: bool,
    ) -> Result<Self> {
        let graph_name = id.graphname()?;
        let graph_name_ref = graph_name.as_ref();
        let location = id.location().clone();

        // get the rdf:type owl:Ontology declarations
        let mut decls: Vec<NamedOrBlankNode> = store
            .quads_for_pattern(
                None,
                Some(TYPE),
                Some(ONTOLOGY.into()),
                Some(graph_name_ref),
            )
            .filter_map(Result::ok)
            .map(|q| q.subject)
            .collect::<Vec<_>>();

        // if decls is empty, then find all subjects of sh:declare
        if decls.is_empty() {
            decls.extend(
                store
                    .quads_for_pattern(None, Some(DECLARE), None, Some(graph_name_ref))
                    .filter_map(Result::ok)
                    .map(|t| t.subject),
            );
        }

        if decls.len() > 1 {
            warn!("Multiple ontology declarations found in {location}, using first one");
        }

        if decls.is_empty() {
            if require_ontology_names {
                return Err(anyhow::anyhow!(
                    "No ontology declaration found in {}",
                    location
                ));
            }
            warn!("No ontology declaration found in {location}. Using this as the ontology name");
            let ontology_subject = NamedOrBlankNode::NamedNode(location.to_iri());
            Self::build_from_subject_in_store(store, graph_name_ref, ontology_subject, location)
        } else {
            let decl = decls.into_iter().next().unwrap();
            let ontology_subject = match decl {
                NamedOrBlankNode::NamedNode(s) => NamedOrBlankNode::NamedNode(s),
                _ => {
                    return Err(anyhow::anyhow!(
                        "Ontology declaration subject is not a NamedNode, skipping."
                    ));
                }
            };
            Self::build_from_subject_in_store(store, graph_name_ref, ontology_subject, location)
        }
    }

    pub fn from_str(s: &str) -> Result<Self> {
        Ok(serde_json::from_str(s)?)
    }
}
