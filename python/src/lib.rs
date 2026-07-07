use anyhow::{anyhow, Error, Result};
use chrono::prelude::*;
use ontoenv::api::{find_ontoenv_root_from, OntoEnv as OntoEnvRs, ResolveTarget};
use ontoenv::config;
use ontoenv::consts::{IMPORTS, ONTOLOGY, TYPE};
use ontoenv::errors::OfflineRetrievalError;
use ontoenv::io::{GraphIO, StoreStats};
use ontoenv::ontology::{GraphIdentifier, Ontology as OntologyRs, OntologyLocation};
use ontoenv::options::{CacheMode, Overwrite, RefreshStrategy};
use ontoenv::transform;
use ontoenv::util::{get_file_contents, get_url_contents};
use ontoenv::FailedImport;
use ontoenv::ToUriString;
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{
    BlankNode, Graph as OxigraphGraph, GraphName, GraphNameRef, Literal, NamedNode, NamedNodeRef,
    NamedOrBlankNode, NamedOrBlankNodeRef, Quad, Term, TermRef, Triple, TripleRef,
};
use oxigraph::store::Store;
use oxrdf::{Dataset as OxDataset, Variable as OxVariable};
#[cfg(not(feature = "cli"))]
use pyo3::exceptions::PyRuntimeError;
use pyo3::{
    prelude::*,
    types::{IntoPyDict, PyDict, PyList, PySet, PyString, PyStringMethods, PyTuple},
};
use rand::random;
use rdf5d::{DecodedTerm, Pattern, Scope, Snapshot};
use spareval::{QueryEvaluator, QueryResults as SpareQueryResults};
use spargebra::SparqlParser;
use std::borrow::{Borrow, Cow};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

fn anyhow_to_pyerr(e: Error) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
}

fn pyerr_to_anyhow(e: PyErr) -> Error {
    anyhow!(e.to_string())
}

fn r5error_to_pyerr(e: rdf5d::reader::R5Error) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
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

fn rdflib_graph_to_turtle_bytes(graph: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    let py = graph.py();
    let kwargs = [("format", "turtle")].into_py_dict(py)?;
    let serialized = graph.call_method("serialize", (), Some(&kwargs))?;

    if let Ok(bytes) = serialized.extract::<Vec<u8>>() {
        return Ok(bytes);
    }
    if let Ok(text) = serialized.extract::<String>() {
        return Ok(text.into_bytes());
    }

    Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
        "rdflib graph serialization must return bytes or str",
    ))
}

fn add_resolved_to_env(
    env: &mut OntoEnvRs,
    py_location: &Bound<'_, PyAny>,
    resolved: ResolvedLocation,
    overwrite: Overwrite,
    refresh: RefreshStrategy,
    fetch_imports: bool,
    rename: Option<&str>,
) -> PyResult<String> {
    let preferred_name = resolved.preferred_name;
    let location = resolved.location;

    let graph_id = match location {
        OntologyLocation::InMemory { .. } => {
            let bytes = rdflib_graph_to_turtle_bytes(py_location)?;
            if fetch_imports {
                env.add_from_bytes(location, bytes, Some(RdfFormat::Turtle), overwrite, refresh)
            } else {
                env.add_from_bytes_no_imports(
                    location,
                    bytes,
                    Some(RdfFormat::Turtle),
                    overwrite,
                    refresh,
                )
            }
        }
        _ => {
            if fetch_imports {
                env.add(location, overwrite, refresh)
            } else {
                env.add_no_imports(location, overwrite, refresh)
            }
        }
    }
    .map_err(anyhow_to_pyerr)?;

    // Apply rename if requested.
    let graph_id = if let Some(new_iri_str) = rename {
        let new_iri = NamedNode::new(new_iri_str)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        env.rename_graph_iri(&graph_id, new_iri)
            .map_err(anyhow_to_pyerr)?
    } else {
        graph_id
    };

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

/// Extract the objects of every `owl:imports` triple from an rdflib.Graph.
fn extract_imports_from_py_graph(
    py: Python<'_>,
    graph: &Bound<'_, PyAny>,
) -> PyResult<Vec<String>> {
    let rdflib = py.import("rdflib")?;
    let py_imports_pred = term_to_python(py, &rdflib, Term::NamedNode(IMPORTS.into()))?;
    let kwargs = [("predicate", py_imports_pred)].into_py_dict(py)?;
    let objects_iter = graph.call_method("objects", (), Some(&kwargs))?;
    let builtins = py.import("builtins")?;
    let objects_list = builtins.getattr("list")?.call1((objects_iter,))?;
    objects_list.extract::<Vec<String>>()
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
    let mut seen: HashMap<String, String> = HashMap::new();
    let root_decl_iter = graph.call_method1("triples", ((&root_ref, &sh_declare, py.None()),))?;
    for triple in root_decl_iter.try_iter()? {
        let t = triple?;
        let decl = t.get_item(2)?;
        let pref = graph.call_method1("value", (&decl, &sh_prefix, py.None()))?;
        let ns = graph.call_method1("value", (&decl, &sh_namespace, py.None()))?;
        if !pref.is_none() && !ns.is_none() {
            let pv = pyany_to_string(&pref)?;
            let nv = pyany_to_string(&ns)?;
            if !pv.is_empty() && !nv.is_empty() {
                if let Some(existing_ns) = seen.get(&pv) {
                    if *existing_ns != nv {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                            "Conflicting sh:prefix \"{pv}\": namespace \"{existing_ns}\" vs \"{nv}\". \
                            Fix the conflict or disable sh:prefixes rewriting \
                            (CLI: `--no-rewrite-sh-prefixes`, Python: `rewrite_sh_prefixes=False`, \
                            Rust: `rewrite_sh_prefixes=Some(false)`)."
                        )));
                    }
                } else {
                    seen.insert(pv, nv);
                }
            }
        }
    }

    // Move all sh:declare entries to the root ontology, deduplicating by (prefix, namespace).
    let declare_iter = graph.call_method1("triples", ((py.None(), &sh_declare, py.None()),))?;
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
            if !pv.is_empty() && !nv.is_empty() {
                if let Some(existing_ns) = seen.get(&pv) {
                    if *existing_ns != nv {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                            "Conflicting sh:prefix \"{pv}\": namespace \"{existing_ns}\" vs \"{nv}\". \
                            Fix the conflict or disable sh:prefixes rewriting \
                            (CLI: `--no-rewrite-sh-prefixes`, Python: `rewrite_sh_prefixes=False`, \
                            Rust: `rewrite_sh_prefixes=Some(false)`)."
                        )));
                    }
                    // Same prefix, same namespace — deduplicate (skip adding)
                    continue;
                }
                seen.insert(pv, nv);
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

/// Cached handles to the rdflib term constructors. Built once per outer
/// call (per `triples()` invocation, per iterator) and reused for every
/// term — avoids `py.import("rdflib")` + three `getattr` lookups per
/// term, which dominate the per-triple Python construction cost.
struct RdflibCtors {
    uri_ref: Py<PyAny>,
    literal: Py<PyAny>,
    bnode: Py<PyAny>,
}

impl RdflibCtors {
    fn new(py: Python<'_>) -> PyResult<Self> {
        let rdflib = py.import("rdflib")?;
        Ok(Self {
            uri_ref: rdflib.getattr("URIRef")?.unbind(),
            literal: rdflib.getattr("Literal")?.unbind(),
            bnode: rdflib.getattr("BNode")?.unbind(),
        })
    }
}

fn term_to_python<'a>(
    py: Python<'a>,
    rdflib: &Bound<'a, PyModule>,
    node: Term,
) -> PyResult<Bound<'a, PyAny>> {
    let _ = rdflib;
    let ctors = RdflibCtors::new(py)?;
    term_to_python_with(py, &ctors, node)
}

fn term_to_python_with<'a>(
    py: Python<'a>,
    ctors: &RdflibCtors,
    node: Term,
) -> PyResult<Bound<'a, PyAny>> {
    match node {
        Term::NamedNode(uri) => {
            let s = uri.into_string();
            ctors.uri_ref.bind(py).call1((s,))
        }
        Term::Literal(literal) => {
            let lang = literal.language().map(|s| s.to_string());
            let dtype = {
                let dt = literal.datatype().as_str();
                if dt == "http://www.w3.org/2001/XMLSchema#string" {
                    None
                } else {
                    Some(dt.to_string())
                }
            };
            let value = literal.value().to_string();
            let lit = ctors.literal.bind(py);
            match (dtype, lang) {
                (_, Some(lang)) => lit.call1((value, lang, py.None())),
                (Some(dtype), None) => lit.call1((value, py.None(), dtype)),
                (None, None) => lit.call1((value,)),
            }
        }
        Term::BlankNode(id) => ctors.bnode.bind(py).call1((id.into_string(),)),
    }
}

/// Convert a `DecodedTerm` straight to an rdflib Python object, skipping
/// the intermediate oxrdf `Term` allocation chain in `decoded_term_to_term`.
fn decoded_term_to_python_with<'a>(
    py: Python<'a>,
    ctors: &RdflibCtors,
    term: DecodedTerm<'_>,
) -> PyResult<Bound<'a, PyAny>> {
    match term {
        DecodedTerm::Iri(value) => ctors.uri_ref.bind(py).call1((value.as_ref(),)),
        DecodedTerm::BNode(value) => ctors.bnode.bind(py).call1((value.as_ref(),)),
        DecodedTerm::Literal { lex, dt, lang } => {
            let lit = ctors.literal.bind(py);
            match (dt, lang) {
                (_, Some(lang)) => lit.call1((lex.as_ref(), lang.as_ref(), py.None())),
                (Some(dt), None) if dt.as_ref() == "http://www.w3.org/2001/XMLSchema#string" => {
                    lit.call1((lex.as_ref(),))
                }
                (Some(dt), None) => lit.call1((lex.as_ref(), py.None(), dt.as_ref())),
                (None, None) => lit.call1((lex.as_ref(),)),
            }
        }
    }
}

fn term_from_python(node: &Bound<'_, PyAny>) -> Result<Term> {
    let type_name = node.get_type().name().map_err(pyerr_to_anyhow)?.to_string();
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
                Literal::new_language_tagged_literal(value, l)
                    .map_err(|e| anyhow!(e.to_string()))?,
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

fn graph_name_to_python<'a>(
    py: Python<'a>,
    rdflib: &Bound<'a, PyModule>,
    graph_name: GraphName,
) -> PyResult<Bound<'a, PyAny>> {
    match graph_name {
        GraphName::NamedNode(node) => term_to_python(py, rdflib, Term::NamedNode(node)),
        GraphName::BlankNode(node) => term_to_python(py, rdflib, Term::BlankNode(node)),
        GraphName::DefaultGraph => Ok(py.None().into_bound(py)),
    }
}

fn graph_name_to_python_with<'a>(
    py: Python<'a>,
    ctors: &RdflibCtors,
    graph_name: GraphName,
) -> PyResult<Bound<'a, PyAny>> {
    match graph_name {
        GraphName::NamedNode(node) => term_to_python_with(py, ctors, Term::NamedNode(node)),
        GraphName::BlankNode(node) => term_to_python_with(py, ctors, Term::BlankNode(node)),
        GraphName::DefaultGraph => Ok(py.None().into_bound(py)),
    }
}

fn graph_name_from_term(term: Term) -> Result<GraphName> {
    match term {
        Term::NamedNode(node) => Ok(GraphName::NamedNode(node)),
        Term::BlankNode(node) => Ok(GraphName::BlankNode(node)),
        _ => Err(anyhow!("Graph names must be URIRefs or BNodes")),
    }
}

fn context_identifier<'a>(context: &'a Bound<'a, PyAny>) -> PyResult<Bound<'a, PyAny>> {
    if context.hasattr("identifier")? {
        context.getattr("identifier")
    } else {
        Ok(context.clone())
    }
}

fn graph_name_from_python_context(context: Option<&Bound<'_, PyAny>>) -> Result<GraphName> {
    let Some(context) = context else {
        return Ok(GraphName::DefaultGraph);
    };
    if context.is_none() {
        return Ok(GraphName::DefaultGraph);
    }
    let identifier = context_identifier(context).map_err(pyerr_to_anyhow)?;
    if identifier.is_none() {
        return Ok(GraphName::DefaultGraph);
    }
    graph_name_from_term(term_from_python(&identifier)?)
}

fn is_rdflib_default_graph_name(graph_name: &GraphName) -> bool {
    matches!(
        graph_name,
        GraphName::NamedNode(node) if node.as_str() == "urn:x-rdflib:default"
    )
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

fn graph_to_rdflib<'a>(py: Python<'a>, graph: &OxigraphGraph) -> PyResult<Bound<'a, PyAny>> {
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

fn push_python_triple<'a>(
    py: Python<'a>,
    rdflib: &Bound<'a, PyModule>,
    batch: &Bound<'a, PyList>,
    triple: TripleRef<'_>,
) -> PyResult<()> {
    let t = PyTuple::new(
        py,
        &[
            term_to_python(py, rdflib, triple.subject.into())?,
            term_to_python(py, rdflib, triple.predicate.into())?,
            term_to_python(py, rdflib, triple.object.into())?,
        ],
    )?;
    batch.append(t)?;
    Ok(())
}

fn build_transformed_dependency_graph(
    env: &OntoEnvRs,
    graph_ids: &[GraphIdentifier],
    root: NamedOrBlankNodeRef<'_>,
) -> Result<OxigraphGraph> {
    let mut merged = OxigraphGraph::new();
    let mut failures: Vec<String> = Vec::new();

    for graph_id in graph_ids {
        match env.get_graph(graph_id) {
            Ok(graph) => {
                for triple in graph.iter() {
                    merged.insert(triple);
                }
            }
            Err(err) => {
                failures.push(format!("{}: {}", graph_id.to_uri_string(), err));
            }
        }
    }

    if env.is_strict() && !failures.is_empty() {
        return Err(anyhow!(
            "dependency import: {} graph(s) failed to load: {}",
            failures.len(),
            failures.join("; ")
        ));
    }

    for failure in failures {
        eprintln!("Skipping graph in dependency import: {failure}");
    }

    let to_remove: Vec<NamedNodeRef<'_>> = graph_ids.iter().map(|id| id.name()).collect();
    transform::remove_owl_imports_graph(&mut merged, Some(&to_remove));
    transform::remove_ontology_declarations_graph(&mut merged, root);
    Ok(merged)
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
                    method.call1((id, graph_py)).map_err(pyerr_to_anyhow)?;
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
        if !store.hasattr("graph_ids").map_err(pyerr_to_anyhow)? {
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

    fn graph_ids(&self) -> Result<Vec<GraphIdentifier>> {
        self.with_store(|py, store| {
            let ids = self.graph_ids_from_store(py, &store)?;
            ids.into_iter()
                .map(|id| {
                    NamedNode::new(&id)
                        .map(|n| GraphIdentifier::new(n.as_ref()))
                        .map_err(|e| anyhow!(e.to_string()))
                })
                .collect()
        })
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

    /// Returns a copy of the graph for mutable operations.
    ///
    /// If the Python store exposes a `copy_graph(id)` method it is called;
    /// otherwise falls back to `get_graph(id)`. This lets Python stores
    /// distinguish between returning a live view and returning a fresh
    /// mutable copy.
    fn copy_graph(&self, id: &GraphIdentifier) -> Result<OxigraphGraph> {
        let graph_id = id.to_uri_string();
        self.with_store(|py, store| {
            let has_copy = store.hasattr("copy_graph").map_err(pyerr_to_anyhow)?;
            let method = if has_copy { "copy_graph" } else { "get_graph" };
            let graph_obj = store
                .getattr(method)
                .map_err(pyerr_to_anyhow)?
                .call1((graph_id.as_str(),))
                .map_err(pyerr_to_anyhow)?;
            if graph_obj.is_none() {
                return Err(anyhow!("Graph not found: {graph_id}"));
            }
            graph_from_rdflib(py, &graph_obj)
        })
    }

    /// Best-effort union assembled from `copy_graph` rather than the default
    /// `store()`-streaming impl. A custom `graph_store=` keeps its data in
    /// Python and exposes only an empty scratch oxigraph store, so the default
    /// would silently union nothing. Routing through `copy_graph` (which
    /// dispatches to the Python store's `copy_graph` when available, else
    /// `get_graph`) gives `copy_closure`/`copy_union` correct results,
    /// including the prefix/imports/declaration transforms that run on the
    /// returned dataset.
    fn union_graph(&self, ids: &[GraphIdentifier]) -> (OxDataset, Vec<FailedImport>) {
        let mut dataset = OxDataset::new();
        let mut failures: Vec<FailedImport> = Vec::new();
        for id in ids {
            let graph_name = match id.graphname() {
                Ok(gn) => gn,
                Err(e) => {
                    failures.push(FailedImport::new(id.clone(), e.to_string()));
                    continue;
                }
            };
            match self.copy_graph(id) {
                Ok(graph) => {
                    for triple in graph.iter() {
                        dataset.insert(&Quad::new(
                            triple.subject.into_owned(),
                            triple.predicate.into_owned(),
                            triple.object.into_owned(),
                            graph_name.clone(),
                        ));
                    }
                }
                Err(e) => {
                    failures.push(FailedImport::new(id.clone(), e.to_string()));
                }
            }
        }
        (dataset, failures)
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
                let res = store.getattr("num_triples").map_err(pyerr_to_anyhow)?;
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

/// Resolve an oxrdf [`Term`] to its on-disk term id in `snapshot`, or `None`
/// when the term is absent from the dictionary.
fn snapshot_term_id(snapshot: &Snapshot, term: &Term) -> Option<u64> {
    snapshot.file().term_id(&term_to_decoded_term(term))
}

/// The snapshot's logical graphs as rdflib graph-name nodes.
fn snapshot_named_graphs(snapshot: &Snapshot) -> Result<Vec<NamedOrBlankNode>> {
    let mut values = Vec::new();
    for graph_name in snapshot.graph_names() {
        values.push(named_or_blank_node_from_graph_name(graph_name)?);
    }
    Ok(values)
}

/// The storage strategy currently bound to an `OntoEnvStore`.
///
/// `EnvSnapshotMaterialized` holds an in-memory `OxDataset` copy of the env's
/// quads (the `copy` backend). `EnvSnapshotRdf5d` holds an mmap-backed view of
/// a `.ontoenv/store.r5tu` file (the `rdf5d` backend). Rebinding the store
/// (via `refresh_from_env`) swaps the variant.
#[derive(Debug)]
enum RdfLibStoreBackend {
    EnvSnapshotMaterialized {
        dataset: Arc<OxDataset>,
    },
    EnvSnapshotRdf5d {
        #[allow(dead_code)]
        store_path: PathBuf,
        snapshot: Arc<Snapshot>,
    },
}

impl Default for RdfLibStoreBackend {
    fn default() -> Self {
        Self::EnvSnapshotMaterialized {
            dataset: Arc::new(OxDataset::new()),
        }
    }
}

/// Pull every triple in the named graph identified by `id` into `out`.
///
/// Routes through `env.get_graph(id)` (which dispatches to the IO backend's
/// `get_graph`) rather than reading `env.io().store()` directly. Custom
/// `graph_store=` backends keep their data in Python and expose an *empty*
/// scratch oxigraph store, so reading the store directly would silently
/// return nothing for them. The backend `get_graph` path is the one that
/// works for every backend, matching `copy_graph`/`get_closure`.
fn collect_graph_triples_into(
    env: &OntoEnvRs,
    id: &GraphIdentifier,
    out: &mut Vec<Triple>,
) -> Result<()> {
    let graph = env.get_graph(id)?;
    out.extend(graph.iter().map(|t| t.into_owned()));
    Ok(())
}

fn collect_graph_triples(env: &OntoEnvRs, id: &GraphIdentifier) -> Result<Vec<Triple>> {
    let mut out = Vec::new();
    collect_graph_triples_into(env, id, &mut out)?;
    Ok(out)
}

fn named_or_blank_node_from_graph_name(graph_name: &str) -> Result<NamedOrBlankNode> {
    let graph_name = graph_name_from_string(graph_name)?;
    Ok(match graph_name {
        GraphName::NamedNode(node) => NamedOrBlankNode::NamedNode(node),
        GraphName::BlankNode(node) => NamedOrBlankNode::BlankNode(node),
        GraphName::DefaultGraph => return Err(anyhow!("Default graph names are not supported")),
    })
}

fn graph_name_from_string(graph_name: &str) -> Result<GraphName> {
    Ok(GraphName::NamedNode(
        NamedNode::new(graph_name).map_err(|e| anyhow!(e.to_string()))?,
    ))
}

fn graph_name_to_python_from_str<'a>(
    py: Python<'a>,
    rdflib: &Bound<'a, PyModule>,
    graph_name: &str,
) -> PyResult<Bound<'a, PyAny>> {
    graph_name_to_python(
        py,
        rdflib,
        graph_name_from_string(graph_name).map_err(anyhow_to_pyerr)?,
    )
}

fn graph_name_to_python_from_str_with<'a>(
    py: Python<'a>,
    ctors: &RdflibCtors,
    graph_name: &str,
) -> PyResult<Bound<'a, PyAny>> {
    graph_name_to_python_with(
        py,
        ctors,
        graph_name_from_string(graph_name).map_err(anyhow_to_pyerr)?,
    )
}

fn term_to_decoded_term(term: &Term) -> DecodedTerm<'static> {
    match term {
        Term::NamedNode(node) => DecodedTerm::Iri(Cow::Owned(node.as_str().to_string())),
        Term::BlankNode(node) => DecodedTerm::BNode(Cow::Owned(node.as_str().to_string())),
        Term::Literal(literal) => DecodedTerm::Literal {
            lex: Cow::Owned(literal.value().to_string()),
            dt: if literal.datatype().as_str() == "http://www.w3.org/2001/XMLSchema#string" {
                None
            } else {
                Some(Cow::Owned(literal.datatype().as_str().to_string()))
            },
            lang: literal.language().map(|lang| Cow::Owned(lang.to_string())),
        },
    }
}

#[allow(dead_code)]
fn decoded_term_to_term(term: DecodedTerm<'_>) -> Result<Term> {
    Ok(match term {
        DecodedTerm::Iri(value) => {
            Term::NamedNode(NamedNode::new(value.into_owned()).map_err(|e| anyhow!(e.to_string()))?)
        }
        DecodedTerm::BNode(value) => {
            Term::BlankNode(BlankNode::new(value.into_owned()).map_err(|e| anyhow!(e.to_string()))?)
        }
        DecodedTerm::Literal { lex, dt, lang } => match (dt, lang) {
            (Some(dt), None) => Term::Literal(Literal::new_typed_literal(
                lex.into_owned(),
                NamedNode::new(dt.into_owned()).map_err(|e| anyhow!(e.to_string()))?,
            )),
            (None, Some(lang)) => Term::Literal(
                Literal::new_language_tagged_literal(lex.into_owned(), lang.into_owned())
                    .map_err(|e| anyhow!(e.to_string()))?,
            ),
            (None, None) => Term::Literal(Literal::new_simple_literal(lex.into_owned())),
            (Some(_), Some(_)) => return Err(anyhow!("Literal cannot have datatype and language")),
        },
    })
}

fn subject_to_term(subject: &NamedOrBlankNode) -> Term {
    match subject {
        NamedOrBlankNode::NamedNode(node) => Term::NamedNode(node.clone()),
        NamedOrBlankNode::BlankNode(node) => Term::BlankNode(node.clone()),
    }
}

fn graph_name_key(graph_name: &GraphName) -> Option<String> {
    match graph_name {
        GraphName::NamedNode(node) => Some(node.as_str().to_string()),
        GraphName::BlankNode(node) => Some(node.as_str().to_string()),
        GraphName::DefaultGraph => None,
    }
}

fn dataset_named_graphs(dataset: &OxDataset) -> Vec<NamedOrBlankNode> {
    dataset
        .iter()
        .filter_map(|quad| match quad.graph_name.into_owned() {
            GraphName::NamedNode(node) => Some(NamedOrBlankNode::NamedNode(node)),
            GraphName::BlankNode(node) => Some(NamedOrBlankNode::BlankNode(node)),
            GraphName::DefaultGraph => None,
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

fn query_results_to_python(py: Python<'_>, results: SpareQueryResults<'_>) -> PyResult<Py<PyAny>> {
    let rdflib = PyModule::import(py, "rdflib")?;
    let query_mod = py.import("rdflib.query")?;
    let result = query_mod.getattr("Result")?;
    match results {
        SpareQueryResults::Solutions(solutions) => {
            let py_result = result.call1(("SELECT",))?;
            let variables: Vec<String> = solutions
                .variables()
                .iter()
                .map(|variable| variable.as_str().to_string())
                .collect();
            let vars = variables
                .iter()
                .map(|variable| rdflib.getattr("Variable")?.call1((variable,)))
                .collect::<PyResult<Vec<_>>>()?;
            py_result.setattr("vars", vars)?;

            let rows = solutions
                .map(|row| {
                    let row = row.map_err(|e| {
                        PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
                    })?;
                    let bindings = PyDict::new(py);
                    for variable in &variables {
                        if let Some(term) = row.get(variable.as_str()) {
                            bindings.set_item(
                                rdflib.getattr("Variable")?.call1((variable,))?,
                                term_to_python(py, &rdflib, term.clone())?,
                            )?;
                        }
                    }
                    Ok(bindings.unbind())
                })
                .collect::<PyResult<Vec<_>>>()?;
            py_result.setattr("bindings", rows)?;
            Ok(py_result.unbind())
        }
        SpareQueryResults::Boolean(value) => {
            let py_result = result.call1(("ASK",))?;
            py_result.setattr("askAnswer", value)?;
            Ok(py_result.unbind())
        }
        SpareQueryResults::Graph(triples) => {
            let py_result = result.call1(("CONSTRUCT",))?;
            let graph = rdflib.getattr("Graph")?.call0()?;
            for triple in triples {
                let triple = triple
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                graph.call_method1(
                    "add",
                    ((
                        term_to_python(py, &rdflib, triple.subject.into())?,
                        term_to_python(py, &rdflib, triple.predicate.into())?,
                        term_to_python(py, &rdflib, triple.object)?,
                    ),),
                )?;
            }
            py_result.setattr("graph", graph)?;
            Ok(py_result.unbind())
        }
    }
}

macro_rules! apply_query_graph_selection {
    ($prepared:expr, $query_graph:expr, $named_graphs:expr) => {{
        if let Some(query_graph) = $query_graph {
            let is_plain_str = query_graph
                .get_type()
                .name()
                .map(|name| name == "str")
                .unwrap_or(false);
            if is_plain_str {
                let value = query_graph.extract::<String>()?;
                if value == "__UNION__" {
                    $prepared.dataset_mut().set_default_graph(
                        $named_graphs.iter().cloned().map(GraphName::from).collect(),
                    );
                    $prepared
                        .dataset_mut()
                        .set_available_named_graphs($named_graphs.to_vec());
                }
            } else {
                let graph_name =
                    graph_name_from_python_context(Some(query_graph)).map_err(anyhow_to_pyerr)?;
                if matches!(graph_name, GraphName::BlankNode(_))
                    || matches!(graph_name, GraphName::DefaultGraph)
                    || is_rdflib_default_graph_name(&graph_name)
                {
                    $prepared
                        .dataset_mut()
                        .set_default_graph(vec![GraphName::DefaultGraph]);
                    $prepared
                        .dataset_mut()
                        .set_available_named_graphs($named_graphs.to_vec());
                } else {
                    let default_graph = graph_name.clone();
                    let allowed_graph = match graph_name {
                        GraphName::NamedNode(node) => NamedOrBlankNode::NamedNode(node),
                        GraphName::BlankNode(node) => NamedOrBlankNode::BlankNode(node),
                        GraphName::DefaultGraph => unreachable!(),
                    };
                    $prepared
                        .dataset_mut()
                        .set_default_graph(vec![default_graph]);
                    $prepared
                        .dataset_mut()
                        .set_available_named_graphs(vec![allowed_graph]);
                }
            }
        }
    }};
}

#[pyclass(name = "_RdfLibStoreBackend")]
struct PyRdfLibStoreBackend {
    backend: Arc<Mutex<RdfLibStoreBackend>>,
}

#[pymethods]
impl PyRdfLibStoreBackend {
    #[new]
    fn new() -> Self {
        Self {
            backend: Arc::new(Mutex::new(RdfLibStoreBackend::default())),
        }
    }

    /// Bind this store to an in-memory snapshot built from a Python-supplied
    /// iterable of `(subject, predicate, object, context)` rdflib tuples.
    ///
    /// Slower than `bind_env_snapshot` because every term crosses the Python
    /// boundary; retained as a public entry point for callers that already
    /// have quads in Python and don't have a backing `OntoEnv`.
    fn bind_materialized_snapshot(&self, quads: &Bound<'_, PyAny>) -> PyResult<()> {
        let iter = quads.try_iter()?;
        let mut dataset = OxDataset::new();
        for item in iter {
            let item = item?;
            let tuple = item.cast::<PyTuple>()?;
            if tuple.len() != 4 {
                return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "materialized snapshot quads must be (subject, predicate, object, context) tuples",
                ));
            }
            let subject =
                term_to_subject(term_from_python(&tuple.get_item(0)?).map_err(anyhow_to_pyerr)?)
                    .map_err(anyhow_to_pyerr)?;
            let predicate =
                term_to_predicate(term_from_python(&tuple.get_item(1)?).map_err(anyhow_to_pyerr)?)
                    .map_err(anyhow_to_pyerr)?;
            let object = term_from_python(&tuple.get_item(2)?).map_err(anyhow_to_pyerr)?;
            let graph_name = graph_name_from_python_context(Some(&tuple.get_item(3)?))
                .map_err(anyhow_to_pyerr)?;
            dataset.insert(&Quad::new(subject, predicate, object, graph_name));
        }
        let mut backend = self.backend.lock().unwrap();
        *backend = RdfLibStoreBackend::EnvSnapshotMaterialized {
            dataset: Arc::new(dataset),
        };
        Ok(())
    }

    /// Build a materialized snapshot directly from a Python `OntoEnv` without
    /// staging through an rdflib `Dataset`. Each ontology graph is fetched as
    /// an oxigraph `Graph` via the existing IO path and inserted as quads
    /// into the snapshot's `OxDataset` in a single pass.
    fn bind_env_snapshot(&self, env: &Bound<'_, PyAny>) -> PyResult<()> {
        let py_env: PyRef<OntoEnv> = env.extract().map_err(|_| {
            PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "bind_env_snapshot requires an ontoenv.OntoEnv instance",
            )
        })?;
        let inner = py_env.inner.clone();
        drop(py_env);
        let guard = inner.lock().unwrap();
        let env_rs = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;

        let mut dataset = OxDataset::new();
        let ids: Vec<GraphIdentifier> = env_rs.ontologies().keys().cloned().collect();
        for id in ids {
            let graph_name = GraphName::NamedNode(id.name().into_owned());
            // Use get_graph here: bind_env_snapshot is shared by both
            // get_dataset (view) and the OntoEnvStore-backed copy_dataset path.
            // The common copy_dataset() path (plain rdflib.Dataset store) routes
            // through rdflib_store.py's dataset_from_env, which calls env.copy_graph().
            let graph = env_rs.get_graph(&id).map_err(anyhow_to_pyerr)?;
            for triple in graph.iter() {
                let quad = Quad::new(
                    triple.subject,
                    triple.predicate,
                    triple.object,
                    graph_name.clone(),
                );
                dataset.insert(&quad);
            }
        }
        drop(guard);

        let mut backend = self.backend.lock().unwrap();
        *backend = RdfLibStoreBackend::EnvSnapshotMaterialized {
            dataset: Arc::new(dataset),
        };
        Ok(())
    }

    /// Bind this store to an mmap-backed, zero-copy [`Snapshot`] opened from
    /// `store_path` (typically `.ontoenv/store.r5tu`).
    fn bind_rdf5d_snapshot(&self, store_path: &str) -> PyResult<()> {
        let snapshot = Snapshot::open(Path::new(store_path)).map_err(r5error_to_pyerr)?;
        let mut backend = self.backend.lock().unwrap();
        *backend = RdfLibStoreBackend::EnvSnapshotRdf5d {
            store_path: PathBuf::from(store_path),
            snapshot: Arc::new(snapshot),
        };
        Ok(())
    }

    fn backend_kind(&self) -> String {
        let backend = self.backend.lock().unwrap();
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { .. } => "copy".to_string(),
            RdfLibStoreBackend::EnvSnapshotRdf5d { .. } => "rdf5d".to_string(),
        }
    }

    fn is_read_only(&self) -> bool {
        true
    }

    fn add(
        &self,
        subject: &Bound<'_, PyAny>,
        predicate: &Bound<'_, PyAny>,
        object: &Bound<'_, PyAny>,
        context: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let subject = term_to_subject(term_from_python(subject).map_err(anyhow_to_pyerr)?)
            .map_err(anyhow_to_pyerr)?;
        let predicate = term_to_predicate(term_from_python(predicate).map_err(anyhow_to_pyerr)?)
            .map_err(anyhow_to_pyerr)?;
        let object = term_from_python(object).map_err(anyhow_to_pyerr)?;
        let graph_name = graph_name_from_python_context(context).map_err(anyhow_to_pyerr)?;

        let _ = (subject, predicate, object, graph_name);
        Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "This OntoEnvStore is a read-only snapshot",
        ))
    }

    fn remove(
        &self,
        subject: Option<&Bound<'_, PyAny>>,
        predicate: Option<&Bound<'_, PyAny>>,
        object: Option<&Bound<'_, PyAny>>,
        context: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let subject = match subject {
            Some(value) => Some(
                term_to_subject(term_from_python(value).map_err(anyhow_to_pyerr)?)
                    .map_err(anyhow_to_pyerr)?,
            ),
            None => None,
        };
        let predicate = match predicate {
            Some(value) => Some(
                term_to_predicate(term_from_python(value).map_err(anyhow_to_pyerr)?)
                    .map_err(anyhow_to_pyerr)?,
            ),
            None => None,
        };
        let object = object
            .map(|value| term_from_python(value).map_err(anyhow_to_pyerr))
            .transpose()?;
        let graph_name = match context {
            Some(value) => {
                Some(graph_name_from_python_context(Some(value)).map_err(anyhow_to_pyerr)?)
            }
            None => None,
        };

        let _ = (subject, predicate, object, graph_name);
        Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
            "This OntoEnvStore is a read-only snapshot",
        ))
    }

    fn triples(
        &self,
        py: Python<'_>,
        subject: Option<&Bound<'_, PyAny>>,
        predicate: Option<&Bound<'_, PyAny>>,
        object: Option<&Bound<'_, PyAny>>,
        context: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let subject = match subject {
            Some(value) => Some(
                term_to_subject(term_from_python(value).map_err(anyhow_to_pyerr)?)
                    .map_err(anyhow_to_pyerr)?,
            ),
            None => None,
        };
        let predicate = match predicate {
            Some(value) => Some(
                term_to_predicate(term_from_python(value).map_err(anyhow_to_pyerr)?)
                    .map_err(anyhow_to_pyerr)?,
            ),
            None => None,
        };
        let object = object
            .map(|value| term_from_python(value).map_err(anyhow_to_pyerr))
            .transpose()?;
        let graph_name = match context {
            Some(value) => {
                Some(graph_name_from_python_context(Some(value)).map_err(anyhow_to_pyerr)?)
            }
            None => None,
        };

        let backend = self.backend.lock().unwrap();
        let ctors = RdflibCtors::new(py)?;
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => {
                let mut by_triple: HashMap<Triple, Vec<GraphName>> = HashMap::new();
                for quad in dataset.quads_for_pattern(
                    subject.as_ref().map(NamedOrBlankNode::as_ref),
                    predicate.as_ref().map(NamedNode::as_ref),
                    object.as_ref().map(Term::as_ref),
                    graph_name.as_ref().map(GraphName::as_ref),
                ) {
                    let triple = Triple::new(quad.subject, quad.predicate, quad.object);
                    by_triple
                        .entry(triple)
                        .or_default()
                        .push(quad.graph_name.into_owned());
                }

                let py_list = PyList::empty(py);
                for (triple, contexts) in by_triple {
                    let triple_obj = PyTuple::new(
                        py,
                        [
                            term_to_python_with(py, &ctors, triple.subject.into())?,
                            term_to_python_with(py, &ctors, triple.predicate.into())?,
                            term_to_python_with(py, &ctors, triple.object)?,
                        ],
                    )?;
                    let ctx_list = PyList::empty(py);
                    for graph_name in contexts {
                        let v = graph_name_to_python_with(py, &ctors, graph_name)?;
                        ctx_list.append(v)?;
                    }
                    let row = PyTuple::new(py, [triple_obj.into_any(), ctx_list.into_any()])?;
                    py_list.append(row)?;
                }
                Ok(py_list.into_any().unbind())
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                if matches!(graph_name, Some(GraphName::DefaultGraph)) {
                    return Ok(PyList::empty(py).into_any().unbind());
                }
                let mut grouped: HashMap<(u64, u64, u64), Vec<String>> = HashMap::new();
                let subject_id = subject
                    .as_ref()
                    .map(|value| snapshot_term_id(snapshot, &subject_to_term(value)));
                let predicate_id = predicate
                    .as_ref()
                    .map(|value| snapshot_term_id(snapshot, &Term::NamedNode(value.clone())));
                let object_id = object
                    .as_ref()
                    .map(|value| snapshot_term_id(snapshot, value));
                let empty_iter = |py: Python<'_>| -> PyResult<Py<PyAny>> {
                    Py::new(
                        py,
                        StoreTriplesIter {
                            snapshot: snapshot.clone(),
                            grouped: HashMap::new().into_iter(),
                            ctors: RdflibCtors::new(py)?,
                            term_cache: HashMap::new(),
                            graph_cache: HashMap::new(),
                            remaining: 0,
                        },
                    )
                    .map(|p| p.into_any())
                };

                // A bound-but-absent term means no matches.
                if [subject_id, predicate_id, object_id]
                    .iter()
                    .any(|resolved| matches!(resolved, Some(None)))
                {
                    return empty_iter(py);
                }
                let pat = Pattern {
                    s: subject_id.flatten(),
                    p: predicate_id.flatten(),
                    o: object_id.flatten(),
                };

                if let Some(graph_name) = graph_name.as_ref() {
                    let Some(name) = graph_name_key(graph_name) else {
                        return empty_iter(py);
                    };
                    if !snapshot.has_graph(&name) {
                        return empty_iter(py);
                    }
                    for hit in snapshot.scan(pat, Scope::ByName(&name)) {
                        let m = hit.map_err(r5error_to_pyerr)?;
                        grouped
                            .entry((m.s, m.p, m.o))
                            .or_insert_with(|| vec![name.clone()]);
                    }
                } else {
                    for name in snapshot.graph_names() {
                        for hit in snapshot.scan(pat, Scope::ByName(name)) {
                            let m = hit.map_err(r5error_to_pyerr)?;
                            let contexts = grouped.entry((m.s, m.p, m.o)).or_default();
                            if !contexts.iter().any(|c| c == name) {
                                contexts.push(name.to_string());
                            }
                        }
                    }
                }

                let total = grouped.len();
                let iter = StoreTriplesIter {
                    snapshot: snapshot.clone(),
                    grouped: grouped.into_iter(),
                    ctors,
                    term_cache: HashMap::new(),
                    graph_cache: HashMap::new(),
                    remaining: total,
                };
                Ok(Py::new(py, iter)?.into_any())
            }
        }
    }

    fn contexts(
        &self,
        py: Python<'_>,
        subject: Option<&Bound<'_, PyAny>>,
        predicate: Option<&Bound<'_, PyAny>>,
        object: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Vec<Py<PyAny>>> {
        let backend = self.backend.lock().unwrap();
        let rdflib = PyModule::import(py, "rdflib")?;
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => {
                let mut contexts = HashSet::new();
                if let (Some(subject), Some(predicate), Some(object)) = (subject, predicate, object)
                {
                    let subject =
                        term_to_subject(term_from_python(subject).map_err(anyhow_to_pyerr)?)
                            .map_err(anyhow_to_pyerr)?;
                    let predicate =
                        term_to_predicate(term_from_python(predicate).map_err(anyhow_to_pyerr)?)
                            .map_err(anyhow_to_pyerr)?;
                    let object = term_from_python(object).map_err(anyhow_to_pyerr)?;
                    for quad in dataset.quads_for_pattern(
                        Some(subject.as_ref()),
                        Some(predicate.as_ref()),
                        Some(object.as_ref()),
                        None,
                    ) {
                        contexts.insert(quad.graph_name.into_owned());
                    }
                } else {
                    for quad in dataset.iter() {
                        contexts.insert(quad.graph_name.into_owned());
                    }
                }
                contexts
                    .into_iter()
                    .map(|graph_name| {
                        graph_name_to_python(py, &rdflib, graph_name).map(Bound::unbind)
                    })
                    .collect()
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                if let (Some(subject), Some(predicate), Some(object)) = (subject, predicate, object)
                {
                    let subject =
                        term_to_subject(term_from_python(subject).map_err(anyhow_to_pyerr)?)
                            .map_err(anyhow_to_pyerr)?;
                    let predicate =
                        term_to_predicate(term_from_python(predicate).map_err(anyhow_to_pyerr)?)
                            .map_err(anyhow_to_pyerr)?;
                    let object = term_from_python(object).map_err(anyhow_to_pyerr)?;

                    let Some(subject_id) = snapshot_term_id(snapshot, &subject_to_term(&subject))
                    else {
                        return Ok(Vec::new());
                    };
                    let Some(predicate_id) =
                        snapshot_term_id(snapshot, &Term::NamedNode(predicate))
                    else {
                        return Ok(Vec::new());
                    };
                    let Some(object_id) = snapshot_term_id(snapshot, &object) else {
                        return Ok(Vec::new());
                    };
                    let pat = Pattern {
                        s: Some(subject_id),
                        p: Some(predicate_id),
                        o: Some(object_id),
                    };
                    snapshot
                        .names_containing(pat)
                        .map_err(r5error_to_pyerr)?
                        .into_iter()
                        .map(|graph_name| {
                            graph_name_to_python_from_str(py, &rdflib, graph_name)
                                .map(Bound::unbind)
                        })
                        .collect()
                } else {
                    snapshot
                        .graph_names()
                        .map(|graph_name| {
                            graph_name_to_python_from_str(py, &rdflib, graph_name)
                                .map(Bound::unbind)
                        })
                        .collect()
                }
            }
        }
    }

    fn len(&self, context: Option<&Bound<'_, PyAny>>) -> PyResult<usize> {
        let graph_name = match context {
            Some(value) => {
                Some(graph_name_from_python_context(Some(value)).map_err(anyhow_to_pyerr)?)
            }
            None => None,
        };
        let backend = self.backend.lock().unwrap();
        Ok(match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => match graph_name {
                Some(graph_name) => dataset
                    .quads_for_pattern(None, None, None, Some(graph_name.as_ref()))
                    .count(),
                None => dataset.iter().count(),
            },
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => match graph_name {
                Some(graph_name) => {
                    if is_rdflib_default_graph_name(&graph_name)
                        || matches!(graph_name, GraphName::DefaultGraph)
                    {
                        0
                    } else if let Some(name) = graph_name_key(&graph_name) {
                        snapshot
                            .triple_count(Scope::ByName(&name))
                            .map_err(r5error_to_pyerr)?
                    } else {
                        0
                    }
                }
                None => {
                    let mut total = 0usize;
                    for name in snapshot.graph_names() {
                        total += snapshot
                            .triple_count(Scope::ByName(name))
                            .map_err(r5error_to_pyerr)?;
                    }
                    total
                }
            },
        })
    }

    /// Scan a fixed list of named graphs (the imports closure of an
    /// ontology) and yield deduplicated ``(triple, contexts)`` rows.
    ///
    /// Dedup is done at the rdf5d term-ID level — much cheaper than
    /// hashing rdflib term tuples in Python, which is what
    /// ``ClosureGraphView.triples`` used to do before pushing this merge
    /// into Rust.
    fn triples_in_graphs(
        &self,
        py: Python<'_>,
        subject: Option<&Bound<'_, PyAny>>,
        predicate: Option<&Bound<'_, PyAny>>,
        object: Option<&Bound<'_, PyAny>>,
        graph_names: Vec<String>,
    ) -> PyResult<Py<PyAny>> {
        let subject = match subject {
            Some(value) => Some(
                term_to_subject(term_from_python(value).map_err(anyhow_to_pyerr)?)
                    .map_err(anyhow_to_pyerr)?,
            ),
            None => None,
        };
        let predicate = match predicate {
            Some(value) => Some(
                term_to_predicate(term_from_python(value).map_err(anyhow_to_pyerr)?)
                    .map_err(anyhow_to_pyerr)?,
            ),
            None => None,
        };
        let object = object
            .map(|value| term_from_python(value).map_err(anyhow_to_pyerr))
            .transpose()?;

        let backend = self.backend.lock().unwrap();
        let ctors = RdflibCtors::new(py)?;
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => {
                let mut by_triple: HashMap<Triple, Vec<GraphName>> = HashMap::new();
                for name in &graph_names {
                    let gn = match NamedNode::new(name) {
                        Ok(n) => GraphName::NamedNode(n),
                        Err(_) => continue,
                    };
                    for quad in dataset.quads_for_pattern(
                        subject.as_ref().map(NamedOrBlankNode::as_ref),
                        predicate.as_ref().map(NamedNode::as_ref),
                        object.as_ref().map(Term::as_ref),
                        Some(gn.as_ref()),
                    ) {
                        let triple = Triple::new(quad.subject, quad.predicate, quad.object);
                        by_triple
                            .entry(triple)
                            .or_default()
                            .push(quad.graph_name.into_owned());
                    }
                }
                let py_list = PyList::empty(py);
                for (triple, contexts) in by_triple {
                    let triple_obj = PyTuple::new(
                        py,
                        [
                            term_to_python_with(py, &ctors, triple.subject.into())?,
                            term_to_python_with(py, &ctors, triple.predicate.into())?,
                            term_to_python_with(py, &ctors, triple.object)?,
                        ],
                    )?;
                    let ctx_list = PyList::empty(py);
                    for graph_name in contexts {
                        let v = graph_name_to_python_with(py, &ctors, graph_name)?;
                        ctx_list.append(v)?;
                    }
                    let row = PyTuple::new(py, [triple_obj.into_any(), ctx_list.into_any()])?;
                    py_list.append(row)?;
                }
                Ok(py_list.into_any().unbind())
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                let empty_iter = |py: Python<'_>| -> PyResult<Py<PyAny>> {
                    Py::new(
                        py,
                        StoreTriplesIter {
                            snapshot: snapshot.clone(),
                            grouped: HashMap::new().into_iter(),
                            ctors: RdflibCtors::new(py)?,
                            term_cache: HashMap::new(),
                            graph_cache: HashMap::new(),
                            remaining: 0,
                        },
                    )
                    .map(|p| p.into_any())
                };

                // Resolve bound terms to ids once; any bound-but-absent term
                // means the result is empty.
                let resolve = |t: &Term| snapshot_term_id(snapshot, t);
                let subject_id = match subject.as_ref() {
                    Some(v) => match resolve(&subject_to_term(v)) {
                        Some(id) => Some(id),
                        None => return empty_iter(py),
                    },
                    None => None,
                };
                let predicate_id = match predicate.as_ref() {
                    Some(v) => match resolve(&Term::NamedNode(v.clone())) {
                        Some(id) => Some(id),
                        None => return empty_iter(py),
                    },
                    None => None,
                };
                let object_id = match object.as_ref() {
                    Some(v) => match resolve(v) {
                        Some(id) => Some(id),
                        None => return empty_iter(py),
                    },
                    None => None,
                };
                let pat = Pattern {
                    s: subject_id,
                    p: predicate_id,
                    o: object_id,
                };

                let mut grouped: HashMap<(u64, u64, u64), Vec<String>> = HashMap::new();
                for name in &graph_names {
                    if !snapshot.has_graph(name) {
                        continue;
                    }
                    for hit in snapshot.scan(pat, Scope::ByName(name.as_str())) {
                        let m = hit.map_err(r5error_to_pyerr)?;
                        let contexts = grouped.entry((m.s, m.p, m.o)).or_default();
                        if !contexts.iter().any(|c| c == name) {
                            contexts.push(name.clone());
                        }
                    }
                }
                let total = grouped.len();
                let iter = StoreTriplesIter {
                    snapshot: snapshot.clone(),
                    grouped: grouped.into_iter(),
                    ctors,
                    term_cache: HashMap::new(),
                    graph_cache: HashMap::new(),
                    remaining: total,
                };
                Ok(Py::new(py, iter)?.into_any())
            }
        }
    }

    /// Stream every triple across a list of named graphs without
    /// de-duplication or per-row context lists. The right primitive for
    /// ``ClosureGraphView.triples((None, None, None))`` — when the caller
    /// doesn't need pattern matching, dedup, or contexts, this skips the
    /// outer ``HashMap`` and the ``Vec<String>`` per row that
    /// :py:meth:`triples_in_graphs` builds.
    ///
    /// Returns the same `TripleIter` type used by
    /// :py:meth:`OntoEnv.iter_closure_triples`, with the same per-iterator
    /// term cache.
    fn iter_triples_in_graphs(
        &self,
        py: Python<'_>,
        graph_names: Vec<String>,
    ) -> PyResult<Py<PyAny>> {
        let backend = self.backend.lock().unwrap();
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => {
                let mut triples: Vec<Triple> = Vec::new();
                for name in &graph_names {
                    let Ok(nn) = NamedNode::new(name) else {
                        continue;
                    };
                    let gn = GraphName::NamedNode(nn);
                    for quad in dataset.quads_for_pattern(None, None, None, Some(gn.as_ref())) {
                        triples.push(Triple::new(quad.subject, quad.predicate, quad.object));
                    }
                }
                Ok(Py::new(py, TripleIter::new(triples))?.into_any())
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                // Collect term-ID triples eagerly; defer Term/Py construction
                // until iteration so the u64-keyed cache amortizes work
                // across repeated terms (predicates and shared subjects).
                let file = snapshot.file();
                let mut ids: Vec<(u64, u64, u64)> = Vec::new();
                for name in &graph_names {
                    let Some(gids) = snapshot.gids_for_name(name) else {
                        continue;
                    };
                    for &gid in gids {
                        for triple_ids in file.triples_ids(gid).map_err(r5error_to_pyerr)? {
                            ids.push(triple_ids);
                        }
                    }
                }
                let remaining = ids.len();
                let iter = Rdf5dTripleIdIter {
                    snapshot: snapshot.clone(),
                    triples: ids.into_iter(),
                    ctors: RdflibCtors::new(py)?,
                    term_cache: HashMap::new(),
                    remaining,
                };
                Ok(Py::new(py, iter)?.into_any())
            }
        }
    }

    /// Count unique triples across a list of named graphs. Used by
    /// ``ClosureGraphView.__len__``.
    fn len_in_graphs(&self, graph_names: Vec<String>) -> PyResult<usize> {
        let backend = self.backend.lock().unwrap();
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => {
                let mut seen: HashSet<Triple> = HashSet::new();
                for name in &graph_names {
                    let gn = match NamedNode::new(name) {
                        Ok(n) => GraphName::NamedNode(n),
                        Err(_) => continue,
                    };
                    for quad in dataset.quads_for_pattern(None, None, None, Some(gn.as_ref())) {
                        seen.insert(Triple::new(quad.subject, quad.predicate, quad.object));
                    }
                }
                Ok(seen.len())
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                let mut gids: Vec<u64> = Vec::new();
                for name in &graph_names {
                    if let Some(g) = snapshot.gids_for_name(name) {
                        gids.extend_from_slice(g);
                    }
                }
                snapshot
                    .triple_count(Scope::Gids(&gids))
                    .map_err(r5error_to_pyerr)
            }
        }
    }

    /// True if any of ``graph_names`` contains the bound triple.
    fn contains_in_graphs(
        &self,
        subject: &Bound<'_, PyAny>,
        predicate: &Bound<'_, PyAny>,
        object: &Bound<'_, PyAny>,
        graph_names: Vec<String>,
    ) -> PyResult<bool> {
        let subject = term_to_subject(term_from_python(subject).map_err(anyhow_to_pyerr)?)
            .map_err(anyhow_to_pyerr)?;
        let predicate = term_to_predicate(term_from_python(predicate).map_err(anyhow_to_pyerr)?)
            .map_err(anyhow_to_pyerr)?;
        let object = term_from_python(object).map_err(anyhow_to_pyerr)?;

        let backend = self.backend.lock().unwrap();
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => {
                for name in &graph_names {
                    let gn = match NamedNode::new(name) {
                        Ok(n) => GraphName::NamedNode(n),
                        Err(_) => continue,
                    };
                    if dataset
                        .quads_for_pattern(
                            Some(subject.as_ref()),
                            Some(predicate.as_ref()),
                            Some(object.as_ref()),
                            Some(gn.as_ref()),
                        )
                        .next()
                        .is_some()
                    {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                let Some(s_id) = snapshot_term_id(snapshot, &subject_to_term(&subject)) else {
                    return Ok(false);
                };
                let Some(p_id) = snapshot_term_id(snapshot, &Term::NamedNode(predicate)) else {
                    return Ok(false);
                };
                let Some(o_id) = snapshot_term_id(snapshot, &object) else {
                    return Ok(false);
                };
                let pat = Pattern {
                    s: Some(s_id),
                    p: Some(p_id),
                    o: Some(o_id),
                };
                let mut gids: Vec<u64> = Vec::new();
                for name in &graph_names {
                    if let Some(g) = snapshot.gids_for_name(name) {
                        gids.extend_from_slice(g);
                    }
                }
                match snapshot.scan(pat, Scope::Gids(&gids)).next() {
                    Some(Ok(_)) => Ok(true),
                    Some(Err(error)) => Err(r5error_to_pyerr(error)),
                    None => Ok(false),
                }
            }
        }
    }

    fn query(
        &self,
        py: Python<'_>,
        query: &str,
        init_bindings: Option<HashMap<String, Py<PyAny>>>,
        query_graph: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let mut parsed = SparqlParser::new()
            .parse_query(query)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let evaluator = QueryEvaluator::new();
        let backend = self.backend.lock().unwrap();
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => {
                let dataset = (**dataset).clone();
                let named_graphs = dataset_named_graphs(&dataset);
                let mut prepared = evaluator.prepare(&parsed);
                apply_query_graph_selection!(&mut prepared, query_graph, &named_graphs);
                if let Some(init_bindings) = init_bindings {
                    for (variable, term) in init_bindings {
                        prepared = prepared.substitute_variable(
                            OxVariable::new(variable)
                                .map_err(|e| anyhow_to_pyerr(anyhow!(e.to_string())))?,
                            term_from_python(term.bind(py)).map_err(anyhow_to_pyerr)?,
                        );
                    }
                }
                let results = prepared
                    .execute(&dataset)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                query_results_to_python(py, results)
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                // Rewrite property-path closures the snapshot knows about into
                // materialized VALUES blocks before handing the query to spareval.
                snapshot.rewrite_query(&mut parsed);
                let named_graphs = snapshot_named_graphs(snapshot).map_err(anyhow_to_pyerr)?;
                let mut prepared = evaluator.prepare(&parsed);
                apply_query_graph_selection!(&mut prepared, query_graph, &named_graphs);
                if let Some(init_bindings) = init_bindings {
                    for (variable, term) in init_bindings {
                        prepared = prepared.substitute_variable(
                            OxVariable::new(variable)
                                .map_err(|e| anyhow_to_pyerr(anyhow!(e.to_string())))?,
                            term_from_python(term.bind(py)).map_err(anyhow_to_pyerr)?,
                        );
                    }
                }
                let view = snapshot.sparql_view();
                let results = prepared
                    .execute(view)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                query_results_to_python(py, results)
            }
        }
    }

    /// Stream every triple across a list of named graphs without pattern
    /// matching or de-duplication. Thin alias for
    /// :py:meth:`iter_triples_in_graphs` so :py:class:`ontoenv.ViewGraph`
    /// can name its primitive after the view concept rather than the store
    /// concept. The argument is the view's scope (a list of graph IRIs).
    fn iter_triples_scoped(&self, py: Python<'_>, graph_names: Vec<String>) -> PyResult<Py<PyAny>> {
        self.iter_triples_in_graphs(py, graph_names)
    }

    /// Pattern-restricted, de-duplicated triple scan across a list of named
    /// graphs. Returns ``(triple, contexts)`` rows, like
    /// :py:meth:`triples_in_graphs`, but with the scope argument first to
    /// match :py:class:`ontoenv.ViewGraph`'s calling convention.
    fn triples_scoped(
        &self,
        py: Python<'_>,
        graph_names: Vec<String>,
        subject: Option<&Bound<'_, PyAny>>,
        predicate: Option<&Bound<'_, PyAny>>,
        object: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        self.triples_in_graphs(py, subject, predicate, object, graph_names)
    }

    /// Yield the unique subjects of triples matching ``(predicate, object)``
    /// across the scope's named graphs. De-duplication is exact (rdflib term
    /// equality) and reuses the per-backend matching logic from
    /// :py:meth:`triples_in_graphs`.
    fn subjects_scoped(
        &self,
        py: Python<'_>,
        graph_names: Vec<String>,
        predicate: Option<&Bound<'_, PyAny>>,
        object: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        scoped_unique_terms(self, py, graph_names, None, predicate, object, 0)
    }

    /// Yield the unique predicates of triples matching ``(subject, object)``
    /// across the scope's named graphs.
    fn predicates_scoped(
        &self,
        py: Python<'_>,
        graph_names: Vec<String>,
        subject: Option<&Bound<'_, PyAny>>,
        object: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        scoped_unique_terms(self, py, graph_names, subject, None, object, 1)
    }

    /// Yield the unique objects of triples matching ``(subject, predicate)``
    /// across the scope's named graphs.
    fn objects_scoped(
        &self,
        py: Python<'_>,
        graph_names: Vec<String>,
        subject: Option<&Bound<'_, PyAny>>,
        predicate: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        scoped_unique_terms(self, py, graph_names, subject, predicate, None, 2)
    }

    /// Run a SPARQL query scoped to a list of named graphs.
    ///
    /// The query's default graph is set to the union of the scope's named
    /// graphs and ``available_named_graphs`` is restricted to the scope, so
    /// ``GRAPH`` clauses and the default graph both see only the view's
    /// graphs. This is the read-only, scoped equivalent of
    /// :py:meth:`query`.
    fn query_scoped(
        &self,
        py: Python<'_>,
        query: &str,
        graph_names: Vec<String>,
        init_bindings: Option<HashMap<String, Py<PyAny>>>,
    ) -> PyResult<Py<PyAny>> {
        let mut parsed = SparqlParser::new()
            .parse_query(query)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let evaluator = QueryEvaluator::new();
        let backend = self.backend.lock().unwrap();
        match &*backend {
            RdfLibStoreBackend::EnvSnapshotMaterialized { dataset } => {
                let dataset = (**dataset).clone();
                let scoped = filter_named_graphs(&dataset_named_graphs(&dataset), &graph_names);
                let mut prepared = evaluator.prepare(&parsed);
                prepared
                    .dataset_mut()
                    .set_default_graph(scoped.iter().cloned().map(GraphName::from).collect());
                prepared.dataset_mut().set_available_named_graphs(scoped);
                if let Some(init_bindings) = init_bindings {
                    for (variable, term) in init_bindings {
                        prepared = prepared.substitute_variable(
                            OxVariable::new(variable)
                                .map_err(|e| anyhow_to_pyerr(anyhow!(e.to_string())))?,
                            term_from_python(term.bind(py)).map_err(anyhow_to_pyerr)?,
                        );
                    }
                }
                let results = prepared
                    .execute(&dataset)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                query_results_to_python(py, results)
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                // Rewrite property-path closures the snapshot knows about into
                // materialized VALUES blocks before handing the query to spareval.
                snapshot.rewrite_query(&mut parsed);
                let named_graphs = snapshot_named_graphs(snapshot).map_err(anyhow_to_pyerr)?;
                let scoped = filter_named_graphs(&named_graphs, &graph_names);
                let mut prepared = evaluator.prepare(&parsed);
                prepared
                    .dataset_mut()
                    .set_default_graph(scoped.iter().cloned().map(GraphName::from).collect());
                prepared.dataset_mut().set_available_named_graphs(scoped);
                if let Some(init_bindings) = init_bindings {
                    for (variable, term) in init_bindings {
                        prepared = prepared.substitute_variable(
                            OxVariable::new(variable)
                                .map_err(|e| anyhow_to_pyerr(anyhow!(e.to_string())))?,
                            term_from_python(term.bind(py)).map_err(anyhow_to_pyerr)?,
                        );
                    }
                }
                let view = snapshot.sparql_view();
                let results = prepared
                    .execute(view)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                query_results_to_python(py, results)
            }
        }
    }
}

/// Collect the unique term at position ``index`` (0=subject, 1=predicate,
/// 2=object) of every de-duplicated triple matching ``(subject, predicate,
/// object)`` across ``graph_names``.
///
/// Reuses :py:meth:`PyRdfLibStoreBackend::triples_in_graphs` (which already
/// de-duplicates triples per-backend) and collapses to unique terms using a
/// Python ``set``, so rdflib term equality — including datatype and language
/// on literals — is respected exactly.
fn scoped_unique_terms(
    backend: &PyRdfLibStoreBackend,
    py: Python<'_>,
    graph_names: Vec<String>,
    subject: Option<&Bound<'_, PyAny>>,
    predicate: Option<&Bound<'_, PyAny>>,
    object: Option<&Bound<'_, PyAny>>,
    index: usize,
) -> PyResult<Py<PyAny>> {
    let rows = backend.triples_in_graphs(py, subject, predicate, object, graph_names)?;
    let rows_bound = rows.bind(py);
    let set = PySet::empty(py)?;
    for row in rows_bound.try_iter()? {
        let row = row?;
        let triple_tuple = row.get_item(0)?;
        let term = triple_tuple.get_item(index)?;
        set.add(term)?;
    }
    let mut out: Vec<Py<PyAny>> = Vec::new();
    for item in set.iter() {
        out.push(item.into_any().unbind());
    }
    Ok(PyList::new(py, out)?.into_any().unbind())
}

/// Filter ``named_graphs`` down to those whose IRI/blank-node id appears in
/// ``names``, preserving order. Used by :py:meth:`query_scoped` to restrict a
/// query's default graph and available named graphs to the view's scope.
fn filter_named_graphs(
    named_graphs: &[NamedOrBlankNode],
    names: &[String],
) -> Vec<NamedOrBlankNode> {
    let wanted: HashSet<&str> = names.iter().map(|s| s.as_str()).collect();
    named_graphs
        .iter()
        .filter(|n| wanted.contains(named_or_blank_node_str(n)))
        .cloned()
        .collect()
}

/// The IRI or blank-node id of a ``NamedOrBlankNode`` as a ``&str``.
fn named_or_blank_node_str(node: &NamedOrBlankNode) -> &str {
    match node {
        NamedOrBlankNode::NamedNode(n) => n.as_str(),
        NamedOrBlankNode::BlankNode(n) => n.as_str(),
    }
}

/// Construct a :py:class:`ontoenv.ViewGraph` over ``names`` backed by the
/// Rust backend of ``dataset``'s ``OntoEnvStore``.
///
/// ``dataset`` is an ``rdflib.Dataset`` whose ``store`` is an
/// :py:class:`ontoenv.OntoEnvStore`; its ``_backend`` is the
/// :py:class:`ontoenv._native._RdfLibStoreBackend` the view delegates to.
/// The scope (``names``) is passed as a Python ``tuple`` of graph IRI strings.
fn build_view_graph(py: Python<'_>, dataset: &Py<PyAny>, names: &[String]) -> PyResult<Py<PyAny>> {
    let rdflib_store = py.import("ontoenv.rdflib_store")?;
    let backend = dataset
        .bind(py)
        .getattr("store")?
        .getattr("_backend")?
        .unbind();
    let scope_items: Vec<Py<PyAny>> = names
        .iter()
        .map(|n| PyString::new(py, n).into_any().unbind())
        .collect();
    let scope = PyTuple::new(py, scope_items)?.into_any().unbind();
    let view = rdflib_store
        .getattr("ViewGraph")?
        .call1((backend, scope, py.None()))?
        .unbind();
    Ok(view)
}

/// Internal cache for the lazily-built Dataset used by [`OntoEnv::get_graph`].
///
/// Building a Dataset (mmap snapshot or full materialized copy) is the
/// expensive part of `get_graph`. We build it once on first use and reuse it
/// across subsequent calls; any state-mutating method bumps `generation`,
/// which invalidates the cached entry on the next access.
#[derive(Default)]
struct DatasetCache {
    generation: u64,
    cached: Option<(u64, Py<PyAny>)>,
}

#[derive(Debug, PartialEq, Eq)]
struct SnapshotSignature {
    len: u64,
    modified: Option<SystemTime>,
    #[cfg(unix)]
    inode: u64,
}

fn snapshot_signature(path: &Path) -> Option<SnapshotSignature> {
    let metadata = fs::metadata(path).ok()?;
    Some(SnapshotSignature {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        #[cfg(unix)]
        inode: metadata.ino(),
    })
}

/// A high-level API for managing and querying RDF ontologies.
///
/// The OntoEnv class provides methods for:
///
/// * Adding ontologies from files, URLs, or in-memory graphs
/// * Managing imports and resolving the dependency closure
/// * Creating merged views over ontology closures
/// * Managing aliases for ontology IRIs
///
/// Example:
///     .. code-block:: python
///
///         from ontoenv import OntoEnv
///
///         env = OntoEnv()
///         # Add an ontology
///         env.add("http://example.com/ontology.ttl")
///
///         # Add an alias - multiple IRIs can now refer to the same ontology
///         env.add_alias("http://example.com/B-alias", "http://example.com/B")
///
///         # Resolve an alias to its canonical IRI
///         canonical = env.resolve_alias("http://example.com/B-alias")
///
///         # Get the closure (all imported ontologies)
///         graph, iris = env.get_closure("http://example.com/ontology")
///
///     The OntoEnv uses a snapshot-based approach for consistency:
///     operations see a consistent view of the environment, and the
///     underlying store can be updated without affecting existing queries.
#[pyclass]
struct OntoEnv {
    inner: Arc<Mutex<Option<OntoEnvRs>>>,
    cache: Arc<Mutex<DatasetCache>>,
}

/// Streaming iterator over ``(subject, predicate, object)`` tuples of
/// ``rdflib`` terms. Returned by [`OntoEnv::iter_triples`] and
/// [`OntoEnv::iter_closure_triples`].
///
/// Triples are materialized eagerly from the env at construction time; the
/// iterator yields lazily after that, so callers don't pay rdflib ``Graph``
/// overhead per triple. Closure iteration does *not* de-duplicate across
/// named graphs — wrap in ``set()`` if you need set semantics.
/// Streaming iterator returned by ``OntoEnvStore.triples()`` when the
/// underlying backend is rdf5d. Each ``__next__`` yields one
/// ``(triple_tuple, contexts_list)`` row, lazily decoding term IDs into
/// rdflib term objects and caching the result so repeated terms (the
/// common case) are constructed once per scan.
#[pyclass]
struct StoreTriplesIter {
    snapshot: Arc<Snapshot>,
    grouped: std::collections::hash_map::IntoIter<(u64, u64, u64), Vec<String>>,
    ctors: RdflibCtors,
    term_cache: HashMap<u64, Py<PyAny>>,
    graph_cache: HashMap<String, Py<PyAny>>,
    remaining: usize,
}

impl StoreTriplesIter {
    #[allow(clippy::wrong_self_convention)]
    fn to_py_term(&mut self, py: Python<'_>, id: u64) -> PyResult<Py<PyAny>> {
        if let Some(cached) = self.term_cache.get(&id) {
            return Ok(cached.clone_ref(py));
        }
        let decoded = self
            .snapshot
            .file()
            .decoded_term(id)
            .map_err(r5error_to_pyerr)?;
        let obj = decoded_term_to_python_with(py, &self.ctors, decoded)?.unbind();
        self.term_cache.insert(id, obj.clone_ref(py));
        Ok(obj)
    }

    fn graph_to_py(&mut self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        if let Some(cached) = self.graph_cache.get(name) {
            return Ok(cached.clone_ref(py));
        }
        let obj = graph_name_to_python_from_str_with(py, &self.ctors, name)?.unbind();
        self.graph_cache.insert(name.to_string(), obj.clone_ref(py));
        Ok(obj)
    }
}

#[pymethods]
impl StoreTriplesIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyTuple>>> {
        let Some(((s_id, p_id, o_id), contexts)) = self.grouped.next() else {
            return Ok(None);
        };
        self.remaining = self.remaining.saturating_sub(1);
        let s = self.to_py_term(py, s_id)?;
        let p = self.to_py_term(py, p_id)?;
        let o = self.to_py_term(py, o_id)?;
        let triple_obj = PyTuple::new(py, [s, p, o])?;
        let ctx_list = PyList::empty(py);
        for name in &contexts {
            let v = self.graph_to_py(py, name)?;
            ctx_list.append(v)?;
        }
        let row = PyTuple::new(py, [triple_obj.into_any(), ctx_list.into_any()])?;
        Ok(Some(row.unbind()))
    }

    fn __len__(&self) -> usize {
        self.remaining
    }
}

/// Iterator over `(s, p, o)` triples that decodes term IDs against an rdf5d
/// snapshot lazily on each `__next__`, with a u64-keyed term Py-object cache.
/// Used by :py:meth:`PyRdfLibStoreBackend::iter_triples_in_graphs` for the
/// rdf5d backend — preferred over `TripleIter` because it avoids
/// materializing `oxrdf::Term` objects (whose string allocations dominate
/// the cost when the inner cache is u64-keyed).
#[pyclass]
struct Rdf5dTripleIdIter {
    snapshot: Arc<Snapshot>,
    triples: std::vec::IntoIter<(u64, u64, u64)>,
    ctors: RdflibCtors,
    term_cache: HashMap<u64, Py<PyAny>>,
    remaining: usize,
}

impl Rdf5dTripleIdIter {
    #[allow(clippy::wrong_self_convention)]
    fn to_py_term(&mut self, py: Python<'_>, id: u64) -> PyResult<Py<PyAny>> {
        if let Some(cached) = self.term_cache.get(&id) {
            return Ok(cached.clone_ref(py));
        }
        let decoded = self
            .snapshot
            .file()
            .decoded_term(id)
            .map_err(r5error_to_pyerr)?;
        let obj = decoded_term_to_python_with(py, &self.ctors, decoded)?.unbind();
        self.term_cache.insert(id, obj.clone_ref(py));
        Ok(obj)
    }
}

#[pymethods]
impl Rdf5dTripleIdIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyTuple>>> {
        let Some((s_id, p_id, o_id)) = self.triples.next() else {
            return Ok(None);
        };
        self.remaining = self.remaining.saturating_sub(1);
        let s = self.to_py_term(py, s_id)?;
        let p = self.to_py_term(py, p_id)?;
        let o = self.to_py_term(py, o_id)?;
        let tuple = PyTuple::new(py, [s, p, o])?;
        Ok(Some(tuple.unbind()))
    }

    fn __len__(&self) -> usize {
        self.remaining
    }
}

#[pyclass]
struct TripleIter {
    triples: std::vec::IntoIter<Triple>,
    ctors: OnceLock<RdflibCtors>,
    // Reuses rdflib URIRef/Literal/BNode Py-objects across triples that
    // share a term. Ontology closures have ~30k distinct terms across
    // hundreds of thousands of triples, so the hit rate is ~95%+ and the
    // savings on Python object construction are substantial.
    term_cache: HashMap<Term, Py<PyAny>>,
}

impl TripleIter {
    fn new(triples: Vec<Triple>) -> Self {
        Self {
            triples: triples.into_iter(),
            ctors: OnceLock::new(),
            term_cache: HashMap::new(),
        }
    }

    fn ctors(&self, py: Python<'_>) -> PyResult<&RdflibCtors> {
        if let Some(c) = self.ctors.get() {
            return Ok(c);
        }
        let _ = self.ctors.set(RdflibCtors::new(py)?);
        Ok(self.ctors.get().unwrap())
    }
}

#[pymethods]
impl TripleIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyTuple>>> {
        // Initialize ctors lazily before mutably borrowing term_cache.
        let _ = self.ctors(py)?;
        let Some(t) = self.triples.next() else {
            return Ok(None);
        };
        let ctors = self.ctors.get().expect("ctors initialized above");
        // Build the three terms via the per-iterator cache so repeated
        // terms (predicates, common subjects/objects) don't pay
        // URIRef/Literal/BNode construction more than once.
        let s_term: Term = t.subject.into();
        let p_term: Term = t.predicate.into();
        let o_term: Term = t.object;
        let s_obj = cached_term_py(py, ctors, &mut self.term_cache, s_term)?;
        let p_obj = cached_term_py(py, ctors, &mut self.term_cache, p_term)?;
        let o_obj = cached_term_py(py, ctors, &mut self.term_cache, o_term)?;
        let tuple = PyTuple::new(py, &[s_obj, p_obj, o_obj])?;
        Ok(Some(tuple.unbind()))
    }

    fn __len__(&self) -> usize {
        self.triples.len()
    }
}

/// Build the rdflib Py-object for `term`, reusing a previously-constructed
/// one when the same term has already been seen in this iterator.
fn cached_term_py<'py>(
    py: Python<'py>,
    ctors: &RdflibCtors,
    cache: &mut HashMap<Term, Py<PyAny>>,
    term: Term,
) -> PyResult<Bound<'py, PyAny>> {
    if let Some(obj) = cache.get(&term) {
        return Ok(obj.clone_ref(py).into_bound(py));
    }
    let bound = term_to_python_with(py, ctors, term.clone())?;
    cache.insert(term, bound.clone().unbind());
    Ok(bound)
}

impl OntoEnv {
    /// Internal helper that delegates Dataset construction to the Python-side
    /// `ontoenv.rdflib_store.dataset_from_env` factory. Used by
    /// `as_dataset` and the cached `get_graph` path; not exposed to Python.
    fn build_dataset(
        &self,
        py: Python<'_>,
        mode: &str,
        store: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let rdflib_store = py.import("ontoenv.rdflib_store")?;
        let dataset_from_env = rdflib_store.getattr("dataset_from_env")?;
        let kwargs = PyDict::new(py);
        let env_obj = Py::new(
            py,
            OntoEnv {
                inner: self.inner.clone(),
                cache: self.cache.clone(),
            },
        )?;
        kwargs.set_item("env", env_obj)?;
        kwargs.set_item("mode", mode)?;
        if let Some(store) = store {
            kwargs.set_item("store", store)?;
        }
        Ok(dataset_from_env.call((), Some(&kwargs))?.unbind())
    }

    fn current_snapshot_store_path(&self) -> PyResult<Option<PathBuf>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        Ok(env.store_path().map(PathBuf::from))
    }

    fn build_cached_view_dataset(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let Some(store_path) = self.current_snapshot_store_path()? else {
            return self.build_dataset(py, "auto", None);
        };

        let rdflib = py.import("rdflib")?;
        let rdflib_store = py.import("ontoenv.rdflib_store")?;
        let store = rdflib_store.getattr("OntoEnvStore")?.call0()?;
        let store_path = store_path.to_string_lossy();
        store
            .getattr("_backend")?
            .call_method1("bind_rdf5d_snapshot", (store_path.as_ref(),))?;
        store.setattr("_env_mode", "rdf5d")?;
        Ok(rdflib.getattr("Dataset")?.call1((store,))?.unbind())
    }

    /// Mark the cached Dataset (if any) as stale. Subsequent reads via
    /// [`Self::cached_view_dataset`] will rebuild before returning.
    fn bump_generation(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.generation = cache.generation.wrapping_add(1);
    }

    fn cached_view_dataset_exists(&self) -> bool {
        self.cache.lock().unwrap().cached.is_some()
    }

    fn cached_view_dataset_is_stale(&self) -> bool {
        let cache = self.cache.lock().unwrap();
        cache
            .cached
            .as_ref()
            .is_some_and(|(cached_generation, _)| *cached_generation != cache.generation)
    }

    fn mark_cached_view_dataset_current(&self) {
        let mut cache = self.cache.lock().unwrap();
        let generation = cache.generation;
        if let Some((cached_generation, _)) = cache.cached.as_mut() {
            *cached_generation = generation;
        }
    }

    /// Rebind the cached `get_graph` Dataset to the current env snapshot.
    ///
    /// Returned Graph views share this Dataset's store, so refreshing the
    /// cached Dataset updates those existing views without requiring callers
    /// to fetch them again.
    fn refresh_cached_view_dataset(&self, py: Python<'_>) -> PyResult<()> {
        let (target_generation, dataset) = {
            let cache = self.cache.lock().unwrap();
            let Some((_, dataset)) = &cache.cached else {
                return Ok(());
            };
            (cache.generation, dataset.clone_ref(py))
        };

        let snapshot_store_path = self.current_snapshot_store_path()?;
        let rdflib_store = py.import("ontoenv.rdflib_store")?;
        let store_type = rdflib_store.getattr("OntoEnvStore")?;
        let dataset_bound = dataset.bind(py);
        let store = dataset_bound.getattr("store")?;
        if !store.is_instance(&store_type)? {
            return Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
                "cached get_graph Dataset is not backed by OntoEnvStore",
            ));
        }
        if let Some(store_path) = snapshot_store_path {
            let store_path = store_path.to_string_lossy();
            store
                .getattr("_backend")?
                .call_method1("bind_rdf5d_snapshot", (store_path.as_ref(),))?;
            store.setattr("_env_mode", "rdf5d")?;
        } else {
            let env_obj = Py::new(
                py,
                OntoEnv {
                    inner: self.inner.clone(),
                    cache: self.cache.clone(),
                },
            )?;
            store.call_method1("refresh_from_env", (env_obj,))?;
        }

        let mut cache = self.cache.lock().unwrap();
        if cache.generation == target_generation {
            cache.cached = Some((target_generation, dataset));
        }
        Ok(())
    }

    /// Return the cached Dataset used by [`OntoEnv::get_graph`], rebuilding
    /// it if a mutation has happened since the last access. The returned
    /// `Py<PyAny>` is a fresh reference the caller can pass to Python.
    fn cached_view_dataset(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let current_gen = {
            let cache = self.cache.lock().unwrap();
            if let Some((cached_gen, ds)) = &cache.cached {
                if *cached_gen == cache.generation {
                    return Ok(ds.clone_ref(py));
                }
            }
            cache.generation
        };

        // Flush before rebuilding so that the rdf5d snapshot file picked up
        // by backend="auto" reflects any in-memory mutations since the last
        // flush. flush() is a no-op when the env isn't dirty (see io.rs's
        // dirty-flag short-circuit), so this is cheap on read-mostly
        // workloads and avoids silently returning empty graphs for
        // unflushed-add cases.
        {
            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap();
            if let Some(env) = guard.as_mut() {
                env.flush().map_err(anyhow_to_pyerr)?;
            }
        }

        // Build outside the cache lock: dataset construction re-enters Python.
        let dataset = self.build_cached_view_dataset(py)?;

        let mut cache = self.cache.lock().unwrap();
        // Only install if no newer generation snuck in while we were building.
        if cache.generation == current_gen {
            cache.cached = Some((current_gen, dataset.clone_ref(py)));
        }
        Ok(dataset)
    }

    /// Helper to get the inner environment or raise a Python error if closed.
    fn get_env<'a, 'b>(
        &self,
        guard: &'a mut std::sync::MutexGuard<'b, Option<OntoEnvRs>>,
    ) -> PyResult<&'a mut OntoEnvRs> {
        guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))
    }

    fn get_env_ref<'a, 'b>(
        &self,
        guard: &'a std::sync::MutexGuard<'b, Option<OntoEnvRs>>,
    ) -> PyResult<&'a OntoEnvRs> {
        guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))
    }
}

#[pymethods]
impl OntoEnv {
    #[new]
    #[pyo3(signature = (path=None, recreate=false, create_or_use_cached=false, read_only=false, search_directories=None, require_ontology_names=false, strict=false, offline=false, use_cached_ontologies=false, resolution_policy="default".to_owned(), root=".".to_owned(), includes=None, excludes=None, include_ontologies=None, exclude_ontologies=None, temporary=false, remote_cache_ttl_secs=None, graph_store=None, init_from_store=false))]
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
        init_from_store: bool,
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
            let env = if init_from_store {
                OntoEnvRs::new_with_graph_io_from_existing(cfg, Box::new(io))
            } else {
                OntoEnvRs::new_with_graph_io(cfg, Box::new(io))
            }
            .map_err(anyhow_to_pyerr)?;
            let inner = Arc::new(Mutex::new(Some(env)));
            return Ok(OntoEnv {
                inner,
                cache: Arc::new(Mutex::new(DatasetCache::default())),
            });
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
            let mut env =
                OntoEnvRs::load_from_directory(load_root, read_only).map_err(anyhow_to_pyerr)?;
            // Apply any explicitly-activated flags so that e.g.
            // `OntoEnv(offline=True)` makes the loaded env offline.
            // Only non-default values are applied; calling `OntoEnv()` with
            // all defaults is still a pure load with no config side-effects.
            let mut config_changed = false;
            if offline && !env.is_offline() {
                env.set_offline(true);
                config_changed = true;
            }
            if strict && !env.is_strict() {
                env.set_strict(true);
                config_changed = true;
            }
            if require_ontology_names && !env.requires_ontology_names() {
                env.set_require_ontology_names(true);
                config_changed = true;
            }
            if let Some(ttl) = remote_cache_ttl_secs {
                env.set_remote_cache_ttl_secs(ttl);
                config_changed = true;
            }
            if !read_only && config_changed {
                env.save_to_directory().map_err(anyhow_to_pyerr)?;
            }
            env
        };

        let inner = Arc::new(Mutex::new(Some(env)));

        Ok(OntoEnv {
            inner: inner.clone(),
            cache: Arc::new(Mutex::new(DatasetCache::default())),
        })
    }

    #[pyo3(signature = (all=false))]
    fn update(&self, all: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let Some(env) = guard.as_mut() else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ));
        };
        // update_all and save_to_directory both touch env state. Even on
        // failure we may have partially mutated it (e.g. removed missing
        // ontologies before erroring), so invalidate the cache regardless.
        let result = env
            .update_all(all)
            .and_then(|_| env.save_to_directory())
            .map_err(anyhow_to_pyerr);
        drop(guard);
        self.bump_generation();
        result
    }

    /// Re-reads all graphs from the attached graph store and rebuilds the environment's
    /// ontology metadata and import dependency graph.  Call this whenever the graph store
    /// has been mutated externally and you need OntoEnv's view to catch up.
    fn refresh_from_store(&self) -> PyResult<()> {
        let mut guard = self.inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let result = env.refresh_from_graph_io().map_err(anyhow_to_pyerr);
        drop(guard);
        self.bump_generation();
        result
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
        transform::rewrite_sh_prefixes_graph(&mut merged, root_nb).map_err(anyhow_to_pyerr)?;
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

    /// List the ontologies in the imports closure of the given ontology.
    ///
    /// ``uri`` may be either:
    ///
    /// * a string IRI of an ontology already in the environment, or
    /// * an ``rdflib.Graph`` that has not yet been added to the environment.
    ///   In that case the closure is computed from the graph's ``owl:imports``
    ///   declarations without modifying the environment.
    #[pyo3(signature = (uri, recursion_depth = -1))]
    fn list_closure(
        &self,
        py: Python,
        uri: &Bound<'_, PyAny>,
        recursion_depth: i32,
    ) -> PyResult<Vec<String>> {
        // --- string / URI path (existing behaviour) ---
        if let Ok(uri_str) = uri.extract::<String>() {
            let iri = NamedNode::new(&uri_str)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap();
            let env = guard.as_mut().ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed")
            })?;
            let graphid = env
                .resolve(ResolveTarget::Graph(iri.clone()))
                .ok_or_else(|| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to resolve graph for URI: {uri_str}"
                    ))
                })?;
            let ont = env.ontologies().get(&graphid).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology {iri} not found"))
            })?;
            let closure = env
                .get_closure(ont.id(), recursion_depth)
                .map_err(anyhow_to_pyerr)?;
            return Ok(closure.iter().map(|id| id.to_uri_string()).collect());
        }

        // --- rdflib.Graph path (transient graph not in env) ---
        if uri.hasattr("subjects")? {
            let root_iri = extract_ontology_subject(uri)?;
            let imports = extract_imports_from_py_graph(py, uri)?;

            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap();
            let env = guard.as_mut().ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed")
            })?;

            let mut seen: HashSet<String> = HashSet::new();
            let mut names: Vec<String> = Vec::new();

            // The transient graph itself leads the list (mirrors the URI-based behaviour).
            if let Some(ref root) = root_iri {
                seen.insert(root.clone());
                names.push(root.clone());
            }

            for import_uri in &imports {
                let iri = NamedNode::new(import_uri.as_str())
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                if let Some(graphid) = env.resolve(ResolveTarget::Graph(iri)) {
                    let sub = env
                        .get_closure(&graphid, recursion_depth)
                        .map_err(anyhow_to_pyerr)?;
                    for id in sub {
                        let n = id.to_uri_string();
                        if seen.insert(n.clone()) {
                            names.push(n);
                        }
                    }
                }
            }
            return Ok(names);
        }

        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "uri must be a string IRI or an rdflib.Graph",
        ))
    }

    /// Copy the imports closure of `uri` into a mutable graph and return it
    /// alongside the closure list.
    ///
    /// The first element of the returned tuple is either the provided `graph`
    /// after mutation or a brand-new `rdflib.Graph`. The second element is an
    /// ordered list of ontology IRIs in the resolved closure starting with
    /// `uri`. Set `rewrite_sh_prefixes` or `remove_owl_imports` to control
    /// post-processing of the merged triples.
    ///
    /// Works correctly with custom ``graph_store=`` backends: each graph in
    /// the closure is fetched via the backend's ``get_graph`` rather than the
    /// internal oxigraph store.
    #[pyo3(signature = (uri, graph=None, rewrite_sh_prefixes=true, remove_owl_imports=true, recursion_depth=-1))]
    fn copy_closure<'a>(
        &self,
        py: Python<'a>,
        uri: &str,
        graph: Option<&Bound<'a, PyAny>>,
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
        // if graph is null, create a new rdflib.Graph()
        let destination_graph = match graph {
            Some(g) => g.clone(),
            None => rdflib.getattr("Graph")?.call0()?,
        };
        let root_node = iri.as_ref();
        let union = env
            .get_union_graph(
                &closure,
                root_node,
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

        Ok((destination_graph, closure_names))
    }

    /// Copy the union of explicitly listed ontology graphs into a mutable graph.
    ///
    /// Set ``include_closures=True`` to include each listed graph's transitive
    /// ``owl:imports`` closure. The ``root`` IRI is used for ontology
    /// declaration cleanup and optional SHACL prefix rewriting; it does not
    /// need to be one of the listed graphs.
    ///
    /// Use :py:meth:`get_union` for a read-only store-backed view that avoids
    /// materializing triples.
    ///
    /// Works correctly with custom ``graph_store=`` backends: each graph is
    /// fetched via the backend's ``copy_graph`` when available, otherwise
    /// ``get_graph``.
    #[pyo3(signature = (uris, root, graph=None, include_closures=false, rewrite_sh_prefixes=true, remove_owl_imports=true, recursion_depth=-1))]
    #[allow(clippy::too_many_arguments)]
    fn copy_union<'a>(
        &self,
        py: Python<'a>,
        uris: Vec<String>,
        root: &str,
        graph: Option<&Bound<'a, PyAny>>,
        include_closures: bool,
        rewrite_sh_prefixes: bool,
        remove_owl_imports: bool,
        recursion_depth: i32,
    ) -> PyResult<(Bound<'a, PyAny>, Vec<String>)> {
        let rdflib = py.import("rdflib")?;
        let graph_iris: Vec<NamedNode> = uris
            .iter()
            .map(|uri| {
                NamedNode::new(uri.as_str())
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
            })
            .collect::<PyResult<_>>()?;
        let root_node = NamedNode::new(root)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let union = env
            .get_explicit_union_graph(
                &graph_iris,
                root_node.as_ref(),
                include_closures,
                recursion_depth,
                Some(rewrite_sh_prefixes),
                Some(remove_owl_imports),
            )
            .map_err(anyhow_to_pyerr)?;
        let union_names: Vec<String> = union
            .graph_ids
            .iter()
            .map(|graph_id| graph_id.to_uri_string())
            .collect();

        let destination_graph = match graph {
            Some(g) => g.clone(),
            None => rdflib.getattr("Graph")?.call0()?,
        };
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
            for graph_id in union.graph_ids {
                let iri = term_to_python(py, &rdflib, Term::NamedNode(graph_id.into()))?;
                let pred = term_to_python(py, &rdflib, IMPORTS.into())?;
                let remove_tuple = PyTuple::new(py, &[py.None(), pred.into(), iri.into()])?;
                destination_graph
                    .getattr("remove")?
                    .call1((remove_tuple,))?;
            }
        }
        Ok((destination_graph, union_names))
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
        let (root_subject, _root_graphid) = resolve_root_subject_and_graphid(graph, env)?;

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

        // Use the first ontology's name as the explicit root for ontology
        // declaration cleanup. sh:prefixes are still rewritten in Python after
        // merging so the caller's original graph participates in conflict
        // detection and declaration movement.
        let rust_root = NamedOrBlankNodeRef::NamedNode(all_ontologies[0].name());
        let union_graph = build_transformed_dependency_graph(env, &all_ontologies, rust_root)
            .map_err(anyhow_to_pyerr)?;

        let add_triples_to_graph = py
            .import("ontoenv.rdflib_store")?
            .getattr("add_triples_to_graph")?;
        let mut batch = PyList::empty(py);
        for triple in union_graph.iter() {
            push_python_triple(py, &rdflib, &batch, triple)?;
            if batch.len() >= 4096 {
                add_triples_to_graph.call1((graph, &batch))?;
                batch = PyList::empty(py);
            }
        }
        if !batch.is_empty() {
            add_triples_to_graph.call1((graph, &batch))?;
        }

        // Remove all owl:imports from the original graph (they are now materialized).
        let py_imports_pred_for_remove = term_to_python(py, &rdflib, IMPORTS.into())?;
        let remove_tuple = PyTuple::new(
            py,
            &[py.None(), py_imports_pred_for_remove.into(), py.None()],
        )?;
        graph.getattr("remove")?.call1((remove_tuple,))?;

        // Rewrite sh:prefixes in the merged rdflib graph using the known root.
        if let Some(ref root) = root_subject {
            rewrite_sh_prefixes_rdflib(py, graph, root)?;
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
    /// returned as a new graph. The original `graph` is never modified.
    ///
    /// Args:
    ///     graph (rdflib.Graph): The graph to find dependencies for.
    ///     graph_name (Optional[str]): If provided, all ``owl:Ontology`` declarations
    ///         from the closure are replaced with a single one for this URI and
    ///         ``sh:prefixes`` are rewritten to point at it. If ``None`` (default),
    ///         each ontology in the closure retains its own ``owl:Ontology``
    ///         declaration (a proper union of the closure graphs); ``sh:prefixes``
    ///         are left distributed and no root is imposed.
    ///     recursion_depth (int): The maximum depth for recursive import resolution. A
    ///         negative value (default) means no limit.
    ///     fetch_missing (bool): If True, will fetch ontologies that are not in the environment.
    ///
    /// Returns:
    ///     tuple[rdflib.Graph, list[str]]: A tuple containing the populated dependency graph and the sorted list of
    ///     imported ontology IRIs.
    #[pyo3(signature = (graph, graph_name=None, recursion_depth=-1, fetch_missing=false))]
    fn get_dependencies<'a>(
        &self,
        py: Python<'a>,
        graph: &Bound<'a, PyAny>,
        graph_name: Option<&str>,
        recursion_depth: i32,
        fetch_missing: bool,
    ) -> PyResult<(Bound<'a, PyAny>, Vec<String>)> {
        let rdflib = py.import("rdflib")?;
        let py_imports_pred = term_to_python(py, &rdflib, Term::NamedNode(IMPORTS.into()))?;

        let kwargs = [("predicate", py_imports_pred)].into_py_dict(py)?;
        let objects_iter = graph.call_method("objects", (), Some(&kwargs))?;
        let builtins = py.import("builtins")?;
        let objects_list = builtins.getattr("list")?.call1((objects_iter,))?;
        let imports: Vec<String> = objects_list.extract()?;

        let destination_graph = rdflib.getattr("Graph")?.call0()?;

        if imports.is_empty() {
            return Ok((destination_graph, Vec::new()));
        }

        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;

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

        // Use the first ontology as an internal root for get_union_graph mechanics.
        // sh:prefixes rewriting is skipped here; we do it ourselves below if graph_name is set.
        // owl:imports are always removed since they are fully materialized.
        let rust_root = all_ontologies[0].name();
        let union = env
            .get_union_graph(&all_ontologies, rust_root, Some(false), Some(true))
            .map_err(anyhow_to_pyerr)?;

        // get_union_graph collapses all owl:Ontology declarations onto its internal root.
        // We always strip that collapsed declaration and then restore the right set below.
        for triple in union.dataset.into_iter() {
            if triple.predicate == TYPE && triple.object == TermRef::NamedNode(ONTOLOGY) {
                continue;
            }
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

        let type_term = term_to_python(py, &rdflib, Term::NamedNode(TYPE.into_owned()))?;
        let ontology_term = term_to_python(py, &rdflib, Term::NamedNode(ONTOLOGY.into_owned()))?;

        match graph_name {
            Some(gn) => {
                // Named result: single owl:Ontology declaration for graph_name, sh:prefixes rewritten onto it.
                let gn_node = NamedNode::new(gn)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                let decl_triple = PyTuple::new(
                    py,
                    &[
                        term_to_python(py, &rdflib, Term::NamedNode(gn_node))?,
                        type_term,
                        ontology_term,
                    ],
                )?;
                destination_graph.getattr("add")?.call1((decl_triple,))?;
                rewrite_sh_prefixes_rdflib(py, &destination_graph, gn)?;
            }
            None => {
                // Anonymous result: restore each closure ontology's own declaration so the
                // result is a proper union of the closure graphs, not an ownerless bag.
                for gid in &all_ontologies {
                    let decl_triple = PyTuple::new(
                        py,
                        &[
                            term_to_python(py, &rdflib, Term::NamedNode(gid.name().into_owned()))?,
                            type_term.clone(),
                            ontology_term.clone(),
                        ],
                    )?;
                    destination_graph.getattr("add")?.call1((decl_triple,))?;
                }
            }
        }

        all_closure_names.sort();
        all_closure_names.dedup();

        Ok((destination_graph, all_closure_names))
    }

    /// Add a new ontology to the OntoEnv
    #[pyo3(signature = (location, overwrite = false, fetch_imports = true, force = false, rename = None))]
    fn add(
        &self,
        location: &Bound<'_, PyAny>,
        overwrite: bool,
        fetch_imports: bool,
        force: bool,
        rename: Option<&str>,
    ) -> PyResult<String> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;

        let resolved = ontology_location_from_py(location)?;
        let overwrite_flag: Overwrite = overwrite.into();
        let refresh: RefreshStrategy = force.into();
        let result = add_resolved_to_env(
            env,
            location,
            resolved,
            overwrite_flag,
            refresh,
            fetch_imports,
            rename,
        );
        drop(guard);
        self.bump_generation();
        result
    }

    /// Add a new ontology to the OntoEnv without exploring owl:imports.
    #[pyo3(signature = (location, overwrite = false, force = false, rename = None))]
    fn add_no_imports(
        &self,
        location: &Bound<'_, PyAny>,
        overwrite: bool,
        force: bool,
        rename: Option<&str>,
    ) -> PyResult<String> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let resolved = ontology_location_from_py(location)?;
        let overwrite_flag: Overwrite = overwrite.into();
        let refresh: RefreshStrategy = force.into();
        let result = add_resolved_to_env(
            env,
            location,
            resolved,
            overwrite_flag,
            refresh,
            false,
            rename,
        );
        drop(guard);
        self.bump_generation();
        result
    }

    /// Rename the IRI of an ontology already in the environment.
    ///
    /// Reads the stored graph for ``uri``, rewrites every occurrence of its
    /// current IRI to ``new_iri`` (subject and object positions, excluding
    /// ``owl:versionIRI`` values), stores the result under the new name,
    /// removes the old named graph, and rebuilds the import dependency graph.
    ///
    /// Returns the new IRI string.
    fn rename_graph_iri(&self, uri: &str, new_iri: &str) -> PyResult<String> {
        let old_iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let new_iri_node = NamedNode::new(new_iri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let graph_id = env.resolve(ResolveTarget::Graph(old_iri)).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology not found: {uri}"))
        })?;
        let new_id = env
            .rename_graph_iri(&graph_id, new_iri_node)
            .map_err(anyhow_to_pyerr)?;
        drop(guard);
        self.bump_generation();
        Ok(new_id.to_uri_string())
    }

    /// Add an alias for a canonical ontology IRI.
    ///
    /// The alias will route to the same graph as the canonical IRI.
    /// Aliases only point to canonical IRIs (not other aliases) to avoid chains.
    ///
    /// Args:
    ///     alias_iri: The alias IRI to add
    ///     canonical_iri: The canonical IRI that the alias should point to
    ///
    /// Example:
    ///     .. code-block:: python
    ///
    ///         env.add_alias("http://example.com/B-alias", "http://example.com/B")
    ///
    ///     After this, ``env.get_graph("http://example.com/B-alias")`` will return
    ///     the same graph as ``env.get_graph("http://example.com/B")``.
    fn add_alias(&self, alias_iri: &str, canonical_iri: &str) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = self.get_env(&mut guard)?;
        env.add_alias(alias_iri, canonical_iri)
            .map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    /// Remove an alias.
    ///
    /// Args:
    ///     alias_iri: The alias IRI to remove
    fn remove_alias(&self, alias_iri: &str) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        let env = self.get_env(&mut guard)?;
        env.remove_alias(alias_iri).map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    /// Get the canonical IRI for an alias.
    ///
    /// Args:
    ///     alias_iri: The alias IRI to resolve
    ///
    /// Returns:
    ///     The canonical IRI if the input is an alias, or None if it's not an alias
    fn resolve_alias(&self, alias_iri: &str) -> PyResult<Option<String>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = self.get_env_ref(&guard)?;
        Ok(env.resolve_alias(alias_iri).map(|id| id.to_uri_string()))
    }

    /// List all aliases that point to a given canonical IRI.
    ///
    /// Args:
    ///     canonical_iri: The canonical IRI to list aliases for
    ///
    /// Returns:
    ///     A list of all alias IRIs that point to the given canonical IRI
    fn get_aliases_for(&self, canonical_iri: &str) -> PyResult<Vec<String>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = self.get_env_ref(&guard)?;
        Ok(env.get_aliases_for(canonical_iri))
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

    /// Return IRIs of imports that cannot be resolved in the environment.
    ///
    /// ``uri`` may be:
    ///
    /// * ``None`` — scans every ontology in the environment and returns the
    ///   union of all unresolvable imports (de-duplicated).
    /// * a string IRI of an ontology already in the environment — walks its
    ///   full transitive closure and returns every unresolvable import IRI.
    /// * an ``rdflib.Graph`` not yet in the environment — extracts its direct
    ///   ``owl:imports``, then for each import that *is* in the environment
    ///   walks that import's closure.  Imports absent from the environment
    ///   are themselves reported as missing.
    #[pyo3(signature = (uri = None))]
    fn missing_imports(&self, py: Python, uri: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<String>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;

        let Some(uri) = uri else {
            // No argument: return all missing across the whole environment.
            return Ok(env
                .missing_imports()
                .into_iter()
                .map(|n| n.to_uri_string())
                .collect());
        };

        // --- string / URI path (existing behaviour) ---
        if let Ok(uri_str) = uri.extract::<String>() {
            let iri = NamedNode::new(&uri_str)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let graphid = env
                .resolve(ResolveTarget::Graph(iri.clone()))
                .ok_or_else(|| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to resolve graph for URI: {uri_str}"
                    ))
                })?;
            let missing = env
                .missing_imports_in_closure(&graphid)
                .map_err(anyhow_to_pyerr)?;
            return Ok(missing.into_iter().map(|n| n.to_uri_string()).collect());
        }

        // --- rdflib.Graph path (transient graph not in env) ---
        if uri.hasattr("subjects")? {
            let imports = extract_imports_from_py_graph(py, uri)?;
            let mut missing: HashSet<String> = HashSet::new();
            for import_uri in &imports {
                let iri = NamedNode::new(import_uri.as_str())
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                match env.resolve(ResolveTarget::Graph(iri)) {
                    None => {
                        // Not in environment — it's missing.
                        missing.insert(import_uri.clone());
                    }
                    Some(graphid) => {
                        // In environment — collect missing from its closure.
                        let sub = env
                            .missing_imports_in_closure(&graphid)
                            .map_err(anyhow_to_pyerr)?;
                        missing.extend(sub.into_iter().map(|n| n.to_uri_string()));
                    }
                }
            }
            return Ok(missing.into_iter().collect());
        }

        Err(PyErr::new::<pyo3::exceptions::PyTypeError, _>(
            "uri must be None, a string IRI, or an rdflib.Graph",
        ))
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

    /// Get a read-only store-backed view of the named graph as an
    /// ``rdflib.Graph``. Mutation raises ``ValueError``; use
    /// :py:meth:`copy_graph` for a mutable in-memory copy.
    fn get_graph(&self, py: Python, uri: &Bound<'_, PyString>) -> PyResult<Py<PyAny>> {
        let uri_string = pystring_to_string(uri)?;
        // Resolve up front so an unknown URI raises ValueError rather than
        // silently returning an empty Dataset.graph(URIRef(uri)) view. The
        // dataset is keyed by canonical GraphIdentifier; use the resolved id
        // for the lookup so aliases/source URLs route to the right named graph.
        let resolved_uri = {
            let iri = NamedNode::new(uri_string.clone())
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let inner = self.inner.clone();
            let guard = inner.lock().unwrap();
            let env = guard.as_ref().ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed")
            })?;
            env.resolve(ResolveTarget::Graph(iri))
                .ok_or_else(|| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "No graph with URI: {uri_string}"
                    ))
                })?
                .to_uri_string()
        };
        let rdflib = py.import("rdflib")?;
        let dataset = self.cached_view_dataset(py)?;
        let uri_ref = rdflib.getattr("URIRef")?.call1((resolved_uri,))?;
        Ok(dataset.bind(py).call_method1("graph", (uri_ref,))?.unbind())
    }

    /// Return a read-only merged view over the transitive ``owl:imports``
    /// closure of *uri*, plus the list of ontology IRIs that contribute to
    /// the view (root first, in BFS order).
    ///
    /// The returned object is a :py:class:`ontoenv.ViewGraph` — a lightweight,
    /// non-``rdflib.Graph`` view that delegates triple-pattern lookups to the
    /// Rust backend scoped to the closure's named graphs, so it behaves like
    /// a merged graph without materializing one. Mutation raises
    /// ``ValueError``; use :py:meth:`copy_closure` for a mutable in-memory
    /// merge.
    #[pyo3(signature = (uri, recursion_depth = -1))]
    fn get_closure(
        &self,
        py: Python<'_>,
        uri: &str,
        recursion_depth: i32,
    ) -> PyResult<(Py<PyAny>, Vec<String>)> {
        let names = {
            let iri = NamedNode::new(uri)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let inner = self.inner.clone();
            let guard = inner.lock().unwrap();
            let env = guard.as_ref().ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed")
            })?;
            let graphid = env.resolve(ResolveTarget::Graph(iri)).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("No graph with URI: {uri}"))
            })?;
            let closure = env
                .get_closure(&graphid, recursion_depth)
                .map_err(anyhow_to_pyerr)?;
            closure
                .into_iter()
                .map(|id| id.to_uri_string())
                .collect::<Vec<_>>()
        };

        let dataset = self.cached_view_dataset(py)?;
        let view = build_view_graph(py, &dataset, &names)?;
        Ok((view, names))
    }

    /// Return a read-only merged view over an explicitly listed set of
    /// ontology graphs, plus the list of ontology IRIs that contribute to
    /// the view (in the order they were resolved).
    ///
    /// The returned object is a :py:class:`ontoenv.ViewGraph` — a lightweight,
    /// non-``rdflib.Graph`` view that delegates triple-pattern lookups to the
    /// Rust backend scoped to the listed named graphs, so it behaves like a
    /// merged graph without materializing one. Mutation raises
    /// ``ValueError``; use :py:meth:`copy_union` for a mutable in-memory merge.
    ///
    /// Set ``include_closures=True`` to expand each listed graph's transitive
    /// ``owl:imports`` closure into the view. This is the read-only equivalent
    /// of :py:meth:`copy_union`.
    #[pyo3(signature = (uris, include_closures = false, recursion_depth = -1))]
    fn get_union(
        &self,
        py: Python<'_>,
        uris: Vec<String>,
        include_closures: bool,
        recursion_depth: i32,
    ) -> PyResult<(Py<PyAny>, Vec<String>)> {
        let names = {
            let graph_iris: Vec<NamedNode> = uris
                .iter()
                .map(|uri| {
                    NamedNode::new(uri.as_str())
                        .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))
                })
                .collect::<PyResult<_>>()?;

            let inner = self.inner.clone();
            let guard = inner.lock().unwrap();
            let env = guard.as_ref().ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed")
            })?;

            let mut names: Vec<String> = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            for iri in &graph_iris {
                let id = env
                    .resolve(ResolveTarget::Graph(iri.clone()))
                    .ok_or_else(|| {
                        PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                            "No graph with URI: {}",
                            iri.as_str()
                        ))
                    })?;
                if include_closures {
                    let closure = env
                        .get_closure(&id, recursion_depth)
                        .map_err(anyhow_to_pyerr)?;
                    for closure_id in closure {
                        let s = closure_id.to_uri_string();
                        if seen.insert(s.clone()) {
                            names.push(s);
                        }
                    }
                } else {
                    let s = id.to_uri_string();
                    if seen.insert(s.clone()) {
                        names.push(s);
                    }
                }
            }
            names
        };

        let dataset = self.cached_view_dataset(py)?;
        let view = build_view_graph(py, &dataset, &names)?;
        Ok((view, names))
    }

    /// Deprecated alias for :meth:`get_closure`.
    #[pyo3(signature = (uri, recursion_depth = -1))]
    fn get_closure_view(
        &self,
        py: Python<'_>,
        uri: &str,
        recursion_depth: i32,
    ) -> PyResult<(Py<PyAny>, Vec<String>)> {
        let warnings = py.import("warnings")?;
        warnings.call_method1(
            "warn",
            (
                "OntoEnv.get_closure_view() is deprecated; use OntoEnv.get_closure()",
                py.get_type::<pyo3::exceptions::PyDeprecationWarning>(),
                2u32,
            ),
        )?;
        self.get_closure(py, uri, recursion_depth)
    }

    /// Stream ``(s, p, o)`` triples for one named graph as rdflib terms,
    /// skipping the rdflib ``Graph`` wrapper entirely.
    ///
    /// Triples are read from the env once at call time; the iterator yields
    /// lazily after that. Use this when you only need to scan and don't want
    /// to pay for hash-set insertion into an rdflib graph.
    fn iter_triples(&self, py: Python<'_>, uri: &str) -> PyResult<Py<TripleIter>> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let graphid = env.resolve(ResolveTarget::Graph(iri)).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("No graph with URI: {uri}"))
        })?;
        let triples = collect_graph_triples(env, &graphid).map_err(anyhow_to_pyerr)?;
        Py::new(py, TripleIter::new(triples))
    }

    /// Stream ``(s, p, o)`` triples across the transitive ``owl:imports``
    /// closure of *uri*. Triples are *not* de-duplicated across named
    /// graphs; wrap in ``set()`` if you need set semantics.
    #[pyo3(signature = (uri, recursion_depth = -1))]
    fn iter_closure_triples(
        &self,
        py: Python<'_>,
        uri: &str,
        recursion_depth: i32,
    ) -> PyResult<Py<TripleIter>> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        let graphid = env.resolve(ResolveTarget::Graph(iri)).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("No graph with URI: {uri}"))
        })?;
        let closure = env
            .get_closure(&graphid, recursion_depth)
            .map_err(anyhow_to_pyerr)?;
        let mut triples: Vec<Triple> = Vec::new();
        for id in &closure {
            collect_graph_triples_into(env, id, &mut triples).map_err(anyhow_to_pyerr)?;
        }
        Py::new(py, TripleIter::new(triples))
    }

    /// Copy the named graph into a mutable in-memory ``rdflib.Graph``.
    ///
    /// If `graph` is provided, triples are added to it and the same object is
    /// returned. Otherwise a new ``rdflib.Graph`` is created.
    ///
    /// Works correctly with custom ``graph_store=`` backends: triples are
    /// fetched via the backend's ``get_graph`` method rather than the
    /// internal oxigraph store, so the returned graph always reflects what
    /// the Python store holds.
    #[pyo3(signature = (uri, graph = None))]
    fn copy_graph(
        &self,
        py: Python,
        uri: &Bound<'_, PyString>,
        graph: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(pystring_to_string(uri)?)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let source_graph = {
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

            env.copy_graph(&graphid).map_err(anyhow_to_pyerr)?
        };
        let res = match graph {
            Some(graph) => graph.clone(),
            None => rdflib.getattr("Graph")?.call0()?,
        };
        for triple in source_graph.into_iter() {
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

    /// Get namespace prefix mappings.
    ///
    /// Prefixes are extracted from both parser-level declarations (``@prefix``
    /// in Turtle, ``PREFIX`` in SPARQL-style syntaxes, XML namespace declarations)
    /// and SHACL ``sh:declare`` entries.  SHACL entries take precedence when the
    /// same prefix appears in both sources.
    ///
    /// Args:
    ///     ontology: The IRI of the ontology to query.  If ``None``, returns
    ///         merged namespaces from every ontology in the environment.
    ///     include_closure: When ``True``, namespaces from the full transitive
    ///         ``owl:imports`` closure of the ontology are included.  Ignored
    ///         when ``ontology`` is ``None``.
    ///
    /// Returns:
    ///     A ``dict[str, str]`` mapping prefix names (e.g. ``"owl"``) to
    ///     namespace IRIs (e.g. ``"http://www.w3.org/2002/07/owl#"``).
    #[pyo3(signature = (ontology = None, include_closure = false))]
    fn get_namespaces(
        &self,
        ontology: Option<&str>,
        include_closure: bool,
    ) -> PyResult<HashMap<String, String>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        if let Some(ontology) = ontology {
            let iri = NamedNode::new(ontology)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let graphid = env.resolve(ResolveTarget::Graph(iri)).ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Ontology not found: {ontology}"
                ))
            })?;
            env.get_namespaces(&graphid, include_closure)
                .map_err(anyhow_to_pyerr)
        } else {
            Ok(env.get_all_namespaces())
        }
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

    /// Return a read-only store-backed Dataset view of the environment.
    fn get_dataset(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.build_dataset(py, "auto", None)
    }

    /// Copy the environment into a mutable in-memory ``rdflib.Dataset``.
    ///
    /// If `dataset` is provided, quads are added to it and the same object is
    /// returned. Otherwise a new ``rdflib.Dataset`` is created.
    ///
    /// Works correctly with custom ``graph_store=`` backends: every named
    /// graph is materialised via the backend's ``get_graph`` path rather than
    /// the internal oxigraph store.
    #[pyo3(signature = (dataset = None))]
    fn copy_dataset(
        &self,
        py: Python<'_>,
        dataset: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let rdflib = py.import("rdflib")?;
        let rdflib_store = py.import("ontoenv.rdflib_store")?;
        let dataset_from_env = rdflib_store.getattr("dataset_from_env")?;
        let dataset = match dataset {
            Some(dataset) => dataset.clone(),
            None => rdflib.getattr("Dataset")?.call0()?,
        };
        let env_obj = Py::new(
            py,
            OntoEnv {
                inner: self.inner.clone(),
                cache: self.cache.clone(),
            },
        )?;
        let kwargs = PyDict::new(py);
        kwargs.set_item("env", env_obj)?;
        kwargs.set_item("store", dataset.getattr("store")?)?;
        kwargs.set_item("mode", "copy")?;
        dataset_from_env.call((), Some(&kwargs))?;
        Ok(dataset.unbind())
    }

    /// Deprecated alias for :meth:`get_dataset` / :meth:`copy_dataset`.
    #[pyo3(signature = (backend = "auto", store = None))]
    fn snapshot_as_dataset(
        &self,
        py: Python,
        backend: &str,
        store: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let warnings = py.import("warnings")?;
        warnings.call_method1(
            "warn",
            (
                "OntoEnv.snapshot_as_dataset() is deprecated; use \
                 OntoEnv.get_dataset() for a read-only view or \
                 OntoEnv.copy_dataset() for a mutable copy",
                py.get_type::<pyo3::exceptions::PyDeprecationWarning>(),
                2u32,
            ),
        )?;
        if backend == "copy" && store.is_none() {
            self.copy_dataset(py, None)
        } else {
            self.build_dataset(py, backend, store)
        }
    }

    /// Deprecated alias for :meth:`get_dataset` / :meth:`copy_dataset`.
    #[pyo3(signature = (backend = "auto", store = None))]
    fn as_dataset(
        &self,
        py: Python,
        backend: &str,
        store: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let warnings = py.import("warnings")?;
        warnings.call_method1(
            "warn",
            (
                "OntoEnv.as_dataset() is deprecated; use OntoEnv.get_dataset() \
                 for a read-only view or OntoEnv.copy_dataset() for a mutable copy",
                py.get_type::<pyo3::exceptions::PyDeprecationWarning>(),
                2u32,
            ),
        )?;
        if backend == "copy" && store.is_none() {
            self.copy_dataset(py, None)
        } else {
            self.build_dataset(py, backend, store)
        }
    }

    /// Refresh an existing read-only Dataset view from the current env.
    fn refresh_dataset(&self, py: Python<'_>, dataset: Py<PyAny>) -> PyResult<()> {
        let rdflib_store = py.import("ontoenv.rdflib_store")?;
        let refresh = rdflib_store.getattr("refresh_dataset_from_env")?;
        let env_obj = Py::new(
            py,
            OntoEnv {
                inner: self.inner.clone(),
                cache: self.cache.clone(),
            },
        )?;
        refresh.call1((dataset, env_obj))?;
        Ok(())
    }

    /// Deprecated alias for :meth:`get_dataset` / :meth:`copy_dataset`.
    #[pyo3(signature = (mode = "auto"))]
    fn to_rdflib_dataset(&self, py: Python, mode: &str) -> PyResult<Py<PyAny>> {
        let warnings = py.import("warnings")?;
        warnings.call_method1(
            "warn",
            (
                "OntoEnv.to_rdflib_dataset() is deprecated; use \
                 OntoEnv.get_dataset() for a read-only view or \
                 OntoEnv.copy_dataset() for a mutable copy",
                py.get_type::<pyo3::exceptions::PyDeprecationWarning>(),
                2u32,
            ),
        )?;
        if mode == "copy" {
            self.copy_dataset(py, None)
        } else {
            self.build_dataset(py, mode, None)
        }
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
        let Some(env) = guard.as_mut() else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ));
        };
        env.set_offline(offline);
        let result = env.save_to_directory().map_err(anyhow_to_pyerr);
        drop(guard);
        self.bump_generation();
        result
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
        let Some(env) = guard.as_mut() else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ));
        };
        env.set_strict(strict);
        let result = env.save_to_directory().map_err(anyhow_to_pyerr);
        drop(guard);
        self.bump_generation();
        result
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
        let Some(env) = guard.as_mut() else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ));
        };
        env.set_require_ontology_names(require);
        let result = env.save_to_directory().map_err(anyhow_to_pyerr);
        drop(guard);
        self.bump_generation();
        result
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
        let Some(env) = guard.as_mut() else {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ));
        };
        env.set_resolution_policy(policy);
        let result = env.save_to_directory().map_err(anyhow_to_pyerr);
        drop(guard);
        self.bump_generation();
        result
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
            // Release any cached Dataset so its store doesn't outlive the env.
            self.cache.lock().unwrap().cached = None;
            Ok(())
        })
    }

    /// Flush pending changes and rebind cached Graph views to the new
    /// snapshot. Graphs returned by prior `get_graph` calls share the cached
    /// Dataset's store, so they observe post-flush state without re-fetching.
    pub fn flush(&mut self, py: Python<'_>) -> PyResult<()> {
        let track_cached_snapshot = self.cached_view_dataset_exists();
        let cached_dataset_is_stale = self.cached_view_dataset_is_stale();
        let result = py.detach(|| {
            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap();
            if let Some(env) = guard.as_mut() {
                let before = if track_cached_snapshot {
                    env.store_path().and_then(snapshot_signature)
                } else {
                    None
                };
                env.flush().map_err(anyhow_to_pyerr)?;
                let after = if track_cached_snapshot {
                    env.store_path().and_then(snapshot_signature)
                } else {
                    None
                };
                Ok((before, after))
            } else {
                Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "OntoEnv is closed",
                ))
            }
        });

        let (before_snapshot, after_snapshot) = result?;

        if track_cached_snapshot {
            if cached_dataset_is_stale || before_snapshot != after_snapshot {
                if let Err(err) = self.refresh_cached_view_dataset(py) {
                    self.bump_generation();
                    return Err(err);
                }
            } else {
                self.mark_cached_view_dataset_current();
            }
        }

        Ok(())
    }

    // ------------------------------------------------------------------ #
    // Pythonic sugar                                                       #
    // ------------------------------------------------------------------ #

    /// An ``OntoEnv`` reference is always truthy. We define ``__bool__``
    /// explicitly so that ``if env:`` checks identity, not size, and so that
    /// ``__len__`` is free to raise on a closed env without breaking
    /// pre-existing ``if env:`` patterns.
    fn __bool__(&self) -> bool {
        true
    }

    /// ``len(env)`` — number of ontologies in the environment.
    fn __len__(&self) -> PyResult<usize> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        Ok(env.ontologies().len())
    }

    /// ``uri in env`` — true if the environment has an ontology resolvable
    /// from the given URI (canonical name, alias, or source URL).
    fn __contains__(&self, uri: &str) -> PyResult<bool> {
        let Ok(iri) = NamedNode::new(uri) else {
            return Ok(false);
        };
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        let env = guard
            .as_ref()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        Ok(env.resolve(ResolveTarget::Graph(iri)).is_some())
    }

    /// ``env[uri]`` — shorthand for :py:meth:`get_graph`.
    fn __getitem__(&self, py: Python, uri: &Bound<'_, PyString>) -> PyResult<Py<PyAny>> {
        self.get_graph(py, uri)
    }

    /// ``for name in env`` — iterate over the URIs of every ontology in the
    /// environment. The order is unspecified.
    fn __iter__(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let names = self.get_ontology_names()?;
        Ok(names.into_pyobject(py)?.call_method0("__iter__")?.unbind())
    }

    /// ``with OntoEnv(...) as env:`` — return ``self`` so the bound name is
    /// the env itself.
    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// On context-manager exit, persist and release resources by calling
    /// :py:meth:`close`. Exceptions propagate.
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exc_type: &Bound<'_, PyAny>,
        _exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        self.close(py)
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
    m.add_class::<PyRdfLibStoreBackend>()?;
    m.add_function(wrap_pyfunction!(run_cli, m)?)?;
    // add version attribute
    m.add("version", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rdf5d::{write_file, writer::Term as WriterTerm, Quint, Snapshot};
    use spareval::QueryResults;
    use spargebra::SparqlParser;

    #[test]
    fn logical_snapshot_collapses_duplicate_graph_names() {
        let path = std::env::temp_dir().join(format!(
            "ontoenv_python_logical_view_{}.r5tu",
            rand::random::<u64>()
        ));
        let quints = vec![
            Quint {
                id: "dataset:1".into(),
                s: WriterTerm::Iri("http://example.org/alice".into()),
                p: WriterTerm::Iri("http://example.org/name".into()),
                o: WriterTerm::Literal {
                    lex: "Alice".into(),
                    dt: None,
                    lang: None,
                },
                gname: "http://example.org/graph/shared".into(),
            },
            Quint {
                id: "dataset:2".into(),
                s: WriterTerm::Iri("http://example.org/alice".into()),
                p: WriterTerm::Iri("http://example.org/name".into()),
                o: WriterTerm::Literal {
                    lex: "Alice".into(),
                    dt: None,
                    lang: None,
                },
                gname: "http://example.org/graph/shared".into(),
            },
            Quint {
                id: "dataset:3".into(),
                s: WriterTerm::Iri("http://example.org/bob".into()),
                p: WriterTerm::Iri("http://example.org/name".into()),
                o: WriterTerm::Literal {
                    lex: "Bob".into(),
                    dt: None,
                    lang: None,
                },
                gname: "http://example.org/graph/shared".into(),
            },
        ];
        write_file(&path, &quints).expect("write test fixture");

        let snapshot = Snapshot::open(&path).expect("open snapshot");
        let mut query = SparqlParser::new()
            .parse_query(
                "SELECT ?s WHERE {
                    GRAPH <http://example.org/graph/shared> {
                        ?s <http://example.org/name> ?o
                    }
                }",
            )
            .expect("parse query");
        let results = snapshot.query(&mut query).expect("execute query");
        let QueryResults::Solutions(solutions) = results else {
            panic!("expected solution results");
        };
        let mut subjects = solutions
            .map(|row| row.map(|solution| solution["s"].to_string()))
            .collect::<std::result::Result<Vec<_>, _>>()
            .expect("collect results");
        subjects.sort();

        assert_eq!(
            subjects,
            vec!["<http://example.org/alice>", "<http://example.org/bob>"]
        );

        let _ = std::fs::remove_file(path);
    }
}
