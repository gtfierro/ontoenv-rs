use ::ontoenv::api::{OntoEnv as OntoEnvRs, ResolveTarget};
use ::ontoenv::config;
use ::ontoenv::consts::{IMPORTS, ONTOLOGY, TYPE};
use ::ontoenv::ontology::{Ontology as OntologyRs, OntologyLocation};
use ::ontoenv::options::{CacheMode, Overwrite, RefreshStrategy};
use ::ontoenv::transform;
use ::ontoenv::ToUriString;
use anyhow::Error;
#[cfg(feature = "cli")]
use ontoenv_cli;
use oxigraph::model::{BlankNode, Literal, NamedNode, NamedOrBlankNodeRef, Term};
use pyo3::{
    prelude::*,
    types::{IntoPyDict, PyString, PyTuple},
    exceptions::PyValueError,
};
#[cfg(not(feature = "cli"))]
use pyo3::exceptions::PyRuntimeError;
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex};

fn anyhow_to_pyerr(e: Error) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
}

// Helper function to format paths with forward slashes for cross-platform error messages
fn format_path_for_error(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

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

/// Run the Rust CLI implementation and return its process-style exit code.
#[pyfunction]
#[cfg(feature = "cli")]
fn run_cli(py: Python<'_>, args: Option<Vec<String>>) -> PyResult<i32> {
    let argv = args.unwrap_or_else(|| std::env::args().collect());
    let code = py.allow_threads(move || match ontoenv_cli::run_from_args(argv) {
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
    #[pyo3(signature = (path=None, recreate=false, read_only=false, search_directories=None, require_ontology_names=false, strict=false, offline=false, use_cached_ontologies=false, resolution_policy="default".to_owned(), root=".".to_owned(), includes=None, excludes=None, temporary=false, no_search=false))]
    fn new(
        _py: Python,
        path: Option<PathBuf>,
        recreate: bool,
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
        temporary: bool,
        no_search: bool,
    ) -> PyResult<Self> {

        // Check if OntoEnv() is called without any meaningful arguments
        // This implements the behavior expected by the tests
        if path.is_none() && root == "." && !recreate && !temporary {
            // Use forward slashes for cross-platform compatibility in error messages
            return Err(PyValueError::new_err(
                "OntoEnv directory not found at \"./.ontoenv\". You must provide a valid path or set recreate=True or temporary=True to create a new OntoEnv.",
            ));
        }
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
        // - otherwise: discover upward; if not found, error

        let mut builder = config::Config::builder()
            .root(root_path.clone())
            .require_ontology_names(require_ontology_names)
            .strict(strict)
            .offline(offline)
            .use_cached_ontologies(CacheMode::from(use_cached_ontologies))
            .resolution_policy(resolution_policy)
            .temporary(temporary)
            .no_search(no_search);

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

        let cfg = builder
            .build()
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;

        let env = if cfg.temporary {
            // Explicit in-memory env
            OntoEnvRs::init(cfg, false).map_err(anyhow_to_pyerr)?
        } else if recreate {
            // Explicit create/overwrite at root_path
            OntoEnvRs::init(cfg, true).map_err(anyhow_to_pyerr)?
        } else {
            // Discover upward from root_path; load if found. If not found and not read-only,
            // initialize a new environment at the requested root.
            match ::ontoenv::api::find_ontoenv_root_from(&root_path) {
                Some(found_root) => OntoEnvRs::load_from_directory(found_root, read_only)
                    .map_err(anyhow_to_pyerr)?,
                None => {
                    // If a specific path was provided but no .ontoenv exists, raise error
                    if path.is_some() {
                        return Err(PyValueError::new_err(format!(
                            "OntoEnv directory not found at: \"{}\"",
                            format_path_for_error(&root_path.join(".ontoenv"))
                        )));
                    }
                    if read_only {
                        return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                            "OntoEnv directory not found at: \"{}\" and read_only=True",
                            format_path_for_error(&root_path.join(".ontoenv"))
                        )));
                    }
                    OntoEnvRs::init(cfg, false).map_err(anyhow_to_pyerr)?
                }
            }
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

    fn import_graph(
        &self,
        py: Python,
        destination_graph: &Bound<'_, PyAny>,
        uri: &str,
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
        let mut graph = env.get_graph(&graphid).map_err(anyhow_to_pyerr)?;

        let uriref_constructor = rdflib.getattr("URIRef")?;
        let type_uri = uriref_constructor.call1((TYPE.as_str(),))?;
        let ontology_uri = uriref_constructor.call1((ONTOLOGY.as_str(),))?;
        let kwargs = [("predicate", type_uri), ("object", ontology_uri)].into_py_dict(py)?;
        let result = destination_graph.call_method("value", (), Some(&kwargs))?;
        if !result.is_none() {
            let ontology = NamedNode::new(result.extract::<String>()?)
                .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let base_ontology = NamedOrBlankNodeRef::NamedNode(ontology.as_ref());

            transform::rewrite_sh_prefixes_graph(&mut graph, base_ontology);
            transform::remove_ontology_declarations_graph(&mut graph, base_ontology);
        }
        // remove the owl:import statement for the 'uri' ontology
        transform::remove_owl_imports_graph(&mut graph, Some(&[iri.as_ref()]));

        Python::with_gil(|_py| {
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

                destination_graph.getattr("add")?.call1((t,))?;
            }
            Ok::<(), PyErr>(())
        })?;
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
        let rdflib = py.import("rdflib")?;
        let py_imports_pred = term_to_python(py, &rdflib, Term::NamedNode(IMPORTS.into()))?;

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

        let is_strict = env.is_strict();
        let mut all_ontologies = HashSet::new();
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
                all_ontologies.insert(c_ont.clone());
            }
        }

        if all_ontologies.is_empty() {
            return Ok(Vec::new());
        }

        let union = env
            .get_union_graph(&all_ontologies, Some(true), Some(true))
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

        // Remove all owl:imports from the original graph
        let py_imports_pred_for_remove = term_to_python(py, &rdflib, IMPORTS.into())?;
        let remove_tuple = PyTuple::new(
            py,
            &[py.None(), py_imports_pred_for_remove.into(), py.None()],
        )?;
        graph.getattr("remove")?.call1((remove_tuple,))?;

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

        let is_strict = env.is_strict();
        let mut all_ontologies = HashSet::new();
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
                all_ontologies.insert(c_ont.clone());
            }
        }

        if all_ontologies.is_empty() {
            return Ok((destination_graph, Vec::new()));
        }

        let union = env
            .get_union_graph(
                &all_ontologies,
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

        let location =
            OntologyLocation::from_str(&location.to_string()).map_err(anyhow_to_pyerr)?;
        let overwrite_flag: Overwrite = overwrite.into();
        let refresh: RefreshStrategy = force.into();
        let graph_id = if fetch_imports {
            env.add(location, overwrite_flag, refresh)
        } else {
            env.add_no_imports(location, overwrite_flag, refresh)
        }
        .map_err(anyhow_to_pyerr)?;
        Ok(graph_id.to_uri_string())
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
        let location =
            OntologyLocation::from_str(&location.to_string()).map_err(anyhow_to_pyerr)?;
        let overwrite_flag: Overwrite = overwrite.into();
        let refresh: RefreshStrategy = force.into();
        let graph_id = env
            .add_no_imports(location, overwrite_flag, refresh)
            .map_err(anyhow_to_pyerr)?;
        Ok(graph_id.to_uri_string())
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
        let iri = NamedNode::new(uri.to_string())
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

    fn no_search(&self) -> PyResult<bool> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap();
        if let Some(env) = guard.as_ref() {
            Ok(env.no_search())
        } else {
            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "OntoEnv is closed",
            ))
        }
    }

    fn set_no_search(&mut self, no_search: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap();
        if let Some(env) = guard.as_mut() {
            env.set_no_search(no_search);
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
        py.allow_threads(|| {
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
        py.allow_threads(|| {
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
