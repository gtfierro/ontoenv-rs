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
use oxrdf::{Dataset as OxDataset, Variable as OxVariable};
use oxigraph::io::{RdfFormat, RdfParser};
use oxigraph::model::{
    BlankNode, Graph as OxigraphGraph, GraphName, GraphNameRef, Literal, NamedNode,
    NamedOrBlankNode, NamedOrBlankNodeRef, Quad, Term, TermRef, Triple, TripleRef,
};
use oxigraph::store::Store;
#[cfg(not(feature = "cli"))]
use pyo3::exceptions::PyRuntimeError;
use pyo3::{
    prelude::*,
    types::{IntoPyDict, PyDict, PyString, PyStringMethods, PyTuple},
};
use rand::random;
use rdf5d::{DecodedTerm, R5tuFile};
use spargebra::SparqlParser;
use spareval::{
    InternalQuad as SpareInternalQuad, QueryEvaluator, QueryResults as SpareQueryResults,
    QueryableDataset,
};
use std::borrow::{Borrow, Cow};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::iter::{empty, once};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

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

fn term_to_python<'a>(
    py: Python,
    rdflib: &Bound<'a, PyModule>,
    node: Term,
) -> PyResult<Bound<'a, PyAny>> {
    let dtype: Option<String> = match &node {
        Term::Literal(lit) => {
            let dt = lit.datatype().as_str();
            if dt == "http://www.w3.org/2001/XMLSchema#string" {
                None
            } else {
                Some(dt.to_string())
            }
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

#[derive(Debug)]
struct LogicalGraphInfo {
    gids: Vec<u64>,
    /// Exact, dedup-aware unique-triple count. Filled lazily on the first
    /// `len()` call so that opening a snapshot stays O(graphs). For
    /// single-gid logical graphs we initialize this from the GDIR
    /// `n_triples` at open time, since no cross-gid dedup is needed.
    triple_count: OnceLock<usize>,
}

#[derive(Debug)]
struct Rdf5dSnapshot {
    file: Arc<R5tuFile>,
    logical_graphs: HashMap<String, LogicalGraphInfo>,
    // Memoizes term -> term_id lookups so repeated SPARQL bindings of the
    // same IRIs don't repeatedly linear-scan the term table via
    // R5tuFile::find_decoded_term. A proper on-disk reverse index in rdf5d
    // is the right long-term fix.
    term_id_cache: Mutex<HashMap<DecodedTerm<'static>, Option<u64>>>,
}

impl Rdf5dSnapshot {
    fn open(path: &Path) -> Result<Self> {
        let file = Arc::new(R5tuFile::open_mmap(path)?);
        let mut grouped: HashMap<String, Vec<(u64, u64)>> = HashMap::new();
        for graph in file.enumerate_all()? {
            grouped
                .entry(graph.graphname)
                .or_default()
                .push((graph.gid, graph.n_triples));
        }

        let mut logical_graphs = HashMap::with_capacity(grouped.len());
        for (graph_name, entries) in grouped {
            let triple_count = OnceLock::new();
            if entries.len() == 1 {
                // Single physical gid: no cross-gid dedup needed.
                let _ = triple_count.set(entries[0].1 as usize);
            }
            let gids = entries.into_iter().map(|(gid, _)| gid).collect();
            logical_graphs.insert(graph_name, LogicalGraphInfo { gids, triple_count });
        }

        Ok(Self {
            file,
            logical_graphs,
            term_id_cache: Mutex::new(HashMap::new()),
        })
    }

    fn triple_count_for(&self, info: &LogicalGraphInfo) -> Result<usize> {
        if let Some(count) = info.triple_count.get() {
            return Ok(*count);
        }
        let count = count_unique_triples_for_gids(&self.file, &info.gids)?;
        let _ = info.triple_count.set(count);
        Ok(*info.triple_count.get().unwrap_or(&count))
    }

    fn named_graphs(&self) -> Result<Vec<NamedOrBlankNode>> {
        let mut values = Vec::with_capacity(self.logical_graphs.len());
        for graph_name in self.logical_graphs.keys() {
            values.push(named_or_blank_node_from_graph_name(graph_name)?);
        }
        Ok(values)
    }

    fn logical_graph(&self, graph_name: &GraphName) -> Option<&LogicalGraphInfo> {
        match graph_name {
            GraphName::NamedNode(node) => self.logical_graphs.get(node.as_str()),
            GraphName::BlankNode(node) => self.logical_graphs.get(node.as_str()),
            GraphName::DefaultGraph => None,
        }
    }

    fn graph_names_for_term_ids(
        &self,
        subject_id: u64,
        predicate_id: u64,
        object_id: u64,
    ) -> Result<Vec<String>> {
        let mut matches = Vec::new();
        for (graph_name, info) in &self.logical_graphs {
            let mut found = false;
            for gid in &info.gids {
                for (s_id, p_id, o_id) in self.file.triples_ids(*gid)? {
                    if s_id == subject_id && p_id == predicate_id && o_id == object_id {
                        found = true;
                        break;
                    }
                }
                if found {
                    break;
                }
            }
            if found {
                matches.push(graph_name.clone());
            }
        }
        Ok(matches)
    }

    fn find_term_id(&self, term: &Term) -> Result<Option<u64>> {
        let key = term_to_decoded_term(term);
        {
            let cache = self.term_id_cache.lock().unwrap();
            if let Some(cached) = cache.get(&key) {
                return Ok(*cached);
            }
        }
        let result = self.file.find_decoded_term(&key)?;
        let mut cache = self.term_id_cache.lock().unwrap();
        cache.insert(key, result);
        Ok(result)
    }
}

/// The storage strategy currently bound to an `OntoEnvStore`.
///
/// `EnvSnapshotMaterialized` holds an in-memory `OxDataset` copy of the env's
/// quads (the `copy` backend). `EnvSnapshotRdf5d` holds an mmap-backed view of
/// a `.ontoenv/store.r5tu` file (the `rdf5d` backend). Rebinding the store
/// (via `refresh_from_env`) swaps the variant.
#[derive(Debug)]
enum RdfLibStoreBackend {
    EnvSnapshotMaterialized { dataset: Arc<OxDataset> },
    EnvSnapshotRdf5d {
        #[allow(dead_code)]
        store_path: PathBuf,
        snapshot: Arc<Rdf5dSnapshot>,
    },
}

impl Default for RdfLibStoreBackend {
    fn default() -> Self {
        Self::EnvSnapshotMaterialized {
            dataset: Arc::new(OxDataset::new()),
        }
    }
}

fn count_unique_triples_for_gids(file: &R5tuFile, gids: &[u64]) -> Result<usize> {
    let mut triples = HashSet::new();
    for gid in gids {
        for triple_ids in file.triples_ids(*gid)? {
            triples.insert(triple_ids);
        }
    }
    Ok(triples.len())
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
    graph_name_to_python(py, rdflib, graph_name_from_string(graph_name).map_err(anyhow_to_pyerr)?)
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

fn decoded_term_to_term(term: DecodedTerm<'_>) -> Result<Term> {
    Ok(match term {
        DecodedTerm::Iri(value) => Term::NamedNode(
            NamedNode::new(value.into_owned()).map_err(|e| anyhow!(e.to_string()))?,
        ),
        DecodedTerm::BNode(value) => Term::BlankNode(
            BlankNode::new(value.into_owned()).map_err(|e| anyhow!(e.to_string()))?,
        ),
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

/// `spareval::QueryableDataset` adapter that exposes an `Rdf5dSnapshot` as
/// a SPARQL-queryable dataset organized by logical (by-name) graph.
///
/// Differs from `rdf5d::SparqlDatasetView` in that it deduplicates triples
/// across the physical gids that share a single graph name, so SPARQL sees
/// each logical graph as a single named graph.
#[derive(Clone, Copy, Debug)]
struct LogicalSparqlDatasetView<'a> {
    snapshot: &'a Rdf5dSnapshot,
}

impl<'a> LogicalSparqlDatasetView<'a> {
    fn new(snapshot: &'a Rdf5dSnapshot) -> Self {
        Self { snapshot }
    }

    fn graph_term(graph_name: &'a str) -> DecodedTerm<'a> {
        DecodedTerm::Iri(Cow::Borrowed(graph_name))
    }

    fn quads_for_logical_graph(
        snapshot: &'a Rdf5dSnapshot,
        graph_name: &'a str,
        info: &'a LogicalGraphInfo,
        subject: Option<DecodedTerm<'a>>,
        predicate: Option<DecodedTerm<'a>>,
        object: Option<DecodedTerm<'a>>,
    ) -> Box<dyn Iterator<Item = std::result::Result<SpareInternalQuad<DecodedTerm<'a>>, rdf5d::reader::R5Error>> + 'a>
    {
        let file = snapshot.file.as_ref();
        let mut quads = Vec::new();
        let mut seen = HashSet::new();
        for gid in &info.gids {
            let triples = match file.triples_ids(*gid) {
                Ok(triples) => triples,
                Err(error) => return Box::new(once(Err(error))),
            };
            for (s_id, p_id, o_id) in triples {
                if !seen.insert((s_id, p_id, o_id)) {
                    continue;
                }

                let subject_term = match file.decoded_term(s_id) {
                    Ok(term) => term,
                    Err(error) => return Box::new(once(Err(error))),
                };
                if subject
                    .as_ref()
                    .is_some_and(|expected| expected != &subject_term)
                {
                    continue;
                }

                let predicate_term = match file.decoded_term(p_id) {
                    Ok(term) => term,
                    Err(error) => return Box::new(once(Err(error))),
                };
                if predicate
                    .as_ref()
                    .is_some_and(|expected| expected != &predicate_term)
                {
                    continue;
                }

                let object_term = match file.decoded_term(o_id) {
                    Ok(term) => term,
                    Err(error) => return Box::new(once(Err(error))),
                };
                if object
                    .as_ref()
                    .is_some_and(|expected| expected != &object_term)
                {
                    continue;
                }

                quads.push(Ok(SpareInternalQuad {
                    subject: subject_term,
                    predicate: predicate_term,
                    object: object_term,
                    graph_name: Some(Self::graph_term(graph_name)),
                }));
            }
        }
        Box::new(quads.into_iter())
    }

    fn quads_for_all_logical_graphs(
        snapshot: &'a Rdf5dSnapshot,
        subject: Option<DecodedTerm<'a>>,
        predicate: Option<DecodedTerm<'a>>,
        object: Option<DecodedTerm<'a>>,
    ) -> Box<dyn Iterator<Item = std::result::Result<SpareInternalQuad<DecodedTerm<'a>>, rdf5d::reader::R5Error>> + 'a>
    {
        Box::new(
            snapshot
                .logical_graphs
                .iter()
                .flat_map(move |(graph_name, info)| {
                    Self::quads_for_logical_graph(
                        snapshot,
                        graph_name.as_str(),
                        info,
                        subject.clone(),
                        predicate.clone(),
                        object.clone(),
                    )
                }),
        )
    }
}

impl<'a> QueryableDataset<'a> for LogicalSparqlDatasetView<'a> {
    type InternalTerm = DecodedTerm<'a>;
    type Error = rdf5d::reader::R5Error;

    #[allow(refining_impl_trait)]
    fn internal_quads_for_pattern(
        &self,
        subject: Option<&Self::InternalTerm>,
        predicate: Option<&Self::InternalTerm>,
        object: Option<&Self::InternalTerm>,
        graph_name: Option<Option<&Self::InternalTerm>>,
    ) -> Box<dyn Iterator<Item = std::result::Result<SpareInternalQuad<Self::InternalTerm>, Self::Error>> + 'a>
    {
        let subject = subject.cloned();
        let predicate = predicate.cloned();
        let object = object.cloned();
        let snapshot = self.snapshot;

        match graph_name {
            None | Some(None) => {
                Self::quads_for_all_logical_graphs(snapshot, subject, predicate, object)
            }
            Some(Some(DecodedTerm::Iri(graph_name))) => {
                let Some((graph_name, info)) = snapshot.logical_graphs.get_key_value(graph_name.as_ref()) else {
                    return Box::new(empty());
                };
                Self::quads_for_logical_graph(
                    snapshot,
                    graph_name.as_ref(),
                    info,
                    subject,
                    predicate,
                    object,
                )
            }
            Some(Some(_)) => Box::new(empty()),
        }
    }

    #[allow(refining_impl_trait)]
    fn internal_named_graphs(
        &self,
    ) -> Box<dyn Iterator<Item = std::result::Result<Self::InternalTerm, Self::Error>> + 'a> {
        Box::new(
            self.snapshot
                .logical_graphs
                .keys()
                .map(|graph_name| Ok(Self::graph_term(graph_name.as_str()))),
        )
    }

    fn contains_internal_graph_name(
        &self,
        graph_name: &Self::InternalTerm,
    ) -> std::result::Result<bool, Self::Error> {
        let DecodedTerm::Iri(graph_name) = graph_name else {
            return Ok(false);
        };
        Ok(self
            .snapshot
            .logical_graphs
            .contains_key(graph_name.as_ref()))
    }

    fn internalize_term(&self, term: Term) -> std::result::Result<Self::InternalTerm, Self::Error> {
        Ok(match term {
            Term::NamedNode(node) => DecodedTerm::Iri(Cow::Owned(node.into_string())),
            Term::BlankNode(node) => DecodedTerm::BNode(Cow::Owned(node.as_str().to_string())),
            Term::Literal(literal) => {
                if let Some(language) = literal.language() {
                    DecodedTerm::Literal {
                        lex: Cow::Owned(literal.value().to_string()),
                        dt: None,
                        lang: Some(Cow::Owned(language.to_string())),
                    }
                } else {
                    let datatype = literal.datatype();
                    let datatype = if datatype.as_str()
                        == "http://www.w3.org/2001/XMLSchema#string"
                    {
                        None
                    } else {
                        Some(Cow::Owned(datatype.as_str().to_string()))
                    };
                    DecodedTerm::Literal {
                        lex: Cow::Owned(literal.value().to_string()),
                        dt: datatype,
                        lang: None,
                    }
                }
            }
        })
    }

    fn externalize_term(
        &self,
        term: Self::InternalTerm,
    ) -> std::result::Result<Term, Self::Error> {
        Ok(match term {
            DecodedTerm::Iri(value) => NamedNode::new(value.into_owned())
                .map_err(|_| rdf5d::reader::R5Error::Invalid("invalid IRI term"))?
                .into(),
            DecodedTerm::BNode(value) => {
                let label = value.strip_prefix("_:").unwrap_or(value.as_ref());
                BlankNode::new(label.to_string())
                    .map_err(|_| rdf5d::reader::R5Error::Invalid("invalid blank node"))?
                    .into()
            }
            DecodedTerm::Literal { lex, dt, lang } => {
                if let Some(dt) = dt {
                    Literal::new_typed_literal(
                        lex.into_owned(),
                        NamedNode::new(dt.into_owned())
                            .map_err(|_| rdf5d::reader::R5Error::Invalid("invalid datatype IRI"))?,
                    )
                    .into()
                } else if let Some(lang) = lang {
                    Literal::new_language_tagged_literal(lex.into_owned(), lang.into_owned())
                        .map_err(|_| rdf5d::reader::R5Error::Invalid("invalid language tag"))?
                        .into()
                } else {
                    Literal::new_simple_literal(lex.into_owned()).into()
                }
            }
        })
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
                let triple = triple.map_err(|e| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
                })?;
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
    fn bind_materialized_snapshot(
        &self,
        quads: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
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
            let subject = term_to_subject(
                term_from_python(&tuple.get_item(0)?).map_err(anyhow_to_pyerr)?,
            )
            .map_err(anyhow_to_pyerr)?;
            let predicate = term_to_predicate(
                term_from_python(&tuple.get_item(1)?).map_err(anyhow_to_pyerr)?,
            )
            .map_err(anyhow_to_pyerr)?;
            let object = term_from_python(&tuple.get_item(2)?).map_err(anyhow_to_pyerr)?;
            let graph_name =
                graph_name_from_python_context(Some(&tuple.get_item(3)?)).map_err(anyhow_to_pyerr)?;
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
        let env_rs = guard.as_ref().ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed")
        })?;

        let mut dataset = OxDataset::new();
        let ids: Vec<GraphIdentifier> = env_rs.ontologies().keys().cloned().collect();
        for id in ids {
            let graph_name = GraphName::NamedNode(NamedNode::from(id.name().into_owned()));
            let graph = env_rs.get_graph(&id).map_err(anyhow_to_pyerr)?;
            for triple in graph.iter() {
                let quad = Quad::new(
                    triple.subject.clone(),
                    triple.predicate.clone(),
                    triple.object.clone(),
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

    /// Bind this store to an mmap-backed, zero-copy `Rdf5dSnapshot` opened
    /// from `store_path` (typically `.ontoenv/store.r5tu`).
    fn bind_rdf5d_snapshot(&self, store_path: &str) -> PyResult<()> {
        let snapshot = Rdf5dSnapshot::open(Path::new(store_path)).map_err(anyhow_to_pyerr)?;
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
        let predicate =
            term_to_predicate(term_from_python(predicate).map_err(anyhow_to_pyerr)?)
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
    ) -> PyResult<Vec<(Py<PyAny>, Vec<Py<PyAny>>)>> {
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
        let rdflib = PyModule::import(py, "rdflib")?;
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

                let mut rows = Vec::with_capacity(by_triple.len());
                for (triple, contexts) in by_triple {
                    let triple_obj = PyTuple::new(
                        py,
                        [
                            term_to_python(py, &rdflib, triple.subject.into())?,
                            term_to_python(py, &rdflib, triple.predicate.into())?,
                            term_to_python(py, &rdflib, triple.object)?,
                        ],
                    )?;
                    let contexts = contexts
                        .into_iter()
                        .map(|graph_name| {
                            graph_name_to_python(py, &rdflib, graph_name).map(Bound::unbind)
                        })
                        .collect::<PyResult<Vec<_>>>()?;
                    rows.push((triple_obj.unbind().into_any(), contexts));
                }
                Ok(rows)
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                if matches!(graph_name, Some(GraphName::DefaultGraph)) {
                    return Ok(Vec::new());
                }
                let mut grouped: HashMap<(u64, u64, u64), Vec<String>> = HashMap::new();
                let subject_id = subject
                    .as_ref()
                    .map(|value| snapshot.find_term_id(&subject_to_term(value)).map_err(anyhow_to_pyerr))
                    .transpose()?
                    .flatten();
                let predicate_id = predicate
                    .as_ref()
                    .map(|value| {
                        snapshot
                            .find_term_id(&Term::NamedNode(value.clone()))
                            .map_err(anyhow_to_pyerr)
                    })
                    .transpose()?
                    .flatten();
                let object_id = object
                    .as_ref()
                    .map(|value| snapshot.find_term_id(value).map_err(anyhow_to_pyerr))
                    .transpose()?
                    .flatten();
                if (subject.is_some() && subject_id.is_none())
                    || (predicate.is_some() && predicate_id.is_none())
                    || (object.is_some() && object_id.is_none())
                {
                    return Ok(Vec::new());
                }

                if let Some(graph_name) = graph_name.as_ref() {
                    let Some(info) = snapshot.logical_graph(graph_name) else {
                        return Ok(Vec::new());
                    };
                    let Some(graph_name_key) = graph_name_key(graph_name) else {
                        return Ok(Vec::new());
                    };
                    let mut seen = HashSet::new();
                    for gid in &info.gids {
                        for triple_ids in snapshot.file.triples_ids(*gid).map_err(r5error_to_pyerr)? {
                            if !seen.insert(triple_ids) {
                                continue;
                            }
                            let (s_id, p_id, o_id) = triple_ids;
                            if subject_id.is_some_and(|id| id != s_id)
                                || predicate_id.is_some_and(|id| id != p_id)
                                || object_id.is_some_and(|id| id != o_id)
                            {
                                continue;
                            }
                            grouped.insert((s_id, p_id, o_id), vec![graph_name_key.clone()]);
                        }
                    }
                } else {
                    for (graph_name, info) in &snapshot.logical_graphs {
                        for gid in &info.gids {
                            for (s_id, p_id, o_id) in snapshot.file.triples_ids(*gid).map_err(r5error_to_pyerr)? {
                                if subject_id.is_some_and(|id| id != s_id)
                                    || predicate_id.is_some_and(|id| id != p_id)
                                    || object_id.is_some_and(|id| id != o_id)
                                {
                                    continue;
                                }
                                let contexts = grouped.entry((s_id, p_id, o_id)).or_default();
                                if !contexts.iter().any(|value| value == graph_name) {
                                    contexts.push(graph_name.clone());
                                }
                            }
                        }
                    }
                }

                let mut rows = Vec::with_capacity(grouped.len());
                for ((s_id, p_id, o_id), contexts) in grouped {
                    let triple_obj = PyTuple::new(
                        py,
                        [
                            term_to_python(
                                py,
                                &rdflib,
                                decoded_term_to_term(snapshot.file.decoded_term(s_id).map_err(r5error_to_pyerr)?)
                                    .map_err(anyhow_to_pyerr)?,
                            )?,
                            term_to_python(
                                py,
                                &rdflib,
                                decoded_term_to_term(snapshot.file.decoded_term(p_id).map_err(r5error_to_pyerr)?)
                                    .map_err(anyhow_to_pyerr)?,
                            )?,
                            term_to_python(
                                py,
                                &rdflib,
                                decoded_term_to_term(snapshot.file.decoded_term(o_id).map_err(r5error_to_pyerr)?)
                                    .map_err(anyhow_to_pyerr)?,
                            )?,
                        ],
                    )?;
                    let contexts = contexts
                        .into_iter()
                        .map(|graph_name| {
                            graph_name_to_python_from_str(py, &rdflib, &graph_name)
                                .map(Bound::unbind)
                        })
                        .collect::<PyResult<Vec<_>>>()?;
                    rows.push((triple_obj.unbind().into_any(), contexts));
                }
                Ok(rows)
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
                if let (Some(subject), Some(predicate), Some(object)) = (subject, predicate, object) {
                    let subject = term_to_subject(term_from_python(subject).map_err(anyhow_to_pyerr)?)
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
                    .map(|graph_name| graph_name_to_python(py, &rdflib, graph_name).map(Bound::unbind))
                    .collect()
            }
            RdfLibStoreBackend::EnvSnapshotRdf5d { snapshot, .. } => {
                if let (Some(subject), Some(predicate), Some(object)) = (subject, predicate, object) {
                    let subject = term_to_subject(term_from_python(subject).map_err(anyhow_to_pyerr)?)
                        .map_err(anyhow_to_pyerr)?;
                    let predicate =
                        term_to_predicate(term_from_python(predicate).map_err(anyhow_to_pyerr)?)
                            .map_err(anyhow_to_pyerr)?;
                    let object = term_from_python(object).map_err(anyhow_to_pyerr)?;

                    let Some(subject_id) = snapshot
                        .find_term_id(&subject_to_term(&subject))
                        .map_err(anyhow_to_pyerr)?
                    else {
                        return Ok(Vec::new());
                    };
                    let Some(predicate_id) = snapshot
                        .find_term_id(&Term::NamedNode(predicate))
                        .map_err(anyhow_to_pyerr)?
                    else {
                        return Ok(Vec::new());
                    };
                    let Some(object_id) = snapshot.find_term_id(&object).map_err(anyhow_to_pyerr)? else {
                        return Ok(Vec::new());
                    };
                    let graph_names = snapshot
                        .graph_names_for_term_ids(subject_id, predicate_id, object_id)
                        .map_err(anyhow_to_pyerr)?;
                    graph_names
                        .into_iter()
                        .map(|graph_name| {
                            graph_name_to_python_from_str(py, &rdflib, &graph_name).map(Bound::unbind)
                        })
                        .collect()
                } else {
                    snapshot
                        .logical_graphs
                        .keys()
                        .map(|graph_name| {
                            graph_name_to_python_from_str(py, &rdflib, graph_name).map(Bound::unbind)
                        })
                        .collect()
                }
            }
        }
    }

    fn len(&self, context: Option<&Bound<'_, PyAny>>) -> PyResult<usize> {
        let graph_name = match context {
            Some(value) => Some(graph_name_from_python_context(Some(value)).map_err(anyhow_to_pyerr)?),
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
                    } else {
                        match snapshot.logical_graph(&graph_name) {
                            Some(info) => snapshot.triple_count_for(info).map_err(anyhow_to_pyerr)?,
                            None => 0,
                        }
                    }
                }
                None => {
                    let mut total = 0usize;
                    for info in snapshot.logical_graphs.values() {
                        total += snapshot.triple_count_for(info).map_err(anyhow_to_pyerr)?;
                    }
                    total
                }
            },
        })
    }

    fn query(
        &self,
        py: Python<'_>,
        query: &str,
        init_bindings: Option<HashMap<String, Py<PyAny>>>,
        query_graph: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let parsed = SparqlParser::new()
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
                let named_graphs = snapshot.named_graphs().map_err(anyhow_to_pyerr)?;
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
                let view = LogicalSparqlDatasetView::new(snapshot.as_ref());
                let results = prepared
                    .execute(view)
                    .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
                query_results_to_python(py, results)
            }
        }
    }
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
#[pyclass]
struct TripleIter {
    triples: std::vec::IntoIter<Triple>,
}

#[pymethods]
impl TripleIter {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&mut self, py: Python<'_>) -> PyResult<Option<Py<PyTuple>>> {
        let Some(t) = self.triples.next() else {
            return Ok(None);
        };
        let rdflib = py.import("rdflib")?;
        let tuple = PyTuple::new(
            py,
            &[
                term_to_python(py, &rdflib, t.subject.into())?,
                term_to_python(py, &rdflib, t.predicate.into())?,
                term_to_python(py, &rdflib, t.object)?,
            ],
        )?;
        Ok(Some(tuple.unbind()))
    }

    fn __len__(&self) -> usize {
        self.triples.len()
    }
}

impl OntoEnv {
    /// Internal helper that delegates Dataset construction to the Python-side
    /// `ontoenv.rdflib_store.dataset_from_env` factory. Used by
    /// `snapshot_as_dataset`; not exposed to Python.
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

    /// Mark the cached Dataset (if any) as stale. Subsequent reads via
    /// [`Self::cached_view_dataset`] will rebuild before returning.
    fn bump_generation(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.generation = cache.generation.wrapping_add(1);
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

        // Build outside the cache lock: dataset construction re-enters Python.
        let dataset = self.build_dataset(py, "auto", None)?;

        let mut cache = self.cache.lock().unwrap();
        // Only install if no newer generation snuck in while we were building.
        if cache.generation == current_gen {
            cache.cached = Some((current_gen, dataset.clone_ref(py)));
        }
        Ok(dataset)
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
            OntoEnvRs::load_from_directory(load_root, read_only).map_err(anyhow_to_pyerr)?
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
        if let Some(env) = guard.as_mut() {
            env.update_all(all).map_err(anyhow_to_pyerr)?;
            env.save_to_directory().map_err(anyhow_to_pyerr)?;
            drop(guard);
            self.bump_generation();
            Ok(())
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    /// Re-reads all graphs from the attached graph store and rebuilds the environment's
    /// ontology metadata and import dependency graph.  Call this whenever the graph store
    /// has been mutated externally and you need OntoEnv's view to catch up.
    fn refresh_from_store(&self) -> PyResult<()> {
        let mut guard = self.inner.lock().unwrap();
        let env = guard
            .as_mut()
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("OntoEnv is closed"))?;
        env.refresh_from_graph_io().map_err(anyhow_to_pyerr)
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

        // Use the first ontology's name as the explicit root for sh:prefixes rewrite.
        // Always pass rewrite_sh_prefixes=false to get_union_graph; we rewrite in Python
        // after merging so the rdflib graph's own triples are included.
        let rust_root = all_ontologies[0].name();
        let union = env
            .get_union_graph(&all_ontologies, rust_root, Some(false), Some(true))
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
        let overwrite_flag: Overwrite = overwrite.into();
        let refresh: RefreshStrategy = force.into();
        let result = add_resolved_to_env(
            env,
            location,
            resolved,
            overwrite_flag,
            refresh,
            fetch_imports,
        );
        drop(guard);
        self.bump_generation();
        result
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
        let overwrite_flag: Overwrite = overwrite.into();
        let refresh: RefreshStrategy = force.into();
        let result =
            add_resolved_to_env(env, location, resolved, overwrite_flag, refresh, false);
        drop(guard);
        self.bump_generation();
        result
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
        let rdflib = py.import("rdflib")?;
        let dataset = self.cached_view_dataset(py)?;
        let uri_ref = rdflib.getattr("URIRef")?.call1((uri_string,))?;
        Ok(dataset
            .bind(py)
            .call_method1("graph", (uri_ref,))?
            .unbind())
    }

    /// Return a read-only merged view over the transitive ``owl:imports``
    /// closure of *uri*, plus the list of ontology IRIs that contribute to
    /// the view (root first, in BFS order).
    ///
    /// The returned ``Graph`` is a :py:class:`ontoenv.ClosureGraphView` —
    /// triple-pattern lookups dispatch to each underlying named graph and
    /// de-duplicate, so it behaves like a merged graph without materializing
    /// one. Mutation raises ``ValueError``; use :py:meth:`get_closure` for a
    /// mutable in-memory merge.
    #[pyo3(signature = (uri, recursion_depth = -1))]
    fn get_closure_view(
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
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "No graph with URI: {uri}"
                ))
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
        let rdflib_store = py.import("ontoenv.rdflib_store")?;
        let view = rdflib_store
            .getattr("ClosureGraphView")?
            .call1((dataset, names.clone()))?
            .unbind();
        Ok((view, names))
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
        let graph = env.get_graph(&graphid).map_err(anyhow_to_pyerr)?;
        let triples: Vec<Triple> = graph.iter().map(|t| t.into_owned()).collect();
        Py::new(
            py,
            TripleIter {
                triples: triples.into_iter(),
            },
        )
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
            let graph = env.get_graph(id).map_err(anyhow_to_pyerr)?;
            triples.extend(graph.iter().map(|t| t.into_owned()));
        }
        Py::new(
            py,
            TripleIter {
                triples: triples.into_iter(),
            },
        )
    }

    /// Copy the named graph into a mutable in-memory ``rdflib.Graph``.
    fn copy_graph(&self, py: Python, uri: &Bound<'_, PyString>) -> PyResult<Py<PyAny>> {
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

    /// Return a point-in-time `rdflib.Dataset` view of the environment.
    ///
    /// The Dataset is read-only and reflects the state of the env at the
    /// time of the call. Call again (or `refresh_dataset_from_env`) after
    /// `env.flush()` to pick up subsequent changes.
    ///
    /// Args:
    ///     backend: ``"auto"`` (default) picks ``"rdf5d"`` when a
    ///         persistent ``.ontoenv/store.r5tu`` exists and ``"copy"``
    ///         otherwise. ``"rdf5d"`` forces the zero-copy mmap-backed
    ///         path and raises ``ValueError`` for temporary or
    ///         ``graph_store=``-backed envs. ``"copy"`` always
    ///         materializes the env into an in-memory snapshot.
    ///     store: Optional existing `rdflib.Store` to bind the Dataset to.
    #[pyo3(signature = (backend = "auto", store = None))]
    fn snapshot_as_dataset(
        &self,
        py: Python,
        backend: &str,
        store: Option<Py<PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        self.build_dataset(py, backend, store)
    }

    /// Deprecated alias for :meth:`snapshot_as_dataset`. Emits
    /// ``DeprecationWarning`` and delegates with ``backend=mode``.
    #[pyo3(signature = (mode = "auto"))]
    fn to_rdflib_dataset(&self, py: Python, mode: &str) -> PyResult<Py<PyAny>> {
        let warnings = py.import("warnings")?;
        warnings.call_method1(
            "warn",
            (
                "OntoEnv.to_rdflib_dataset() is deprecated; use \
                 OntoEnv.snapshot_as_dataset(backend=...) instead",
                py.get_type::<pyo3::exceptions::PyDeprecationWarning>(),
                2u32,
            ),
        )?;
        self.build_dataset(py, mode, None)
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
            // Release any cached Dataset so its store doesn't outlive the env.
            self.cache.lock().unwrap().cached = None;
            Ok(())
        })
    }

    pub fn flush(&mut self, py: Python<'_>) -> PyResult<()> {
        let result = py.detach(|| {
            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap();
            if let Some(env) = guard.as_mut() {
                env.flush().map_err(anyhow_to_pyerr)
            } else {
                Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                    "OntoEnv is closed",
                ))
            }
        });
        if result.is_ok() {
            // The on-disk snapshot has been rewritten; any rdf5d-mmap-backed
            // cached Dataset is now pointing at stale bytes.
            self.bump_generation();
        }
        result
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
    use super::{LogicalSparqlDatasetView, Rdf5dSnapshot};
    use rdf5d::{
        write_file, Quint,
        writer::Term as WriterTerm,
    };
    use spargebra::SparqlParser;
    use spareval::{QueryEvaluator, QueryResults};

    #[test]
    fn logical_sparql_view_collapses_duplicate_graph_names() {
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

        let snapshot = Rdf5dSnapshot::open(&path).expect("open snapshot");
        let query = SparqlParser::new()
            .parse_query(
                "SELECT ?s WHERE {
                    GRAPH <http://example.org/graph/shared> {
                        ?s <http://example.org/name> ?o
                    }
                }",
            )
            .expect("parse query");
        let evaluator = QueryEvaluator::new();
        let prepared = evaluator.prepare(&query);
        let results = prepared
            .execute(LogicalSparqlDatasetView::new(&snapshot))
            .expect("execute query");
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
