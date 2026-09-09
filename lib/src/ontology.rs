//! Defines the core data structures for representing ontologies and their metadata within the OntoEnv.
//! Includes `Ontology`, `GraphIdentifier`, and `OntologyLocation`.

use crate::consts::*;
use crate::util::{read_file, read_url};
use anyhow::Result;
use chrono::prelude::*;
use log::{debug, info, warn};
use oxigraph::model::{
    Graph as OxigraphGraph, GraphName, NamedNode, NamedNodeRef, NamedOrBlankNodeRef, TermRef,
    Triple, TripleRef,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{serde_as, DeserializeAs, SerializeAs};
use std::collections::{HashMap, HashSet};
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
        // Default location mirrors the graph IRI for simple in-memory identifiers.
        // location is same as name
        GraphIdentifier {
            location: OntologyLocation::from_str(name.as_str()).unwrap(),
            name: name.into(),
        }
    }
    pub fn new_with_location(name: NamedNodeRef, location: OntologyLocation) -> Self {
        // Use explicit location when the graph IRI differs from its source.
        GraphIdentifier {
            location,
            name: name.into(),
        }
    }
    pub fn location(&self) -> &OntologyLocation {
        // Borrow the location to avoid cloning for simple lookups.
        &self.location
    }

    pub fn name(&self) -> NamedNodeRef<'_> {
        // Return a lightweight reference to the graph name.
        self.name.as_ref()
    }

    pub fn to_filename(&self) -> String {
        // Create a filesystem-safe name for cache/storage paths.
        let name = self.name.as_str().replace(':', "+");
        let location = self.location.as_str().replace("file://", "");
        format!("{name}-{location}").replace('/', "_")
    }

    pub fn graphname(&self) -> Result<GraphName> {
        // Convert identifier to an Oxigraph GraphName for store APIs.
        Ok(GraphName::NamedNode(self.name.clone()))
    }
}

#[derive(Serialize, Deserialize, Hash, Clone, Eq, PartialEq, Debug)]
pub enum OntologyLocation {
    #[serde(rename = "file")]
    File(PathBuf),
    #[serde(rename = "url")]
    Url(String),
    /// Virtual source identifier for ontologies supplied as in-memory bytes.
    ///
    /// This identifier is used for environment bookkeeping only. `owl:imports` resolution still
    /// uses the import IRIs declared in the ontology content and resolves those against permanent
    /// locations (e.g., `http(s)`/`file`) when loaded through OntoEnv APIs.
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
        // Provide a shared string view for logging and comparisons.
        match self {
            OntologyLocation::File(p) => p.to_str().unwrap_or_default(),
            OntologyLocation::Url(u) => u.as_str(),
            OntologyLocation::InMemory { identifier } => identifier.as_str(),
        }
    }

    pub fn graph(&self) -> Result<OxigraphGraph> {
        // Load content from the underlying location when possible.
        match self {
            OntologyLocation::File(p) => read_file(p),
            OntologyLocation::Url(u) => read_url(u),
            OntologyLocation::InMemory { .. } => Err(anyhow::anyhow!(
                "In-memory ontology locations cannot be refreshed from an external source"
            )),
        }
    }

    pub fn is_file(&self) -> bool {
        // Simple predicate used in filters and branching logic.
        match self {
            OntologyLocation::File(_) => true,
            OntologyLocation::Url(_) => false,
            OntologyLocation::InMemory { .. } => false,
        }
    }

    pub fn is_url(&self) -> bool {
        // Simple predicate used in filters and branching logic.
        match self {
            OntologyLocation::File(_) => false,
            OntologyLocation::Url(_) => true,
            OntologyLocation::InMemory { .. } => false,
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self> {
        // Accept both IRIs and file paths, normalizing to absolute paths.
        let trimmed = s.trim();
        let value = if trimmed.starts_with('<') && trimmed.ends_with('>') && trimmed.len() >= 2 {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        };

        if value.starts_with("http://") || value.starts_with("https://") {
            return Ok(OntologyLocation::Url(value.to_string()));
        }

        if value.starts_with("file://") {
            let url = Url::parse(value)?;
            let path = match url.to_file_path() {
                Ok(path) => path,
                Err(()) => {
                    // Compatibility fallback for platform-dependent file URL handling
                    // (e.g., `file:///dummy.ttl` on Windows).
                    let mut p = PathBuf::from(value.trim_start_matches("file://"));
                    if !p.is_absolute() {
                        p = std::env::current_dir()?.join(p);
                    }
                    p
                }
            };
            return Ok(OntologyLocation::File(Self::normalized_file_path(&path)));
        }

        let mut p = PathBuf::from(value);
        if !p.is_absolute() {
            p = std::env::current_dir()?.join(p);
        }
        Ok(OntologyLocation::File(Self::normalized_file_path(&p)))
    }

    pub fn to_iri(&self) -> NamedNode {
        // Convert location to a canonical IRI for graph identifiers.
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
        // Expose file paths only when the location is filesystem-based.
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

/// Canonicalize a filesystem path so that two references to the same file compare
/// equal regardless of how the caller obtained the path (relative vs absolute,
/// symlinked vs resolved, `\??\` UNC prefix on Windows, etc.).
///
/// Uses [`dunce::canonicalize`] when the path exists (resolving symlinks, `.`,
/// `..`, and making the path absolute), falling back to a lexical normalization
/// (join to the current directory and simplify) when the path does not yet
/// exist or cannot be resolved. This keeps locations comparable across
/// `init`/`update` discovery and explicit `add`/`add_from_bytes` calls.
pub fn canonicalize_file_path(path: &Path) -> PathBuf {
    use std::path::Component;

    if path.as_os_str().is_empty() {
        return dunce::canonicalize(".").unwrap_or_else(|_| PathBuf::from("."));
    }

    if let Ok(canonical) = dunce::canonicalize(path) {
        return canonical;
    }

    // Lexical fallback for not-yet-existing or inaccessible paths: anchor to the
    // current directory and collapse `.`/`..` components without touching the
    // filesystem. Symlinks cannot be resolved here, but this at least makes
    // relative and `.`/`..`-laden paths comparable.
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };

    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop if the last component is a normal name; otherwise keep
                // the `..` (e.g. at a root boundary).
                match out.components().next_back() {
                    Some(Component::Normal(_)) => {
                        out.pop();
                    }
                    _ => out.push(".."),
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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
    use tempfile::TempDir;

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

    #[test]
    fn file_url_from_str_round_trips_to_path() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("import.ttl");
        std::fs::write(&path, b"").unwrap();
        let url = Url::from_file_path(&path).unwrap().to_string();

        let parsed = OntologyLocation::from_str(&url).unwrap();
        match parsed {
            OntologyLocation::File(parsed_path) => {
                assert_eq!(
                    OntologyLocation::normalized_file_path(&parsed_path),
                    OntologyLocation::normalized_file_path(&path)
                );
            }
            other => panic!("Expected file location, got {other:?}"),
        }
    }

    #[test]
    fn file_url_root_only_path_is_accepted() {
        let parsed = OntologyLocation::from_str("file:///dummy.ttl").unwrap();
        assert!(matches!(parsed, OntologyLocation::File(_)));
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
    #[serde(default)]
    content_hash: Option<String>,
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
            content_hash: None,
        }
    }
}

/// Render a version-property object the same way the store-backed metadata
/// extraction does: IRIs and literals in their N-Triples form, nothing else.
fn version_property_value(object: TermRef<'_>) -> Option<String> {
    match object {
        TermRef::NamedNode(n) => Some(n.to_string()),
        TermRef::Literal(lit) => Some(lit.to_string()),
        _ => None,
    }
}

impl Ontology {
    pub fn with_last_updated(&mut self, last_updated: DateTime<Utc>) {
        // Update timestamp after a successful refresh.
        self.last_updated = Some(last_updated);
    }

    /// Update the ontology's location (and associated identifier) in one step.
    /// Keeps the name stable while swapping in the new location.
    pub fn set_location(&mut self, location: OntologyLocation) {
        // Keep id/location consistent since both are persisted and indexed.
        self.id = GraphIdentifier::new_with_location(self.id.name(), location.clone());
        self.location = Some(location);
    }

    /// Update the ontology's declared IRI (graph name) while keeping the source location.
    pub fn set_iri(&mut self, new_iri: NamedNode) {
        let location = self.id.location().clone();
        self.id = GraphIdentifier::new_with_location(new_iri.as_ref(), location);
        self.name = new_iri;
    }

    pub fn set_content_hash(&mut self, hash: String) {
        // Record content hash for change detection without re-parsing.
        self.content_hash = Some(hash);
    }

    pub fn content_hash(&self) -> Option<&str> {
        // Return borrowed hash string for lightweight comparisons.
        self.content_hash.as_deref()
    }

    pub fn id(&self) -> &GraphIdentifier {
        // Return a reference to the stable identifier.
        &self.id
    }

    pub fn exists(&self) -> bool {
        // Check presence based on location type without loading the graph.
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
        // Expose collected version metadata for resolution policies.
        &self.version_properties
    }

    pub fn location(&self) -> Option<&OntologyLocation> {
        // Borrow location to avoid cloning in common read paths.
        self.location.as_ref()
    }

    pub fn graph(&self) -> Result<OxigraphGraph> {
        // Load graph from the best available source (explicit location or name).
        if let Some(location) = &self.location {
            return location.graph();
        }
        OntologyLocation::from_str(self.name.as_str()).and_then(|loc| loc.graph())
    }

    pub fn name(&self) -> NamedNode {
        // Clone to preserve internal ownership semantics.
        self.name.clone()
    }

    pub fn dump(&self) -> String {
        // Serialize for debugging or exports.
        serde_json::to_string_pretty(self).unwrap()
    }

    pub fn namespace_map(&self) -> &HashMap<String, String> {
        // Share the cached prefix map for CLI and diagnostics.
        &self.namespace_map
    }

    /// Creates an `Ontology` from a materialized graph. See [`Self::from_triples`].
    pub fn from_graph(
        graph: &OxigraphGraph,
        location: OntologyLocation,
        require_ontology_names: bool,
    ) -> Result<Self> {
        let triples: Vec<Triple> = graph.iter().map(TripleRef::into_owned).collect();
        Self::from_triples(&triples, location, require_ontology_names)
    }

    /// Creates an `Ontology` from a parsed triple list.
    ///
    /// A handful of linear scans extract the ontology declaration, imports,
    /// version properties, and SHACL prefix declarations, so no store or
    /// index has to be built just to read metadata.
    pub fn from_triples(
        triples: &[Triple],
        location: OntologyLocation,
        require_ontology_names: bool,
    ) -> Result<Self> {
        // Prefer explicit rdf:type owl:Ontology declarations, then fall back
        // to sh:declare subjects, mirroring `from_store`.
        let mut decls: Vec<NamedOrBlankNodeRef<'_>> = triples
            .iter()
            .filter(|t| {
                t.predicate.as_ref() == TYPE && t.object.as_ref() == TermRef::NamedNode(ONTOLOGY)
            })
            .map(|t| t.subject.as_ref())
            .collect();
        if decls.is_empty() {
            decls.extend(
                triples
                    .iter()
                    .filter(|t| t.predicate.as_ref() == DECLARE)
                    .map(|t| t.subject.as_ref()),
            );
        }
        if decls.len() > 1 {
            warn!("Multiple ontology declarations found in {location}, using first one");
        }

        let ontology_name = match decls.first() {
            None => {
                if require_ontology_names {
                    return Err(anyhow::anyhow!(
                        "No ontology declaration found in {}",
                        location
                    ));
                }
                warn!(
                    "No ontology declaration found in {location}. Using this as the ontology name"
                );
                location.to_iri()
            }
            Some(NamedOrBlankNodeRef::NamedNode(name)) => name.into_owned(),
            Some(_) => {
                // Blank nodes are not stable ontology identifiers; fail in strict mode.
                return Err(anyhow::anyhow!(
                    "Ontology declaration subject is not a NamedNode, skipping."
                ));
            }
        };
        Self::build_from_subject_in_triples(triples, ontology_name, location)
    }

    fn build_from_subject_in_triples(
        triples: &[Triple],
        ontology_name: NamedNode,
        location: OntologyLocation,
    ) -> Result<Self> {
        debug!("got ontology name: {ontology_name}");
        let subject_ref = NamedOrBlankNodeRef::NamedNode(ontology_name.as_ref());

        let mut imports: Vec<NamedNode> = Vec::new();
        let mut declare_nodes: HashSet<NamedOrBlankNodeRef<'_>> = HashSet::new();
        let mut metadata_nodes: HashSet<NamedNodeRef<'_>> = HashSet::new();
        let mut version_properties: HashMap<NamedNode, String> = HashMap::new();

        // Pass 1: everything hanging directly off the ontology subject.
        for triple in triples.iter().filter(|t| t.subject.as_ref() == subject_ref) {
            let predicate = triple.predicate.as_ref();
            let object = triple.object.as_ref();
            if predicate == IMPORTS {
                match object {
                    TermRef::NamedNode(import) if import != ontology_name.as_ref() => {
                        imports.push(import.into_owned());
                    }
                    TermRef::NamedNode(_) => {}
                    other => warn!("Ignoring non-IRI owl:imports value {other} in {location}"),
                }
            } else if predicate == DECLARE {
                match object {
                    TermRef::NamedNode(n) => {
                        declare_nodes.insert(n.into());
                    }
                    TermRef::BlankNode(b) => {
                        declare_nodes.insert(b.into());
                    }
                    _ => {}
                }
            } else if predicate == HAS_GRAPH_METADATA {
                if let TermRef::NamedNode(n) = object {
                    metadata_nodes.insert(n);
                }
            } else if let Some(iri) = ONTOLOGY_VERSION_IRIS.iter().find(|iri| **iri == predicate) {
                if let Some(value) = version_property_value(object) {
                    version_properties.entry((*iri).into()).or_insert(value);
                }
            }
        }

        // Pass 2: SHACL prefix declarations and linked graph-metadata nodes.
        let mut prefixes: HashMap<NamedOrBlankNodeRef<'_>, String> = HashMap::new();
        let mut namespaces: HashMap<NamedOrBlankNodeRef<'_>, String> = HashMap::new();
        if !declare_nodes.is_empty() || !metadata_nodes.is_empty() {
            for triple in triples {
                let subject = triple.subject.as_ref();
                let predicate = triple.predicate.as_ref();
                let object = triple.object.as_ref();
                if declare_nodes.contains(&subject) {
                    if let TermRef::Literal(lit) = object {
                        if predicate == SH_PREFIX {
                            prefixes
                                .entry(subject)
                                .or_insert_with(|| lit.value().to_string());
                        } else if predicate == SH_NAMESPACE {
                            namespaces
                                .entry(subject)
                                .or_insert_with(|| lit.value().to_string());
                        }
                    }
                }
                if let NamedOrBlankNodeRef::NamedNode(n) = subject {
                    if metadata_nodes.contains(&n) {
                        if let Some(iri) =
                            ONTOLOGY_VERSION_IRIS.iter().find(|iri| **iri == predicate)
                        {
                            if let Some(value) = version_property_value(object) {
                                // Graph metadata values take precedence over the
                                // values declared directly on the ontology subject.
                                version_properties.insert((*iri).into(), value);
                            }
                        }
                    }
                }
            }
        }
        let namespace_map: HashMap<String, String> = prefixes
            .into_iter()
            .filter_map(|(node, prefix)| Some((prefix, namespaces.remove(&node)?)))
            .collect();

        for (k, v) in version_properties.iter() {
            debug!("{k}: {v}");
        }
        info!("Fetched graph {ontology_name} from location: {location:?}");

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
            content_hash: None,
        })
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Result<Self> {
        // Convenience for reading persisted ontology JSON.
        Ok(serde_json::from_str(s)?)
    }
}
