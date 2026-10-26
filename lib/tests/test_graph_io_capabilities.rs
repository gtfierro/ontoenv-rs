//! Raw Oxigraph access must represent the backend, not a scratch or lazy cache.
use anyhow::{bail, Result};
use ontoenv::api::{ConnectSync, OntoEnv};
use ontoenv::config::Config;
use ontoenv::errors::StoreCapabilityError;
use ontoenv::io::{
    ExternalStoreGraphIO, GraphIO, MemoryGraphIO, PersistentGraphIO, ReadOnlyPersistentGraphIO,
};
use ontoenv::ontology::{GraphIdentifier, Ontology, OntologyLocation};
use ontoenv::options::{Overwrite, RefreshStrategy};
use oxigraph::io::RdfFormat;
use oxigraph::model::{Graph, Literal, NamedNode, Quad, Triple};
use oxigraph::store::Store;
use std::path::Path;

fn source(name: &str) -> OntologyLocation {
    OntologyLocation::InMemory {
        identifier: name.into(),
    }
}

fn ttl(name: &str, value: &str) -> Vec<u8> {
    format!("<{name}> a <http://www.w3.org/2002/07/owl#Ontology>; <urn:value> \"{value}\" .")
        .into_bytes()
}

fn capability_error<T>(result: Result<T>) {
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("backend exposed an unsupported capability"),
    };
    assert!(
        error.downcast_ref::<StoreCapabilityError>().is_some(),
        "{error:#}"
    );
}

#[test]
fn managed_reads_and_publication_work_without_raw_store_access() -> Result<()> {
    let root = tempfile::tempdir()?;
    let config = Config::builder()
        .root(root.path().to_path_buf())
        .temporary(false)
        .offline(true)
        .strict(true)
        .locations(vec![])
        .build()?;
    let mut writer = OntoEnv::connect(config, ConnectSync::Auto, false)?;
    capability_error(writer.io().oxigraph_store());
    let mut ids = Vec::new();
    for name in ["urn:first", "urn:second"] {
        ids.push(writer.add_from_bytes(
            source(name),
            ttl(name, "same"),
            Some(RdfFormat::Turtle),
            Overwrite::Allow,
            RefreshStrategy::Force,
        )?);
    }
    let mut detached = writer.get_graph(&ids[0])?;
    detached.clear();
    assert_eq!(writer.get_graph(&ids[0])?.len(), 2);
    assert_eq!(writer.copy_graph(&ids[0])?.len(), 2);
    capability_error(writer.io().oxigraph_store());
    let (dataset, failures) = writer.io().union_graph(&ids);
    assert!(failures.is_empty());
    assert_eq!(dataset.len(), 4);
    assert_eq!(writer.io().size()?.num_graphs, 2);
    assert_eq!(writer.io().size()?.num_triples, 4);
    writer.flush()?; // Managed flush must not fall back to Oxigraph with rocksdb.
    let mut reader = OntoEnv::open(root.path().to_path_buf(), true)?;
    capability_error(reader.io().oxigraph_store());
    assert_eq!(reader.io().graph_ids()?.len(), 2);
    assert_eq!(reader.get_graph(&ids[1])?.len(), 2);
    reader.flush()?;
    Ok(())
}

#[test]
fn lazy_legacy_caches_do_not_claim_to_be_authoritative_stores() -> Result<()> {
    let root = tempfile::tempdir()?;
    let mut original = PersistentGraphIO::new(root.path().to_path_buf(), true, true)?;
    let mut ids = Vec::new();
    for name in ["urn:first", "urn:second"] {
        ids.push(
            original
                .add_from_bytes(
                    source(name),
                    ttl(name, "value"),
                    Some(RdfFormat::Turtle),
                    Overwrite::Allow,
                )?
                .id()
                .clone(),
        );
    }
    original.flush()?;
    drop(original);
    let writable = PersistentGraphIO::new(root.path().to_path_buf(), true, true)?;
    let readonly = ReadOnlyPersistentGraphIO::new(root.path().to_path_buf(), true)?;
    for backend in [&writable as &dyn GraphIO, &readonly as &dyn GraphIO] {
        capability_error(backend.oxigraph_store());
        assert_eq!(backend.graph_ids()?.len(), 2);
        assert_eq!(backend.size()?.num_triples, 4);
        assert_eq!(backend.get_graph(&ids[0])?.len(), 2);
        capability_error(backend.oxigraph_store()); // Warming a cache does not change the contract.
        let (union, failures) = backend.union_graph(&ids);
        assert!(failures.is_empty());
        assert_eq!(union.len(), 4);
        assert_eq!(backend.get_graph(&ids[1])?.len(), 2);
        capability_error(backend.oxigraph_store());
    }
    Ok(())
}

#[test]
fn supported_raw_access_and_default_operations_share_the_real_store() -> Result<()> {
    let original = Store::new()?;
    let external = ExternalStoreGraphIO::new(original.clone(), true, true);
    let memory = MemoryGraphIO::new(true, true)?;
    let backends: [(Box<dyn GraphIO>, Option<Store>); 2] = [
        (Box::new(external), Some(original.clone())),
        (Box::new(memory), None),
    ];
    for (mut backend, supplied) in backends {
        let name = NamedNode::new("urn:graph")?;
        let id = GraphIdentifier::new(name.as_ref());
        let triple = Triple::new(
            NamedNode::new("urn:subject")?,
            NamedNode::new("urn:value")?,
            Literal::new_simple_literal("raw"),
        );
        let quad = Quad::new(
            triple.subject.clone(),
            triple.predicate.clone(),
            triple.object.clone(),
            name.clone(),
        );
        backend.oxigraph_store()?.insert(&quad)?;
        if let Some(store) = &supplied {
            assert!(store.contains(&quad)?);
        }
        assert_eq!(backend.graph_ids()?, vec![id.clone()]);
        assert_eq!(backend.get_graph(&id)?.len(), 1);
        assert_eq!(backend.size()?.num_triples, 1);
        let (union, failures) = backend.union_graph(std::slice::from_ref(&id));
        assert!(failures.is_empty());
        assert_eq!(union.len(), 1);
        let replacement = Triple::new(
            triple.subject,
            triple.predicate,
            Literal::new_simple_literal("replacement"),
        );
        let mut graph = Graph::new();
        graph.insert(replacement.as_ref());
        backend.add_named_graph(id.clone(), graph)?;
        if let Some(store) = &supplied {
            let replaced = Quad::new(
                replacement.subject.clone(),
                replacement.predicate.clone(),
                replacement.object.clone(),
                name,
            );
            assert!(store.contains(&replaced)?);
            assert!(!store.contains(&quad)?);
        }
        assert!(!backend.oxigraph_store()?.contains(&quad)?);
        assert!(backend.get_graph(&id)?.contains(replacement.as_ref()));
        backend.flush()?;
        backend.remove(&id)?;
        assert_eq!(backend.size()?.num_triples, 0);
    }
    // The externally supplied handle observes the default write/remove paths too.
    assert!(original.is_empty()?);
    Ok(())
}

struct NoOxigraph;

impl GraphIO for NoOxigraph {
    fn io_type(&self) -> String {
        "test opaque backend".into()
    }
    fn is_offline(&self) -> bool {
        true
    }
    fn store_location(&self) -> Option<&Path> {
        None
    }
    fn add(&mut self, _: OntologyLocation, _: Overwrite) -> Result<Ontology> {
        bail!("test backend does not ingest sources")
    }
    fn add_from_bytes(
        &mut self,
        _: OntologyLocation,
        _: Vec<u8>,
        _: Option<RdfFormat>,
        _: Overwrite,
    ) -> Result<Ontology> {
        bail!("test backend does not ingest sources")
    }
}

#[test]
fn unsupported_default_operations_fail_instead_of_returning_empty_success() -> Result<()> {
    let mut backend = NoOxigraph;
    let id = GraphIdentifier::new(NamedNode::new("urn:missing")?.as_ref());
    capability_error(backend.graph_ids());
    capability_error(backend.size());
    capability_error(backend.get_graph(&id));
    capability_error(backend.copy_graph(&id));
    capability_error(backend.add_named_graph(id.clone(), Graph::new()));
    capability_error(backend.remove(&id));
    #[cfg(feature = "rocksdb")]
    capability_error(backend.flush());
    let (union, failures) = backend.union_graph(&[id]);
    assert!(union.is_empty());
    assert_eq!(failures.len(), 1);
    assert!(failures[0].error().contains("authoritative Oxigraph store"));
    Ok(())
}
