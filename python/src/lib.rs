use ::ontoenv::api::{find_ontoenv_root_from, OntoEnv as OntoEnvRs, ResolveTarget};
use ::ontoenv::config;
use ::ontoenv::consts::{IMPORTS, ONTOLOGY, TYPE};
use ::ontoenv::errors::OfflineRetrievalError;
use ::ontoenv::io::{GraphIO, StoreStats};
use ::ontoenv::ontology::{GraphIdentifier, Ontology as OntologyRs, OntologyLocation};
use ::ontoenv::options::{CacheMode, Overwrite, RefreshStrategy};
use ::ontoenv::transform;
use ::ontoenv::util::{get_file_contents, get_url_contents};
use ::ontoenv::ToUriString;
use anyhow::{anyhow, Error, Result};
use chrono::prelude::*;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{
    BlankNode, Graph as OxigraphGraph, GraphNameRef, Literal, NamedNode, NamedOrBlankNode,
    NamedOrBlankNodeRef, Term, TermRef, Triple, TripleRef,
};
use oxigraph::store::Store;
#[cfg(not(feature = "cli"))]
use pyo3::exceptions::PyRuntimeError;
use pyo3::{
    prelude::*,
    types::{IntoPyDict, PyString, PyStringMethods, PyTuple},
};
use rand::random;
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn anyhow_to_pyerr(e: Error) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
}

fn pyerr_to_anyhow(e: PyErr) -> Error {
    anyhow!(e.to_string())
}

struct ResolvedLocation {
    location: OntologyLocation,
    preferred_name: Option<String>,
}

fn ontology_location_from_py(location: &Bound<'_, PyAny>) -> PyResult<ResolvedLocation> {
    let ontology_subject = extract_ontology_subject(location)?;

    // Direct string extraction covers `str`, `Path`, `pathlib.Path`, etc.
    if let Ok(path_like) = location.extract::<PathBuf>() {
        return OntologyLocation::from_str(path_like.to_string_lossy().as_ref())
            .map(|loc| ResolvedLocation {
                location: loc,
                preferred_name: ontology_subject,
            })
            .map_err(anyhow_to_pyerr);
    }

    if let Ok(fspath_obj) = location.call_method0("__fspath__") {
        if let Ok(path_like) = fspath_obj.extract::<PathBuf>() {
            return OntologyLocation::from_str(path_like.to_string_lossy().as_ref())
                .map(|loc| ResolvedLocation {
                    location: loc,
                    preferred_name: ontology_subject,
                })
                .map_err(anyhow_to_pyerr);
        }
        let fspath = pyany_to_string(&fspath_obj)?;
        return OntologyLocation::from_str(&fspath)
            .map(|loc| ResolvedLocation {
                location: loc,
                preferred_name: ontology_subject,
            })
            .map_err(anyhow_to_pyerr);
    }

    if let Ok(base_attr) = location.getattr("base") {
        if !base_attr.is_none() {
            let base = pyany_to_string(&base_attr)?;
            if !base.is_empty() {
                if let Ok(loc) = OntologyLocation::from_str(&base) {
                    return Ok(ResolvedLocation {
                        location: loc,
                        preferred_name: ontology_subject,
                    });
                }
            }
        }
    }

    if let Ok(identifier_attr) = location.getattr("identifier") {
        if !identifier_attr.is_none() {
            let identifier_str = pyany_to_string(&identifier_attr)?;
            if !identifier_str.is_empty()
                && (identifier_str.starts_with("file:") || Path::new(&identifier_str).exists())
            {
                if let Ok(loc) = OntologyLocation::from_str(&identifier_str) {
                    return Ok(ResolvedLocation {
                        location: loc,
                        preferred_name: ontology_subject,
                    });
                }
            }
        }
    }

    if location.hasattr("serialize")? {
        let identifier = ontology_subject
            .clone()
            .unwrap_or_else(generate_rdflib_graph_identifier);
        return Ok(ResolvedLocation {
            location: OntologyLocation::InMemory { identifier },
            preferred_name: ontology_subject,
        });
    }

    let as_string = pyany_to_string(location)?;

    if as_string.starts_with("file:") || Path::new(&as_string).exists() {
        return OntologyLocation::from_str(&as_string)
            .map(|loc| ResolvedLocation {
                location: loc,
                preferred_name: ontology_subject,
            })
            .map_err(anyhow_to_pyerr);
    }

    Ok(ResolvedLocation {
        location: OntologyLocation::Url(generate_rdflib_graph_identifier()),
        preferred_name: ontology_subject,
    })
}

fn generate_rdflib_graph_identifier() -> String {
    format!("rdflib:graph-{}", random_hex_suffix())
}

fn random_hex_suffix() -> String {
    format!("{:08x}", random::<u32>())
}

fn extract_ontology_subject(graph: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    if !graph.hasattr("subjects")? {
        return Ok(None);
    }

    let py = graph.py();
    let namespace = PyModule::import(py, "rdflib.namespace")?;
    let rdf = namespace.getattr("RDF")?;
    let rdf_type = rdf.getattr("type")?;
    let owl = namespace.getattr("OWL")?;
    let ontology_term = match owl.getattr("Ontology") {
        Ok(term) => term,
        Err(_) => owl.call_method1("__getitem__", ("Ontology",))?,
    };

    let subjects_iter = graph.call_method1("subjects", (rdf_type, ontology_term))?;
    let mut iterator = subjects_iter.try_iter()?;

    if let Some(first_res) = iterator.next() {
        let first = first_res?;
        let subject_str = pyany_to_string(&first)?;
        if !subject_str.is_empty() {
            return Ok(Some(subject_str));
        }
    }

    Ok(None)
}

fn extract_import_root_subject(graph: &Bound<'_, PyAny>) -> PyResult<Option<String>> {
    if !graph.hasattr("subjects")? {
        return Ok(None);
    }

    let py = graph.py();
    let namespace = PyModule::import(py, "rdflib.namespace")?;
    let owl = namespace.getattr("OWL")?;
    let imports_term = match owl.getattr("imports") {
        Ok(term) => term,
        Err(_) => owl.call_method1("__getitem__", ("imports",))?,
    };

    let subjects_iter = graph.call_method1("subjects", (imports_term, py.None()))?;
    let mut iterator = subjects_iter.try_iter()?;

    if let Some(first_res) = iterator.next() {
        let first = first_res?;
        let subject_str = pyany_to_string(&first)?;
        if !subject_str.is_empty() {
            return Ok(Some(subject_str));
        }
    }

    Ok(None)
}

fn resolve_root_subject_and_graphid(
    graph: &Bound<'_, PyAny>,
    env: &OntoEnvRs,
) -> PyResult<(Option<String>, Option<GraphIdentifier>)> {
    let root_subject = match extract_import_root_subject(graph)? {
        Some(root) => Some(root),
        None => extract_ontology_subject(graph)?,
    };

    let mut root_graphid = None;
    if let Some(ref root) = root_subject {
        if let Ok(root_node) = NamedNode::new(root) {
            root_graphid = env.resolve(ResolveTarget::Graph(root_node));
        }
    }

    Ok((root_subject, root_graphid))
}

fn promote_root_graphid(
    all_ontologies: &mut Vec<GraphIdentifier>,
    root_graphid: &GraphIdentifier,
) {
    if let Some(pos) = all_ontologies.iter().position(|id| id == root_graphid) {
        if pos != 0 {
            let root = all_ontologies.remove(pos);
            all_ontologies.insert(0, root);
        }
    } else {
        all_ontologies.insert(0, root_graphid.clone());
    }
}

fn rewrite_sh_prefixes_rdflib(
    py: Python,
    graph: &Bound<'_, PyAny>,
    root_uri: &str,
) -> PyResult<()> {
    // This is a Python-side fallback for in-memory rdflib.Graph inputs.
    // The Rust union graph rewrite only knows about OntoEnv-managed graphs,
    // so for in-memory graphs we normalize sh:prefixes and sh:declare directly.
    //
    // Behavior:
    // - Every (shape, sh:prefixes, X) becomes (shape, sh:prefixes, root).
    // - Every (ontology, sh:declare, decl) is moved to the root ontology,
    //   deduplicated by (sh:prefix, sh:namespace).
    let rdflib = py.import("rdflib")?;
    let uriref = rdflib.getattr("URIRef")?;
    let sh_prefixes = uriref.call1(("http://www.w3.org/ns/shacl#prefixes",))?;
    let sh_declare = uriref.call1(("http://www.w3.org/ns/shacl#declare",))?;
    let sh_prefix = uriref.call1(("http://www.w3.org/ns/shacl#prefix",))?;
    let sh_namespace = uriref.call1(("http://www.w3.org/ns/shacl#namespace",))?;
    let root_ref = uriref.call1((root_uri,))?;

    // Collect subjects that reference sh:prefixes so we can re-add them with the root.
    let triples_iter = graph.call_method1("triples", ((py.None(), &sh_prefixes, py.None()),))?;
    let mut subjects = Vec::new();
    let mut to_remove = Vec::new();
    for triple in triples_iter.try_iter()? {
        let t = triple?;
        subjects.push(t.get_item(0)?);
        to_remove.push(t);
    }
    // Remove every existing sh:prefixes triple, regardless of its object.
    for triple in to_remove {
        graph.getattr("remove")?.call1((triple,))?;
    }
    // Re-add sh:prefixes with the root ontology as the object.
    for subj in subjects {
        let new_triple = PyTuple::new(py, &[subj, sh_prefixes.clone(), root_ref.clone()])?;
        graph.getattr("add")?.call1((new_triple,))?;
    }

    // Track existing (prefix, namespace) declarations on the root to avoid duplicates.
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let root_decl_iter =
        graph.call_method1("triples", ((&root_ref, &sh_declare, py.None()),))?;
    for triple in root_decl_iter.try_iter()? {
        let t = triple?;
        let decl = t.get_item(2)?;
        let pref = graph.call_method1("value", (&decl, &sh_prefix, py.None()))?;
        let ns = graph.call_method1("value", (&decl, &sh_namespace, py.None()))?;
        if !pref.is_none() && !ns.is_none() {
            let pv = pyany_to_string(&pref)?;
            let nv = pyany_to_string(&ns)?;
            if !pv.is_empty() && !nv.is_empty() {
                seen.insert((pv, nv));
            }
        }
    }

    // Move all sh:declare entries to the root ontology, deduplicating by (prefix, namespace).
    let declare_iter =
        graph.call_method1("triples", ((py.None(), &sh_declare, py.None()),))?;
    let mut declare_triples = Vec::new();
    for triple in declare_iter.try_iter()? {
        declare_triples.push(triple?);
    }
    for triple in declare_triples {
        let subj = triple.get_item(0)?;
        if subj.eq(&root_ref)? {
            continue;
        }
        let decl = triple.get_item(2)?;
        graph.getattr("remove")?.call1((triple.clone(),))?;

        // Extract prefix/namespace from the declaration node (if present).
        let pref = graph.call_method1("value", (&decl, &sh_prefix, py.None()))?;
        let ns = graph.call_method1("value", (&decl, &sh_namespace, py.None()))?;
        if !pref.is_none() && !ns.is_none() {
            let pv = pyany_to_string(&pref)?;
            let nv = pyany_to_string(&ns)?;
            if !pv.is_empty() && !nv.is_empty() && !seen.insert((pv, nv)) {
                // Duplicate declaration; skip re-attaching to root.
                continue;
            }
        }
        // Attach the declaration node to the root ontology.
        let new_triple = PyTuple::new(py, &[root_ref.clone(), sh_declare.clone(), decl])?;
        graph.getattr("add")?.call1((new_triple,))?;
    }

    Ok(())
}

// Helper function to format paths with forward slashes for cross-platform error messages
#[allow(dead_code)]
struct MyTerm(Term);
impl From<Result<Bound<'_, PyAny>, pyo3::PyErr>> for MyTerm {
    fn from(s: Result<Bound<'_, PyAny>, pyo3::PyErr>) -> Self {
        let s = s.unwrap();
        let typestr = s.get_type().name().unwrap();
        let typestr = typestr.to_string();
        let data_type: Option<NamedNode> = match s.getattr("datatype") {
            Ok(dt) => {
                if dt.is_none() {
                    None
                } else {
                    Some(NamedNode::new(dt.to_string()).unwrap())
                }
            }
            Err(_) => None,
        };
        let lang: Option<String> = match s.getattr("language") {
            Ok(l) => {
                if l.is_none() {
                    None
                } else {
                    Some(l.to_string())
                }
            }
            Err(_) => None,
        };
        let n: Term = match typestr.borrow() {
            "URIRef" => Term::NamedNode(NamedNode::new(s.to_string()).unwrap()),
            "Literal" => match (data_type, lang) {
                (Some(dt), None) => Term::Literal(Literal::new_typed_literal(s.to_string(), dt)),
                (None, Some(l)) => {
                    Term::Literal(Literal::new_language_tagged_literal(s.to_string(), l).unwrap())
                }
                (_, _) => Term::Literal(Literal::new_simple_literal(s.to_string())),
            },
            "BNode" => Term::BlankNode(BlankNode::new(s.to_string()).unwrap()),
            _ => Term::NamedNode(NamedNode::new(s.to_string()).unwrap()),
        };
        MyTerm(n)
    }
}

fn term_to_python<'a>(
    py: Python,
    rdflib: &Bound<'a, PyModule>,
    node: Term,
) -> PyResult<Bound<'a, PyAny>> {
    let dtype: Option<String> = match &node {
        Term::Literal(lit) => {
            let mut s = lit.datatype().to_string();
            s.remove(0);
            s.remove(s.len() - 1);
            Some(s)
        }
        _ => None,
    };
    let lang: Option<&str> = match &node {
        Term::Literal(lit) => lit.language(),
        _ => None,
    };

    let res: Bound<'_, PyAny> = match &node {
        Term::NamedNode(uri) => {
            let mut uri = uri.to_string();
            uri.remove(0);
            uri.remove(uri.len() - 1);
            rdflib.getattr("URIRef")?.call1((uri,))?
        }
        Term::Literal(literal) => {
            match (dtype, lang) {
                // prioritize 'lang' -> it implies String
                (_, Some(lang)) => {
                    rdflib
                        .getattr("Literal")?
                        .call1((literal.value(), lang, py.None()))?
                }
                (Some(dtype), None) => {
                    rdflib
                        .getattr("Literal")?
                        .call1((literal.value(), py.None(), dtype))?
                }
                (None, None) => rdflib.getattr("Literal")?.call1((literal.value(),))?,
            }
        }
        Term::BlankNode(id) => rdflib
            .getattr("BNode")?
            .call1((id.clone().into_string(),))?,
    };
    Ok(res)
}

fn term_from_python(node: &Bound<'_, PyAny>) -> Result<Term> {
    let type_name = node
        .get_type()
        .name()
        .map_err(pyerr_to_anyhow)?
        .to_string();
    let value = pyany_to_string(node).map_err(pyerr_to_anyhow)?;
    let data_type: Option<NamedNode> = match node.getattr("datatype") {
        Ok(dt) => {
            if dt.is_none() {
                None
            } else {
                let dt_str = pyany_to_string(&dt).map_err(pyerr_to_anyhow)?;
                Some(NamedNode::new(dt_str).map_err(|e| anyhow!(e.to_string()))?)
            }
        }
        Err(_) => None,
    };
    let lang: Option<String> = match node.getattr("language") {
        Ok(l) => {
            if l.is_none() {
                None
            } else {
                Some(pyany_to_string(&l).map_err(pyerr_to_anyhow)?)
            }
        }
        Err(_) => None,
    };

    let term = match type_name.as_str() {
        "URIRef" => Term::NamedNode(NamedNode::new(value).map_err(|e| anyhow!(e.to_string()))?),
        "Literal" => match (data_type, lang) {
            (Some(dt), None) => Term::Literal(Literal::new_typed_literal(value, dt)),
            (None, Some(l)) => Term::Literal(
                Literal::new_language_tagged_literal(value, l).map_err(|e| anyhow!(e.to_string()))?,
            ),
            _ => Term::Literal(Literal::new_simple_literal(value)),
        },
        "BNode" => Term::BlankNode(BlankNode::new(value).map_err(|e| anyhow!(e.to_string()))?),
        _ => Term::NamedNode(NamedNode::new(value).map_err(|e| anyhow!(e.to_string()))?),
    };
    Ok(term)
}

fn term_to_subject(term: Term) -> Result<NamedOrBlankNode> {
    match term {
        Term::NamedNode(n) => Ok(NamedOrBlankNode::NamedNode(n)),
        Term::BlankNode(b) => Ok(NamedOrBlankNode::BlankNode(b)),
        _ => Err(anyhow!("Invalid subject term type")),
    }
}

fn term_to_predicate(term: Term) -> Result<NamedNode> {
    match term {
        Term::NamedNode(n) => Ok(n),
        _ => Err(anyhow!("Predicate must be a named node")),
    }
}

fn graph_from_rdflib(_py: Python<'_>, graph: &Bound<'_, PyAny>) -> Result<OxigraphGraph> {
    let iter = graph.try_iter().map_err(pyerr_to_anyhow)?;
    let mut out = OxigraphGraph::new();
    for item in iter {
        let item = item.map_err(pyerr_to_anyhow)?;
        let triple = item.cast::<PyTuple>().map_err(|e| anyhow!(e.to_string()))?;
        if triple.len() != 3 {
            return Err(anyhow!("Expected rdflib triple tuples of length 3"));
        }
        let s = term_from_python(&triple.get_item(0).map_err(pyerr_to_anyhow)?)?;
        let p = term_from_python(&triple.get_item(1).map_err(pyerr_to_anyhow)?)?;
        let o = term_from_python(&triple.get_item(2).map_err(pyerr_to_anyhow)?)?;
        let subject = term_to_subject(s)?;
        let predicate = term_to_predicate(p)?;
        let triple = Triple::new(subject, predicate, o);
        out.insert(&triple);
    }
    Ok(out)
}

fn graph_to_rdflib<'a>(
    py: Python<'a>,
    graph: &OxigraphGraph,
) -> PyResult<Bound<'a, PyAny>> {
    let rdflib = PyModule::import(py, "rdflib")?;
    let res = rdflib.getattr("Graph")?.call0()?;
    for t in graph.iter() {
        let tuple = PyTuple::new(
            py,
            &[
                term_to_python(py, &rdflib, t.subject.into())?,
                term_to_python(py, &rdflib, t.predicate.into())?,
                term_to_python(py, &rdflib, t.object.into())?,
            ],
        )?;
        res.getattr("add")?.call1((tuple,))?;
    }
    Ok(res)
}

fn load_staging_store_from_bytes(bytes: &[u8], preferred: Option<RdfFormat>) -> Result<Store> {
    let mut candidates = vec![RdfFormat::Turtle, RdfFormat::RdfXml, RdfFormat::NTriples];
    if let Some(p) = preferred {
        candidates.retain(|f| *f != p);
        candidates.insert(0, p);
    }
    let store = Store::new().map_err(|e| anyhow!(e.to_string()))?;
    for fmt in candidates {
        let staging_graph = NamedNode::new_unchecked("temp:graph");
        let parser = RdfParser::from_format(fmt)
            .with_default_graph(GraphNameRef::NamedNode(staging_graph.as_ref()))
            .without_named_graphs();
        let mut loader = store.bulk_loader();
        match loader.load_from_reader(parser, std::io::Cursor::new(bytes)) {
            Ok(_) => {
                loader.commit().map_err(|e| anyhow!(e.to_string()))?;
                return Ok(store);
            }
            Err(_) => continue,
        }
    }
    Err(anyhow!("Failed to parse RDF bytes in any supported format"))
}

fn parse_ontology_bytes(
    location: &OntologyLocation,
    bytes: &[u8],
    format: Option<RdfFormat>,
    strict: bool,
) -> Result<(OntologyRs, OxigraphGraph)> {
    let staging_graph = NamedNode::new_unchecked("temp:graph");
    let tmp_store = load_staging_store_from_bytes(bytes, format)?;
    let staging_id = GraphIdentifier::new_with_location(staging_graph.as_ref(), location.clone());
    let mut ontology = OntologyRs::from_store(&tmp_store, &staging_id, strict)?;
    let hash = blake3::hash(bytes).to_hex().to_string();
    ontology.set_content_hash(hash);
    ontology.with_last_updated(Utc::now());

    let mut graph = OxigraphGraph::new();
    for quad in tmp_store.quads_for_pattern(
        None,
        None,
        None,
        Some(GraphNameRef::NamedNode(staging_graph.as_ref())),
    ) {
        let quad = quad.map_err(|e: oxigraph::store::StorageError| anyhow!(e.to_string()))?;
        graph.insert(quad.as_ref());
    }
    Ok((ontology, graph))
}

fn pystring_to_string(py_str: &Bound<'_, PyString>) -> PyResult<String> {
    Ok(py_str.to_cow()?.into_owned())
}

fn pyany_to_string(value: &Bound<'_, PyAny>) -> PyResult<String> {
    pystring_to_string(&value.str()?)
}

fn graph_store_description(_py: Python<'_>, store: &Bound<'_, PyAny>) -> PyResult<String> {
    let class = store.getattr("__class__")?;
    let module = pyany_to_string(&class.getattr("__module__")?)?;
    let qualname = pyany_to_string(&class.getattr("__qualname__")?)?;
    if module.is_empty() {
        Ok(qualname)
    } else if qualname.is_empty() {
        Ok(module)
    } else {
        Ok(format!("{module}:{qualname}"))
    }
}

struct PythonGraphIO {
    store: Mutex<Py<PyAny>>,
    offline: bool,
    strict: bool,
    read_only: bool,
    scratch: Store,
}

impl PythonGraphIO {
    fn new(store: Py<PyAny>, offline: bool, strict: bool, read_only: bool) -> Result<Self> {
        Ok(Self {
            store: Mutex::new(store),
            offline,
            strict,
            read_only,
            scratch: Store::new().map_err(|e| anyhow!(e.to_string()))?,
        })
    }

    fn with_store<F, T>(&self, f: F) -> Result<T>
    where
        F: for<'py> FnOnce(Python<'py>, Bound<'py, PyAny>) -> Result<T>,
    {
        let store = self
            .store
            .lock()
            .map_err(|_| anyhow!("Failed to lock python graph store"))?;
        Python::attach(|py| {
            let bound = store.clone_ref(py).into_bound(py);
            f(py, bound)
        })
    }

    fn add_graph_to_store(
        &self,
        py: Python<'_>,
        store: &Bound<'_, PyAny>,
        id: &str,
        graph: &OxigraphGraph,
        overwrite: Overwrite,
    ) -> Result<()> {
        let graph_py = graph_to_rdflib(py, graph).map_err(pyerr_to_anyhow)?;
        let method = store.getattr("add_graph").map_err(pyerr_to_anyhow)?;
        let result = method.call1((id, graph_py.clone(), overwrite.as_bool()));
        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                if err.is_instance_of::<pyo3::exceptions::PyTypeError>(py) {
                    method
                        .call1((id, graph_py))
                        .map_err(pyerr_to_anyhow)?;
                    Ok(())
                } else {
                    Err(pyerr_to_anyhow(err))
                }
            }
        }
    }

    fn graph_ids_from_store(
        &self,
        _py: Python<'_>,
        store: &Bound<'_, PyAny>,
    ) -> Result<Vec<String>> {
        if !store
            .hasattr("graph_ids")
            .map_err(pyerr_to_anyhow)?
        {
            return Err(anyhow!(
                "Python graph store must define graph_ids() to report stored graphs"
            ));
        }
        let ids_obj = store.call_method0("graph_ids").map_err(pyerr_to_anyhow)?;
        let iter = ids_obj.try_iter().map_err(pyerr_to_anyhow)?;
        let mut ids = Vec::new();
        for item in iter {
            let item = item.map_err(pyerr_to_anyhow)?;
            let id = pyany_to_string(&item).map_err(pyerr_to_anyhow)?;
            ids.push(id);
        }
        Ok(ids)
    }
}

impl GraphIO for PythonGraphIO {
    fn is_offline(&self) -> bool {
        self.offline
    }

    fn io_type(&self) -> String {
        "python".to_string()
    }

    fn store_location(&self) -> Option<&Path> {
        None
    }

    fn store(&self) -> &Store {
        &self.scratch
    }

    fn add(&mut self, location: OntologyLocation, overwrite: Overwrite) -> Result<OntologyRs> {
        if self.read_only {
            return Err(anyhow!("Cannot add to read-only store"));
        }

        let (bytes, format) = match &location {
            OntologyLocation::File(path) => get_file_contents(path)?,
            OntologyLocation::Url(url) => {
                if self.offline {
                    return Err(Error::new(OfflineRetrievalError { file: url.clone() }));
                }
                get_url_contents(url)?
            }
            OntologyLocation::InMemory { .. } => {
                return Err(anyhow!(
                    "In-memory ontologies cannot be added via the python graph store"
                ))
            }
        };

        let (ontology, graph) = parse_ontology_bytes(&location, &bytes, format, self.strict)?;
        let graph_id = ontology.id().to_uri_string();
        self.with_store(|py, store| {
            self.add_graph_to_store(py, &store, &graph_id, &graph, overwrite)
        })?;
        Ok(ontology)
    }

    fn add_from_bytes(
        &mut self,
        location: OntologyLocation,
        bytes: Vec<u8>,
        format: Option<RdfFormat>,
        overwrite: Overwrite,
    ) -> Result<OntologyRs> {
        if self.read_only {
            return Err(anyhow!("Cannot add to read-only store"));
        }
        let (ontology, graph) = parse_ontology_bytes(&location, &bytes, format, self.strict)?;
        let graph_id = ontology.id().to_uri_string();
        self.with_store(|py, store| {
            self.add_graph_to_store(py, &store, &graph_id, &graph, overwrite)
        })?;
        Ok(ontology)
    }

    fn get_graph(&self, id: &GraphIdentifier) -> Result<OxigraphGraph> {
        let graph_id = id.to_uri_string();
        self.with_store(|py, store| {
            let graph_obj = store
                .getattr("get_graph")
                .map_err(pyerr_to_anyhow)?
                .call1((graph_id.as_str(),))
                .map_err(pyerr_to_anyhow)?;
            if graph_obj.is_none() {
                return Err(anyhow!("Graph not found: {graph_id}"));
            }
            graph_from_rdflib(py, &graph_obj)
        })
    }

    fn size(&self) -> Result<StoreStats> {
        self.with_store(|py, store| {
            if store.hasattr("size").map_err(pyerr_to_anyhow)? {
                let res = store.call_method0("size").map_err(pyerr_to_anyhow)?;
                if let Ok(tuple) = res.cast::<PyTuple>() {
                    if tuple.len() == 2 {
                        let num_graphs = tuple
                            .get_item(0)
                            .map_err(pyerr_to_anyhow)?
                            .extract::<usize>()
                            .map_err(pyerr_to_anyhow)?;
                        let num_triples = tuple
                            .get_item(1)
                            .map_err(pyerr_to_anyhow)?
                            .extract::<usize>()
                            .map_err(pyerr_to_anyhow)?;
                        return Ok(StoreStats {
                            num_triples,
                            num_graphs,
                        });
                    }
                }
                if let Ok(dict) = res.cast::<pyo3::types::PyDict>() {
                    let num_triples = dict
                        .get_item("num_triples")
                        .map_err(pyerr_to_anyhow)?
                        .and_then(|item| item.extract::<usize>().ok());
                    let num_graphs = dict
                        .get_item("num_graphs")
                        .map_err(pyerr_to_anyhow)?
                        .and_then(|item| item.extract::<usize>().ok());
                    if let (Some(num_triples), Some(num_graphs)) = (num_triples, num_graphs) {
                        return Ok(StoreStats {
                            num_triples,
                            num_graphs,
                        });
                    }
                }
                if res.hasattr("num_triples").map_err(pyerr_to_anyhow)?
                    && res.hasattr("num_graphs").map_err(pyerr_to_anyhow)?
                {
                    let num_triples = res
                        .getattr("num_triples")
                        .map_err(pyerr_to_anyhow)?
                        .extract::<usize>()
                        .map_err(pyerr_to_anyhow)?;
                    let num_graphs = res
                        .getattr("num_graphs")
                        .map_err(pyerr_to_anyhow)?
                        .extract::<usize>()
                        .map_err(pyerr_to_anyhow)?;
                    return Ok(StoreStats {
                        num_triples,
                        num_graphs,
                    });
                }
            }

            if !store.hasattr("graph_ids").map_err(pyerr_to_anyhow)? {
                return Ok(StoreStats {
                    num_triples: 0,
                    num_graphs: 0,
                });
            }
            let ids = self.graph_ids_from_store(py, &store)?;
            let num_graphs = ids.len();
            let mut num_triples = 0usize;
            if store.hasattr("num_triples").map_err(pyerr_to_anyhow)? {
                let res = store
                    .getattr("num_triples")
                    .map_err(pyerr_to_anyhow)?;
                num_triples = if res.is_callable() {
                    res.call0()
                        .map_err(pyerr_to_anyhow)?
                        .extract::<usize>()
                        .map_err(pyerr_to_anyhow)?
                } else {
                    res.extract::<usize>().map_err(pyerr_to_anyhow)?
                };
            } else {
                for id in ids {
                    let graph_obj = store
                        .getattr("get_graph")
                        .map_err(pyerr_to_anyhow)?
                        .call1((id.as_str(),))
                        .map_err(pyerr_to_anyhow)?;
                    num_triples += graph_obj.len().map_err(pyerr_to_anyhow)?;
                }
            }

            Ok(StoreStats {
                num_triples,
                num_graphs,
            })
        })
    }

    fn remove(&mut self, id: &GraphIdentifier) -> Result<()> {
        if self.read_only {
            return Err(anyhow!("Cannot remove from read-only store"));
        }
        let graph_id = id.to_uri_string();
        self.with_store(|_py, store| {
            store
                .getattr("remove_graph")
                .map_err(pyerr_to_anyhow)?
                .call1((graph_id.as_str(),))
                .map_err(pyerr_to_anyhow)?;
            Ok(())
        })
    }

    fn flush(&mut self) -> Result<()> {
        self.with_store(|_py, store| {
            if store.hasattr("flush").map_err(pyerr_to_anyhow)? {
                store.call_method0("flush").map_err(pyerr_to_anyhow)?;
            }
            Ok(())
        })
    }

    fn begin_batch(&mut self) -> Result<()> {
        self.with_store(|_py, store| {
            if store.hasattr("begin_batch").map_err(pyerr_to_anyhow)? {
                store.call_method0("begin_batch").map_err(pyerr_to_anyhow)?;
            }
            Ok(())
        })
    }

    fn end_batch(&mut self) -> Result<()> {
        self.with_store(|_py, store| {
            if store.hasattr("end_batch").map_err(pyerr_to_anyhow)? {
                store.call_method0("end_batch").map_err(pyerr_to_anyhow)?;
            }
            Ok(())
        })
    }
}

/// Run the Rust CLI implementation and return its process-style exit code.
#[pyfunction]
#[cfg(feature = "cli")]
fn run_cli(py: Python<'_>, args: Option<Vec<String>>) -> PyResult<i32> {
    let argv = args.unwrap_or_else(|| std::env::args().collect());
    let code = py.detach(move || match ontoenv_cli::run_from_args(argv) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    });
    Ok(code)
}

/// Fallback stub when the CLI feature is disabled at compile time.
#[pyfunction]
#[cfg(not(feature = "cli"))]
#[allow(unused_variables)]
fn run_cli(_py: Python<'_>, _args: Option<Vec<String>>) -> PyResult<i32> {
    Err(PyErr::new::<PyRuntimeError, _>(
        "ontoenv was built without CLI support; rebuild with the 'cli' feature",
    ))
}

#[pyclass(name = "Ontology")]
#[derive(Clone)]
struct PyOntology {
    inner: OntologyRs,
}

#[pymethods]
impl PyOntology {
    #[getter]
    fn id(&self) -> PyResult<String> {
        Ok(self.inner.id().to_uri_string())
    }

    #[getter]
    fn name(&self) -> PyResult<String> {
        Ok(self.inner.name().to_uri_string())
    }

    #[getter]
    fn imports(&self) -> PyResult<Vec<String>> {
        Ok(self
            .inner
            .imports
            .iter()
            .map(|i| i.to_uri_string())
            .collect())
    }

    #[getter]
    fn location(&self) -> PyResult<Option<String>> {
        Ok(self.inner.location().map(|l| l.to_string()))
    }

    #[getter]
    fn last_updated(&self) -> PyResult<Option<String>> {
        Ok(self.inner.last_updated.map(|dt| dt.to_rfc3339()))
    }

    #[getter]
    fn version_properties(&self) -> PyResult<HashMap<String, String>> {
        Ok(self
            .inner
            .version_properties()
            .iter()
            .map(|(k, v)| (k.to_uri_string(), v.clone()))
            .collect())
    }

    #[getter]
    fn namespace_map(&self) -> PyResult<HashMap<String, String>> {
        Ok(self.inner.namespace_map().clone())
    }

    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("<Ontology: {}>", self.inner.name().to_uri_string()))
    }
}

#[pyclass]
struct OntoEnv {
    inner: Arc<Mutex<Option<OntoEnvRs>>>,
}

#[pymethods]
impl OntoEnv {
    #[new]
    #[pyo3(signature = (path=None, recreate=false, create_or_use_cached=false, read_only=false, search_directories=None, require_ontology_names=false, strict=false, offline=false, use_cached_ontologies=false, resolution_policy="default".to_owned(), root=".".to_owned(), includes=None, excludes=None, include_ontologies=None, exclude_ontologies=None, temporary=false, remote_cache_ttl_secs=None, graph_store=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        _py: Python,
        path: Option<PathBuf>,
        recreate: bool,
        create_or_use_cached: bool,
        read_only: bool,
        search_directories: Option<Vec<String>>,
        require_ontology_names: bool,
        strict: bool,
        offline: bool,
        use_cached_ontologies: bool,
        resolution_policy: String,
        root: String,
        includes: Option<Vec<String>>,
        excludes: Option<Vec<String>>,
        include_ontologies: Option<Vec<String>>,
        exclude_ontologies: Option<Vec<String>>,
        temporary: bool,
        remote_cache_ttl_secs: Option<u64>,
        graph_store: Option<Py<PyAny>>,
    ) -> PyResult<Self> {
        let mut root_path = path.clone().unwrap_or_else(|| PathBuf::from(root));
        // If the provided path points to a '.ontoenv' directory, treat its parent as the root
        if root_path
            .file_name()
            .map(|n| n == OsStr::new(".ontoenv"))
            .unwrap_or(false)
        {
            if let Some(parent) = root_path.parent() {
                root_path = parent.to_path_buf();
            }
        }

        // Strict Git-like behavior:
        // - temporary=True: create a temporary (in-memory) env
        // - recreate=True: create (or overwrite) an env at root_path
        // - create_or_use_cached=True: create if missing, otherwise load
        // - otherwise: discover upward; if not found, error

        let mut builder = config::Config::builder()
            .root(root_path.clone())
            .require_ontology_names(require_ontology_names)
            .strict(strict)
            .offline(offline)
            .use_cached_ontologies(CacheMode::from(use_cached_ontologies))
            .resolution_policy(resolution_policy)
            .temporary(temporary);

        if let Some(dirs) = search_directories {
            let paths = dirs.into_iter().map(PathBuf::from).collect();
            builder = builder.locations(paths);
        }
        if let Some(incl) = includes {
            builder = builder.includes(incl);
        }
        if let Some(excl) = excludes {
            builder = builder.excludes(excl);
        }
        if let Some(incl_o) = include_ontologies {
            builder = builder.include_ontologies(incl_o);
        }
        if let Some(excl_o) = exclude_ontologies {
            builder = builder.exclude_ontologies(excl_o);
        }
        if let Some(ttl) = remote_cache_ttl_secs {
            builder = builder.remote_cache_ttl_secs(ttl);
        }

        let mut cfg = builder
            .build()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

        if let Some(store) = graph_store {
            if recreate || create_or_use_cached {
                return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "graph_store cannot be combined with recreate or create_or_use_cached",
                ));
            }
            let desc = graph_store_description(_py, store.bind(_py))?;
            cfg.external_graph_store = Some(desc);
            let io = PythonGraphIO::new(store, cfg.offline, cfg.strict, read_only)
                .map_err(anyhow_to_pyerr)?;
            let env = OntoEnvRs::new_with_graph_io(cfg, Box::new(io)).map_err(anyhow_to_pyerr)?;
            let inner = Arc::new(Mutex::new(Some(env)));
            return Ok(OntoEnv { inner });
        }

        let root_for_lookup = cfg.root.clone();
        let env = if cfg.temporary {
            OntoEnvRs::init(cfg, false).map_err(anyhow_to_pyerr)?
        } else if recreate {
            OntoEnvRs::init(cfg, true).map_err(anyhow_to_pyerr)?
        } else if create_or_use_cached {
            OntoEnvRs::open_or_init(cfg, read_only).map_err(anyhow_to_pyerr)?
        } else {
            let load_root = if let Some(found_root) =
                find_ontoenv_root_from(root_for_lookup.as_path())
            {
                found_root
            } else {
                let ontoenv_dir = root_for_lookup.join(".ontoenv");
                if ontoenv_dir.exists() {
                    root_for_lookup.clone()
                } else {
                    return Err(PyErr::new::<pyo3::exceptions::PyFileNotFoundError, _>(
                        format!(
                            "OntoEnv directory not found at {} (set create_or_use_cached=True to initialize a new environment)",
                            ontoenv_dir.display()
                        ),
                    ));
                }
            };
            OntoEnvRs::load_from_directory(load_root, read_only).map_err(anyhow_to_pyerr)?
        };

        let inner = Arc::new(Mutex::new(Some(env)));

        Ok(OntoEnv {
            inner: inner.clone(),
        })
    }

    #[pyo3(signature = (all=false))]
    fn update(&self, all: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        if let Some(env) = guard.as_mut() {
            env.update_all(all).map_err(anyhow_to_pyerr)?;
            env.save_to_directory().map_err(anyhow_to_pyerr)
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    // fn is_read_only(&self) -> PyResult<bool> {
    //     let inner = self.inner.clone();
    //     let env = inner.lock().unwrap();
    //     Ok(env.is_read_only())
    // }

    fn __repr__(&self) -> PyResult<String> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        if let Some(env) = guard.as_ref() {
            let stats = env.stats().map_err(anyhow_to_pyerr)?;
            Ok(format!(
                "<OntoEnv: {} ontologies, {} graphs, {} triples>",
                stats.num_ontologies, stats.num_graphs, stats.num_triples,
            ))
        } else {
            Ok("<OntoEnv: closed>".to_string())
        }
    }

    // The following methods will now access the inner OntoEnv in a thread-safe manner:

    #[pyo3(signature = (destination_graph, uri, recursion_depth = -1))]
    fn import_graph(
        &self,
        py: Python,
        destination_graph: &Bound<'_, PyAny>,
        uri: &str,
        recursion_depth: i32,
    ) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()))
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Failed to resolve graph for URI: {uri}"
                ))
            })?;

        // Compute closure starting from this ontology, honoring recursion depth and deduping loops.
        let closure = env
            .get_closure(&graphid, recursion_depth)
            .map_err(anyhow_to_pyerr)?;

        // Determine root ontology: prefer an existing ontology in the destination graph; else use the
        // imported ontology name.
        let uriref_constructor = rdflib.getattr("URIRef")?;
        let type_uri = uriref_constructor.call1((TYPE.as_str(),))?;
        let ontology_uri = uriref_constructor.call1((ONTOLOGY.as_str(),))?;
        let kwargs = [("predicate", type_uri), ("object", ontology_uri)].into_py_dict(py)?;
        let existing_root = destination_graph.call_method("value", (), Some(&kwargs))?;
        let root_node_owned: oxigraph::model::NamedNode = if existing_root.is_none() {
            graphid.name().into_owned()
        } else {
            NamedNode::new(existing_root.extract::<String>()?)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?
                .to_owned()
        };
        let root_node = root_node_owned.as_ref();

        // Remove owl:imports in the destination graph only for ontologies that will be rewritten.
        let imports_uri = uriref_constructor.call1((IMPORTS.as_str(),))?;
        let closure_set: std::collections::HashSet<String> =
            closure.iter().map(|c| c.to_uri_string()).collect();
        let triples_to_remove_imports = destination_graph.call_method(
            "triples",
            ((py.None(), imports_uri, py.None()),),
            None,
        )?;
        for triple in triples_to_remove_imports.try_iter()? {
            let t = triple?;
            let obj: Bound<'_, PyAny> = t.get_item(2)?;
            if let Ok(s) = obj.str() {
                let s = pystring_to_string(&s)?;
                if closure_set.contains(s.as_str()) {
                    destination_graph.getattr("remove")?.call1((t,))?;
                }
            }
        }

        // Remove any ontology declarations in the destination that are not the chosen root.
        let triples_to_remove = destination_graph.call_method(
            "triples",
            ((
                py.None(),
                uriref_constructor.call1((TYPE.as_str(),))?,
                uriref_constructor.call1((ONTOLOGY.as_str(),))?,
            ),),
            None,
        )?;
        for triple in triples_to_remove.try_iter()? {
            let t = triple?;
            let subj: Bound<'_, PyAny> = t.get_item(0)?;
            if pyany_to_string(&subj)? != root_node.as_str() {
                destination_graph.getattr("remove")?.call1((t,))?;
            }
        }

        // Merge closure graphs via the Rust API and then normalize onto the chosen root.
        let mut merged = env
            .import_graph(&graphid, recursion_depth)
            .map_err(anyhow_to_pyerr)?;
        let root_nb = NamedOrBlankNodeRef::NamedNode(root_node);
        transform::rewrite_sh_prefixes_graph(&mut merged, root_nb);
        transform::remove_ontology_declarations_graph(&mut merged, root_nb);

        let mut to_remove: Vec<Triple> = Vec::new();
        let mut import_targets: Vec<NamedNode> = Vec::new();
        {
            for triple in merged.triples_for_predicate(IMPORTS) {
                to_remove.push(triple.into());
                if let TermRef::NamedNode(obj) = triple.object {
                    import_targets.push(obj.into_owned());
                }
            }
        }
        for triple in to_remove {
            merged.remove(triple.as_ref());
        }
        let mut seen = std::collections::HashSet::new();
        for dep in import_targets {
            if dep.as_ref() == root_node {
                continue;
            }
            if seen.insert(dep.to_string()) {
                merged.insert(TripleRef::new(root_node, IMPORTS, dep.as_ref()));
            }
        }

        // Flatten triples into the destination graph.
        for triple in merged.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();
            let t = PyTuple::new(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            )?;
            destination_graph.getattr("add")?.call1((t,))?;
        }
        // Re-attach imports from the original closure onto the root in the destination graph.
        for dep in closure.iter().skip(1) {
            let dep_uri = dep.to_uri_string();
            let t = PyTuple::new(
                py,
                &[
                    uriref_constructor.call1((root_node.as_str(),))?,
                    uriref_constructor.call1((IMPORTS.as_str(),))?,
                    uriref_constructor.call1((dep_uri.as_str(),))?,
                ],
            )?;
            destination_graph.getattr("add")?.call1((t,))?;
        }
        Ok(())
    }

    /// List the ontologies in the imports closure of the given ontology
    #[pyo3(signature = (uri, recursion_depth = -1))]
    fn list_closure(&self, _py: Python, uri: &str, recursion_depth: i32) -> PyResult<Vec<String>> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()))
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Failed to resolve graph for URI: {uri}"
                ))
            })?;
        let ont = env.ontologies().get(&graphid).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology {iri} not found"))
        })?;
        let closure = env
            .get_closure(ont.id(), recursion_depth)
            .map_err(anyhow_to_pyerr)?;
        let names: Vec<String> = closure.iter().map(|ont| ont.to_uri_string()).collect();
        Ok(names)
    }

    /// Merge the imports closure of `uri` into a single graph and return it alongside the closure list.
    ///
    /// The first element of the returned tuple is either the provided `destination_graph` (after
    /// mutation) or a brand-new `rdflib.Graph`. The second element is an ordered list of ontology
    /// IRIs in the resolved closure starting with `uri`. Set `rewrite_sh_prefixes` or
    /// `remove_owl_imports` to control post-processing of the merged triples.
    #[pyo3(signature = (uri, destination_graph=None, rewrite_sh_prefixes=true, remove_owl_imports=true, recursion_depth=-1))]
    fn get_closure<'a>(
        &self,
        py: Python<'a>,
        uri: &str,
        destination_graph: Option<&Bound<'a, PyAny>>,
        rewrite_sh_prefixes: bool,
        remove_owl_imports: bool,
        recursion_depth: i32,
    ) -> PyResult<(Bound<'a, PyAny>, Vec<String>)> {
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()))
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("No graph with URI: {uri}"))
            })?;
        let ont = env.ontologies().get(&graphid).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology {iri} not found"))
        })?;
        let closure = env
            .get_closure(ont.id(), recursion_depth)
            .map_err(anyhow_to_pyerr)?;
        let closure_names: Vec<String> = closure.iter().map(|ont| ont.to_uri_string()).collect();
        // if destination_graph is null, create a new rdflib.Graph()
        let destination_graph = match destination_graph {
            Some(g) => g.clone(),
            None => rdflib.getattr("Graph")?.call0()?,
        };
        let union = env
            .get_union_graph(
                &closure,
                Some(rewrite_sh_prefixes),
                Some(remove_owl_imports),
            )
            .map_err(anyhow_to_pyerr)?;
        for triple in union.dataset.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();
            let t = PyTuple::new(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            )?;
            destination_graph.getattr("add")?.call1((t,))?;
        }

        // Remove each successful_imports url in the closure from the destination_graph
        if remove_owl_imports {
            for graphid in union.graph_ids {
                let iri = term_to_python(py, &rdflib, Term::NamedNode(graphid.into()))?;
                let pred = term_to_python(py, &rdflib, IMPORTS.into())?;
                // remove triples with (None, pred, iri)
                let remove_tuple = PyTuple::new(py, &[py.None(), pred.into(), iri.into()])?;
                destination_graph
                    .getattr("remove")?
                    .call1((remove_tuple,))?;
            }
        }
        Ok((destination_graph, closure_names))
    }

    /// Print the contents of the OntoEnv
    #[pyo3(signature = (includes=None))]
    fn dump(&self, _py: Python, includes: Option<String>) -> PyResult<()> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        if let Some(env) = guard.as_ref() {
            env.dump(includes.as_deref());
            Ok(())
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    /// Import the dependencies referenced by `owl:imports` triples in `graph`.
    ///
    /// When `fetch_missing` is true, the environment attempts to download unresolved imports
    /// before computing the closure. After merging the closure triples into `graph`, all
    /// `owl:imports` statements are removed. The returned list contains the deduplicated ontology
    /// IRIs that were successfully imported.
    #[pyo3(signature = (graph, recursion_depth=-1, fetch_missing=false))]
    fn import_dependencies<'a>(
        &self,
        py: Python<'a>,
        graph: &Bound<'a, PyAny>,
        recursion_depth: i32,
        fetch_missing: bool,
    ) -> PyResult<Vec<String>> {
        // This method takes an in-memory rdflib.Graph, finds its owl:imports,
        // resolves them in the OntoEnv store, computes the transitive closure,
        // and merges all closure triples into the input graph.
        //
        // Root selection:
        // - Prefer the subject of owl:imports (if present) as the "root" URI.
        // - Otherwise fall back to the first owl:Ontology subject in the graph.
        // - If the root URI resolves to a stored GraphIdentifier, we ensure that
        //   graph is first in the union list so Rust rewrites sh:prefixes to it.
        // - If it does NOT resolve, we skip Rust rewrite and do a Python-side
        //   rewrite directly on the rdflib graph.
        let rdflib = py.import("rdflib")?;
        let py_imports_pred = term_to_python(py, &rdflib, Term::NamedNode(IMPORTS.into()))?;

        // Gather owl:imports objects from the input graph.
        let kwargs = [("predicate", py_imports_pred)].into_py_dict(py)?;
        let objects_iter = graph.call_method("objects", (), Some(&kwargs))?;
        let builtins = py.import("builtins")?;
        let objects_list = builtins.getattr("list")?.call1((objects_iter,))?;
        let imports: Vec<String> = objects_list.extract()?;

        if imports.is_empty() {
            return Ok(Vec::new());
        }

        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;

        // Determine the root URI for sh:prefix rewrites.
        // Prefer owl:imports subject, else the owl:Ontology subject.
        let (root_subject, root_graphid) = resolve_root_subject_and_graphid(graph, env)?;

        let is_strict = env.is_strict();
        let mut all_ontologies: Vec<GraphIdentifier> = Vec::new();
        let mut seen_ontologies: HashSet<GraphIdentifier> = HashSet::new();
        let mut all_closure_names: Vec<String> = Vec::new();

        // Resolve each import to a stored graph (optionally fetching),
        // then collect the transitive closure for each.
        for uri in &imports {
            let iri = NamedNode::new(uri.as_str())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

            let mut graphid = env.resolve(ResolveTarget::Graph(iri.clone()));

            if graphid.is_none() && fetch_missing {
                let location = OntologyLocation::from_str(uri.as_str()).map_err(anyhow_to_pyerr)?;
                match env.add(location, Overwrite::Preserve, RefreshStrategy::UseCache) {
                    Ok(new_id) => {
                        graphid = Some(new_id);
                    }
                    Err(e) => {
                        if is_strict {
                            return Err(anyhow_to_pyerr(e));
                        }
                        println!("Failed to fetch {uri}: {e}");
                    }
                }
            }

            let graphid = match graphid {
                Some(id) => id,
                None => {
                    if is_strict {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                            "Failed to resolve graph for URI: {}",
                            uri
                        )));
                    }
                    println!("Could not find {uri:?}");
                    continue;
                }
            };

            let ont = env.ontologies().get(&graphid).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Ontology {} not found",
                    uri
                ))
            })?;

            let closure = env
                .get_closure(ont.id(), recursion_depth)
                .map_err(anyhow_to_pyerr)?;
            for c_ont in closure {
                all_closure_names.push(c_ont.to_uri_string());
                if seen_ontologies.insert(c_ont.clone()) {
                    all_ontologies.push(c_ont);
                }
            }
        }

        if all_ontologies.is_empty() {
            return Ok(Vec::new());
        }

        // If the root URI is in the env, move it to the front so Rust uses it
        // as the root for sh:prefixes rewrite. Otherwise, we will rewrite later in Python.
        if let Some(ref root_id) = root_graphid {
            promote_root_graphid(&mut all_ontologies, root_id);
        }

        let rewrite_in_rust = root_graphid.is_some();
        let union = env
            .get_union_graph(&all_ontologies, Some(rewrite_in_rust), Some(true))
            .map_err(anyhow_to_pyerr)?;

        for triple in union.dataset.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();
            let t = PyTuple::new(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            )?;
            graph.getattr("add")?.call1((t,))?;
        }

        // Remove all owl:imports from the original graph (they are now materialized).
        let py_imports_pred_for_remove = term_to_python(py, &rdflib, IMPORTS.into())?;
        let remove_tuple = PyTuple::new(
            py,
            &[py.None(), py_imports_pred_for_remove.into(), py.None()],
        )?;
        graph.getattr("remove")?.call1((remove_tuple,))?;

        // If Rust could not rewrite sh:prefixes (root not in env),
        // perform the rewrite directly in the rdflib graph.
        if !rewrite_in_rust {
            if let Some(ref root) = root_subject {
                rewrite_sh_prefixes_rdflib(py, graph, root)?;
            }
        }

        all_closure_names.sort();
        all_closure_names.dedup();

        Ok(all_closure_names)
    }

    /// Get the dependency closure of a given graph and return it as a new graph.
    ///
    /// This method will look for `owl:imports` statements in the provided `graph`,
    /// then find those ontologies within the `OntoEnv` and compute the full
    /// dependency closure. The triples of all ontologies in the closure are
    /// returned as a new graph. The original `graph` is left untouched unless you also
    /// supply it as the `destination_graph`.
    ///
    /// Args:
    ///     graph (rdflib.Graph): The graph to find dependencies for.
    ///     destination_graph (Optional[rdflib.Graph]): If provided, the dependency graph will be added to this
    ///         graph instead of creating a new one.
    ///     recursion_depth (int): The maximum depth for recursive import resolution. A
    ///         negative value (default) means no limit.
    ///     fetch_missing (bool): If True, will fetch ontologies that are not in the environment.
    ///     rewrite_sh_prefixes (bool): If True, will rewrite SHACL prefixes to be unique.
    ///     remove_owl_imports (bool): If True, will remove `owl:imports` statements from the
    ///         returned graph.
    ///
    /// Returns:
    ///     tuple[rdflib.Graph, list[str]]: A tuple containing the populated dependency graph and the sorted list of
    ///     imported ontology IRIs.
    #[pyo3(signature = (graph, destination_graph=None, recursion_depth=-1, fetch_missing=false, rewrite_sh_prefixes=true, remove_owl_imports=true))]
    #[allow(clippy::too_many_arguments)]
    fn get_dependencies_graph<'a>(
        &self,
        py: Python<'a>,
        graph: &Bound<'a, PyAny>,
        destination_graph: Option<&Bound<'a, PyAny>>,
        recursion_depth: i32,
        fetch_missing: bool,
        rewrite_sh_prefixes: bool,
        remove_owl_imports: bool,
    ) -> PyResult<(Bound<'a, PyAny>, Vec<String>)> {
        let rdflib = py.import("rdflib")?;
        let py_imports_pred = term_to_python(py, &rdflib, Term::NamedNode(IMPORTS.into()))?;

        let kwargs = [("predicate", py_imports_pred)].into_py_dict(py)?;
        let objects_iter = graph.call_method("objects", (), Some(&kwargs))?;
        let builtins = py.import("builtins")?;
        let objects_list = builtins.getattr("list")?.call1((objects_iter,))?;
        let imports: Vec<String> = objects_list.extract()?;

        let destination_graph = match destination_graph {
            Some(g) => g.clone(),
            None => rdflib.getattr("Graph")?.call0()?,
        };

        if imports.is_empty() {
            return Ok((destination_graph, Vec::new()));
        }

        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;

        // Determine the root URI for sh:prefix rewrites.
        // Prefer owl:imports subject, else the owl:Ontology subject.
        let (root_subject, root_graphid) = resolve_root_subject_and_graphid(graph, env)?;

        let is_strict = env.is_strict();
        let mut all_ontologies: Vec<GraphIdentifier> = Vec::new();
        let mut seen_ontologies: HashSet<GraphIdentifier> = HashSet::new();
        let mut all_closure_names: Vec<String> = Vec::new();

        for uri in &imports {
            let iri = NamedNode::new(uri.as_str())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

            let mut graphid = env.resolve(ResolveTarget::Graph(iri.clone()));

            if graphid.is_none() && fetch_missing {
                let location = OntologyLocation::from_str(uri.as_str()).map_err(anyhow_to_pyerr)?;
                match env.add(location, Overwrite::Preserve, RefreshStrategy::UseCache) {
                    Ok(new_id) => {
                        graphid = Some(new_id);
                    }
                    Err(e) => {
                        if is_strict {
                            return Err(anyhow_to_pyerr(e));
                        }
                        println!("Failed to fetch {uri}: {e}");
                    }
                }
            }

            let graphid = match graphid {
                Some(id) => id,
                None => {
                    if is_strict {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                            "Failed to resolve graph for URI: {}",
                            uri
                        )));
                    }
                    println!("Could not find {uri:?}");
                    continue;
                }
            };

            let ont = env.ontologies().get(&graphid).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Ontology {} not found",
                    uri
                ))
            })?;

            let closure = env
                .get_closure(ont.id(), recursion_depth)
                .map_err(anyhow_to_pyerr)?;
            for c_ont in closure {
                all_closure_names.push(c_ont.to_uri_string());
                if seen_ontologies.insert(c_ont.clone()) {
                    all_ontologies.push(c_ont);
                }
            }
        }

        if all_ontologies.is_empty() {
            return Ok((destination_graph, Vec::new()));
        }

        if let Some(ref root_id) = root_graphid {
            promote_root_graphid(&mut all_ontologies, root_id);
        }

        let rewrite_in_rust = rewrite_sh_prefixes && root_graphid.is_some();
        let union = env
            .get_union_graph(
                &all_ontologies,
                Some(rewrite_in_rust),
                Some(remove_owl_imports),
            )
            .map_err(anyhow_to_pyerr)?;

        for triple in union.dataset.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();
            let t = PyTuple::new(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            )?;
            destination_graph.getattr("add")?.call1((t,))?;
        }

        if remove_owl_imports {
            for graphid in union.graph_ids {
                let iri = term_to_python(py, &rdflib, Term::NamedNode(graphid.into()))?;
                let pred = term_to_python(py, &rdflib, IMPORTS.into())?;
                let remove_tuple = PyTuple::new(py, &[py.None(), pred.into(), iri.into()])?;
                destination_graph
                    .getattr("remove")?
                    .call1((remove_tuple,))?;
            }
        }

        if rewrite_sh_prefixes && !rewrite_in_rust {
            if let Some(ref root) = root_subject {
                rewrite_sh_prefixes_rdflib(py, &destination_graph, root)?;
            }
        }

        all_closure_names.sort();
        all_closure_names.dedup();

        Ok((destination_graph, all_closure_names))
    }

    /// Add a new ontology to the OntoEnv
    #[pyo3(signature = (location, overwrite = false, fetch_imports = true, force = false))]
    fn add(
        &self,
        location: &Bound<'_, PyAny>,
        overwrite: bool,
        fetch_imports: bool,
        force: bool,
    ) -> PyResult<String> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;

        let resolved = ontology_location_from_py(location)?;
        if matches!(resolved.location, OntologyLocation::InMemory { .. }) {
            return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "In-memory rdflib graphs cannot be added to the environment",
            ));
        }
        let preferred_name = resolved.preferred_name.clone();
        let location = resolved.location;
        let overwrite_flag: Overwrite = overwrite.into();
        let refresh: RefreshStrategy = force.into();
        let graph_id = if fetch_imports {
            env.add(location, overwrite_flag, refresh)
        } else {
            env.add_no_imports(location, overwrite_flag, refresh)
        }
        .map_err(anyhow_to_pyerr)?;
        let actual_name = graph_id.to_uri_string();
        if let Some(pref) = preferred_name {
            if let Ok(candidate) = NamedNode::new(pref.clone()) {
                if env.resolve(ResolveTarget::Graph(candidate)).is_some() {
                    return Ok(pref);
                }
            }
        }
        Ok(actual_name)
    }

    /// Add a new ontology to the OntoEnv without exploring owl:imports.
    #[pyo3(signature = (location, overwrite = false, force = false))]
    fn add_no_imports(
        &self,
        location: &Bound<'_, PyAny>,
        overwrite: bool,
        force: bool,
    ) -> PyResult<String> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let resolved = ontology_location_from_py(location)?;
        if matches!(resolved.location, OntologyLocation::InMemory { .. }) {
            return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "In-memory rdflib graphs cannot be added to the environment",
            ));
        }
        let preferred_name = resolved.preferred_name.clone();
        let location = resolved.location;
        let overwrite_flag: Overwrite = overwrite.into();
        let refresh: RefreshStrategy = force.into();
        let graph_id = env
            .add_no_imports(location, overwrite_flag, refresh)
            .map_err(anyhow_to_pyerr)?;
        let actual_name = graph_id.to_uri_string();
        if let Some(pref) = preferred_name {
            if let Ok(candidate) = NamedNode::new(pref.clone()) {
                if env.resolve(ResolveTarget::Graph(candidate)).is_some() {
                    return Ok(pref);
                }
            }
        }
        Ok(actual_name)
    }

    /// Get the names of all ontologies that import the given ontology
    fn get_importers(&self, uri: &str) -> PyResult<Vec<String>> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let importers = env.get_importers(&iri).map_err(anyhow_to_pyerr)?;
        let names: Vec<String> = importers.iter().map(|ont| ont.to_uri_string()).collect();
        Ok(names)
    }

    /// Get the ontology metadata with the given URI
    fn get_ontology(&self, uri: &str) -> PyResult<PyOntology> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()))
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Failed to resolve graph for URI: {uri}"
                ))
            })?;
        let ont = env.get_ontology(&graphid).map_err(anyhow_to_pyerr)?;
        Ok(PyOntology { inner: ont })
    }

    /// Get the graph with the given URI as an rdflib.Graph
    fn get_graph(&self, py: Python, uri: &Bound<'_, PyString>) -> PyResult<Py<PyAny>> {
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(pystring_to_string(uri)?)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let graph = {
            let inner = self.inner.clone();
            let guard = inner.lock().unwrap();
            let env = guard.as_ref().ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed")
            })?;
            let graphid = env.resolve(ResolveTarget::Graph(iri)).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Failed to resolve graph for URI: {uri}"
                ))
            })?;

            env.get_graph(&graphid).map_err(anyhow_to_pyerr)?
        };
        let res = rdflib.getattr("Graph")?.call0()?;
        for triple in graph.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();

            let t = PyTuple::new(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            )?;

            res.getattr("add")?.call1((t,))?;
        }
        Ok(res.into())
    }

    /// Get the names of all ontologies in the OntoEnv
    fn get_ontology_names(&self) -> PyResult<Vec<String>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let names: Vec<String> = env.ontologies().keys().map(|k| k.to_uri_string()).collect();
        Ok(names)
    }

    /// Convert the OntoEnv to an in-memory rdflib.Dataset populated with all named graphs
    fn to_rdflib_dataset(&self, py: Python) -> PyResult<Py<PyAny>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let rdflib = py.import("rdflib")?;
        let dataset_cls = rdflib.getattr("Dataset")?;
        let ds = dataset_cls.call0()?;
        let uriref = rdflib.getattr("URIRef")?;

        for (_gid, ont) in env.ontologies().iter() {
            let id_str = ont.id().name().as_str();
            let id_py = uriref.call1((id_str,))?;
            let kwargs = [("identifier", id_py.clone())].into_py_dict(py)?;
            let ctx = ds.getattr("graph")?.call((), Some(&kwargs))?;

            let graph = env.get_graph(ont.id()).map_err(anyhow_to_pyerr)?;
            for t in graph.iter() {
                let s: Term = t.subject.into();
                let p: Term = t.predicate.into();
                let o: Term = t.object.into();
                let triple = PyTuple::new(
                    py,
                    &[
                        term_to_python(py, &rdflib, s)?,
                        term_to_python(py, &rdflib, p)?,
                        term_to_python(py, &rdflib, o)?,
                    ],
                )?;
                ctx.getattr("add")?.call1((triple,))?;
            }
        }

        Ok(ds.into())
    }

    // Config accessors
    fn is_offline(&self) -> PyResult<bool> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        if let Some(env) = guard.as_ref() {
            Ok(env.is_offline())
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    fn set_offline(&mut self, offline: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        if let Some(env) = guard.as_mut() {
            env.set_offline(offline);
            env.save_to_directory().map_err(anyhow_to_pyerr)
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    fn is_strict(&self) -> PyResult<bool> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        if let Some(env) = guard.as_ref() {
            Ok(env.is_strict())
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    fn set_strict(&mut self, strict: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        if let Some(env) = guard.as_mut() {
            env.set_strict(strict);
            env.save_to_directory().map_err(anyhow_to_pyerr)
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    fn requires_ontology_names(&self) -> PyResult<bool> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        if let Some(env) = guard.as_ref() {
            Ok(env.requires_ontology_names())
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    fn set_require_ontology_names(&mut self, require: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        if let Some(env) = guard.as_mut() {
            env.set_require_ontology_names(require);
            env.save_to_directory().map_err(anyhow_to_pyerr)
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    fn resolution_policy(&self) -> PyResult<String> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        if let Some(env) = guard.as_ref() {
            Ok(env.resolution_policy().to_string())
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    fn set_resolution_policy(&mut self, policy: String) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        if let Some(env) = guard.as_mut() {
            env.set_resolution_policy(policy);
            env.save_to_directory().map_err(anyhow_to_pyerr)
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    pub fn store_path(&self) -> PyResult<Option<String>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        if let Some(env) = guard.as_ref() {
            match env.store_path() {
                Some(path) => {
                    let dir = path.parent().unwrap_or(path);
                    Ok(Some(dir.to_string_lossy().to_string()))
                }
                None => Ok(None), // Return None if the path doesn't exist (e.g., temporary env)
            }
        } else {
            Ok(None)
        }
    }

    // Wrapper method to raise error if store_path is None, matching previous panic behavior
    // but providing a Python-level error. Or tests can check for None.
    // Let's keep the Option return type for flexibility and adjust tests.

    pub fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| {
            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap();
            if let Some(env) = guard.as_mut() {
                env.save_to_directory().map_err(anyhow_to_pyerr)?;
                env.flush().map_err(anyhow_to_pyerr)?;
            }
            *guard = None;
            Ok(())
        })
    }

    pub fn flush(&mut self, py: Python<'_>) -> PyResult<()> {
        py.detach(|| {
            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap();
            if let Some(env) = guard.as_mut() {
                env.flush().map_err(anyhow_to_pyerr)
            } else {
                Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "OntoEnv is closed",
                ))
            }
        })
    }
}

#[pymodule]
fn _native(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Initialize logging when the python module is loaded.
    ::ontoenv::api::init_logging();
    // Use try_init to avoid panic if logging is already initialized.
    let _ = env_logger::try_init();

    m.add_class::<OntoEnv>()?;
    m.add_class::<PyOntology>()?;
    m.add_function(wrap_pyfunction!(run_cli, m)?)?;
    // add version attribute
    m.add("version", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
