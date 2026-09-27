//! Versioned RDF5D metadata catalog for persistent OntoEnv environments.
//!
//! The catalog is the authoritative environment metadata snapshot. Ontology
//! graphs remain authoritative for RDF content, but are never inspected while
//! opening a synchronized environment.

use crate::environment::Environment;
use anyhow::{anyhow, Context, Result};
use oxigraph::model::Term as OxTerm;
use rdf5d::{
    reader::R5tuFile,
    writer::{write_file_with_options, Quint, Term, WriterOptions},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const CATALOG_SCHEMA_VERSION: u32 = 1;
pub const CATALOG_FILE: &str = "catalog.r5tu";
pub const PENDING_FILE: &str = "catalog.pending";

const NS: &str = "https://ontoenv.org/catalog/v1#";
const CATALOG_IRI: &str = "urn:ontoenv:catalog";
const CATALOG_GRAPH: &str = "urn:ontoenv:catalog:graph";
const PAYLOAD: &str = "https://ontoenv.org/catalog/v1#environmentPayload";
const SCHEMA_VERSION: &str = "https://ontoenv.org/catalog/v1#schemaVersion";

/// Optional O(1) identity and revision supplied by a graph backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackendState {
    pub id: String,
    pub revision: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CatalogSnapshot {
    schema_version: u32,
    writer_version: String,
    backend: Option<BackendState>,
    #[serde(default)]
    graph_revisions: HashMap<String, String>,
    environment: Environment,
}

fn iri(value: impl Into<String>) -> Term {
    Term::Iri(value.into())
}

fn literal(value: impl Into<String>) -> Term {
    Term::Literal {
        lex: value.into(),
        dt: None,
        lang: None,
    }
}

fn quint(subject: Term, predicate: &str, object: Term) -> Quint {
    Quint {
        id: CATALOG_IRI.to_string(),
        s: subject,
        p: iri(format!("{NS}{predicate}")),
        o: object,
        gname: CATALOG_GRAPH.to_string(),
    }
}

fn location_kind(location: &crate::ontology::OntologyLocation) -> &'static str {
    match location {
        crate::ontology::OntologyLocation::File(_) => "file",
        crate::ontology::OntologyLocation::Url(_) => "url",
        crate::ontology::OntologyLocation::InMemory { .. } => "in-memory",
    }
}

/// Deterministic record IRI for a canonical graph IRI and normalized location.
pub fn record_iri(canonical_iri: &str, normalized_location: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(canonical_iri.as_bytes());
    hasher.update(&[0]);
    hasher.update(normalized_location.as_bytes());
    format!("urn:ontoenv:record:{}", hasher.finalize().to_hex())
}

/// Atomically write a complete catalog snapshot.
pub fn save(
    path: &Path,
    environment: &Environment,
    backend: Option<BackendState>,
    graph_revisions: HashMap<String, String>,
) -> Result<()> {
    let snapshot = CatalogSnapshot {
        schema_version: CATALOG_SCHEMA_VERSION,
        writer_version: env!("CARGO_PKG_VERSION").to_string(),
        backend,
        graph_revisions,
        environment: environment.clone(),
    };
    let payload = serde_json::to_string(&snapshot)?;
    let catalog = iri(CATALOG_IRI);
    let mut quints = vec![
        quint(
            catalog.clone(),
            "schemaVersion",
            literal(CATALOG_SCHEMA_VERSION.to_string()),
        ),
        quint(
            catalog.clone(),
            "writerVersion",
            literal(env!("CARGO_PKG_VERSION")),
        ),
        quint(catalog.clone(), "environmentPayload", literal(payload)),
    ];
    if let Some(state) = &snapshot.backend {
        quints.push(quint(catalog.clone(), "backendId", literal(&state.id)));
        quints.push(quint(
            catalog.clone(),
            "backendRevision",
            literal(&state.revision),
        ));
    }

    let mut ontologies: Vec<_> = environment.ontologies().values().collect();
    ontologies.sort_by_key(|ontology| {
        (
            ontology.id().name().as_str().to_string(),
            ontology.id().location().to_string(),
        )
    });
    for ontology in ontologies {
        let location = ontology.id().location();
        let record = iri(record_iri(
            ontology.id().name().as_str(),
            &location.to_string(),
        ));
        quints.push(quint(
            record.clone(),
            "type",
            iri(format!("{NS}OntologyRecord")),
        ));
        quints.push(quint(
            record.clone(),
            "canonicalIri",
            iri(ontology.id().name().as_str()),
        ));
        quints.push(quint(
            record.clone(),
            "backendGraphIri",
            iri(ontology.id().name().as_str()),
        ));
        quints.push(quint(
            record.clone(),
            "declaredName",
            iri(ontology.name().as_str()),
        ));
        quints.push(quint(
            record.clone(),
            "sourceKind",
            literal(location_kind(location)),
        ));
        quints.push(quint(
            record.clone(),
            "sourceLocation",
            literal(location.to_string()),
        ));
        for import in &ontology.imports {
            quints.push(quint(record.clone(), "imports", iri(import.as_str())));
        }
        for alias in environment.get_aliases_for(ontology.id().name().as_str()) {
            quints.push(quint(record.clone(), "alias", iri(alias)));
        }
        if let Some(hash) = ontology.content_hash() {
            quints.push(quint(record.clone(), "contentHash", literal(hash)));
        }
        if let Some(updated) = ontology.last_updated {
            quints.push(quint(
                record.clone(),
                "lastUpdated",
                literal(updated.to_rfc3339()),
            ));
        }
        if let Some(revision) = snapshot.graph_revisions.get(ontology.id().name().as_str()) {
            quints.push(quint(record.clone(), "backendRevision", literal(revision)));
        }
        for (property, value) in ontology.version_properties() {
            quints.push(quint(
                record.clone(),
                "versionProperty",
                literal(serde_json::to_string(&(property.as_str(), value))?),
            ));
        }
        for (prefix, namespace) in ontology.namespace_map() {
            quints.push(quint(
                record.clone(),
                "namespace",
                literal(serde_json::to_string(&(prefix, namespace))?),
            ));
        }
    }

    write_file_with_options(
        path,
        &quints,
        WriterOptions {
            zstd: false,
            with_crc: true,
        },
    )
    .map_err(|error| anyhow!("failed to write RDF5D catalog: {error}"))
}

/// Load and validate a catalog without reading any ontology graph.
pub fn load(path: &Path) -> Result<(Environment, Option<BackendState>, HashMap<String, String>)> {
    let file = R5tuFile::open(path)
        .map_err(|error| anyhow!("failed to open RDF5D catalog {}: {error}", path.display()))?;
    let mut payload = None;
    let mut encoded_schema = None;
    for graph in file.enumerate_all()? {
        for triple in file.oxigraph_triples(graph.gid)? {
            let triple = triple?;
            if let OxTerm::Literal(value) = triple.object {
                match triple.predicate.as_str() {
                    PAYLOAD => payload = Some(value.value().to_string()),
                    SCHEMA_VERSION => encoded_schema = Some(value.value().to_string()),
                    _ => {}
                }
            }
        }
    }
    let expected_schema = CATALOG_SCHEMA_VERSION.to_string();
    if encoded_schema.as_deref() != Some(expected_schema.as_str()) {
        return Err(anyhow!(
            "catalog has a missing or unsupported RDF schemaVersion"
        ));
    }
    let payload = payload.ok_or_else(|| anyhow!("catalog is missing its environment payload"))?;
    let snapshot: CatalogSnapshot =
        serde_json::from_str(&payload).context("invalid catalog environment payload")?;
    if snapshot.schema_version != CATALOG_SCHEMA_VERSION {
        return Err(anyhow!(
            "unsupported OntoEnv catalog schema version {}; this release supports {}",
            snapshot.schema_version,
            CATALOG_SCHEMA_VERSION
        ));
    }
    Ok((
        snapshot.environment,
        snapshot.backend,
        snapshot.graph_revisions,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_iris_are_stable_and_location_sensitive() {
        let first = record_iri("https://example.org/o", "file:///tmp/o.ttl");
        assert_eq!(
            first,
            record_iri("https://example.org/o", "file:///tmp/o.ttl")
        );
        assert_ne!(
            first,
            record_iri("https://example.org/o", "file:///tmp/other.ttl")
        );
    }

    #[test]
    fn empty_catalog_round_trips_as_rdf5d() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join(CATALOG_FILE);
        let state = BackendState {
            id: "test-store".to_string(),
            revision: "7".to_string(),
        };
        save(
            &path,
            &Environment::new(),
            Some(state.clone()),
            HashMap::new(),
        )?;
        assert_eq!(&std::fs::read(&path)?[..4], b"R5TU");
        let (environment, loaded_state, revisions) = load(&path)?;
        assert!(environment.ontologies().is_empty());
        assert_eq!(loaded_state, Some(state));
        assert!(revisions.is_empty());
        Ok(())
    }
}
