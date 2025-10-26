use ::ontoenv::api::{OntoEnv as OntoEnvRs, ResolveTarget};
use ::ontoenv::config;
use ::ontoenv::consts::{IMPORTS, ONTOLOGY, TYPE};
use ::ontoenv::ontology::{Ontology as OntologyRs, OntologyLocation};
use ::ontoenv::options::{CacheMode, Overwrite, RefreshStrategy};
use ::ontoenv::transform;
use ::ontoenv::ToUriString;
use anyhow::Error;
use ontoenv_cli;
use oxigraph::model::{BlankNode, Literal, NamedNode, NamedOrBlankNodeRef, Term};
use pyo3::{
    prelude::*,
    types::{IntoPyDict, PyString, PyTuple},
    exceptions::PyValueError, // Correct import
};
use std::borrow::Borrow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex}; // Use Mutex

// Helper to convert anyhow::Error to PyValueError
fn anyhow_to_pyerr(e: Error) -> PyErr {
    PyValueError::new_err(e.to_string())
}

// --- MyTerm struct and From impl (seems unused, could potentially be removed if not needed elsewhere) ---
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

// --- term_to_python function ---
fn term_to_python<'a>(
    py: Python,
    rdflib: &Bound<'a, PyModule>,
    node: Term,
) -> PyResult<Bound<'a, PyAny>> {
    let dtype: Option<String> = match &node {
        Term::Literal(lit) => {
            // Use as_iri().to_string() which gives the full IRI
            let dt_iri = lit.datatype().as_iri().to_string();
            Some(dt_iri)
        }
        _ => None,
    };
    let lang: Option<&str> = match &node {
        Term::Literal(lit) => lit.language(),
        _ => None,
    };

    let res: Bound<'_, PyAny> = match &node {
        Term::NamedNode(uri) => {
            // Use as_str() which gives the IRI content directly
             rdflib.getattr("URIRef")?.call1((uri.as_str(),))?
        }
        Term::Literal(literal) => {
            match (lang, dtype) { // Prioritize lang
                (Some(lang_tag), _) => {
                     // Pass lang=lang_tag
                     let kwargs = [("lang", lang_tag.to_object(py))].into_py_dict(py);
                     rdflib.getattr("Literal")?.call((literal.value(),), Some(&kwargs))?
                }
                (None, Some(dtype_iri)) => {
                    // Pass datatype=URIRef(dtype_iri)
                    let py_uriref = rdflib.getattr("URIRef")?;
                    let py_dtype = py_uriref.call1((dtype_iri,))?;
                    let kwargs = [("datatype", py_dtype)].into_py_dict(py);
                    rdflib.getattr("Literal")?.call((literal.value(),), Some(&kwargs))?
                }
                (None, None) => rdflib.getattr("Literal")?.call1((literal.value(),))?,
            }
        }
        Term::BlankNode(id) => rdflib
            .getattr("BNode")?
            // Use id() which returns the string identifier
            .call1((id.id(),))?,
    };
    Ok(res)
}

/// Run the Rust CLI implementation and return its process-style exit code.
#[pyfunction]
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
    // Use Mutex instead of RwLock for simplicity with pyo3 methods needing &mut
    inner: Arc<Mutex<Option<OntoEnvRs>>>,
}

#[pymethods]
impl OntoEnv {
    #[new]
    #[pyo3(signature = (
        path = None,
        recreate = false,
        read_only = false,
        search_directories = None,
        require_ontology_names = false,
        strict = false,
        offline = false,
        use_cached_ontologies = false,
        resolution_policy = "default".to_owned(),
        root = None, // Changed default to None
        includes = None,
        excludes = None,
        temporary = false,
        no_search = false
    ))]
    fn new(
        path: Option<PathBuf>, // Use PathBuf directly
        recreate: bool,
        read_only: bool,
        search_directories: Option<Vec<PathBuf>>, // Use PathBuf
        require_ontology_names: bool,
        strict: bool,
        offline: bool,
        use_cached_ontologies: bool, // This maps to CacheMode below
        resolution_policy: String,
        root: Option<PathBuf>, // Use PathBuf, default None
        includes: Option<Vec<String>>,
        excludes: Option<Vec<String>>,
        temporary: bool,
        no_search: bool,
    ) -> PyResult<Self> {

        // Determine the effective root directory for configuration/discovery
        // If 'path' is provided, it dictates the root for loading/creating.
        // If only 'root' is provided, it's the starting point.
        // If neither, default to current directory "." for discovery.
        let effective_root = path.clone()
            .or(root) // Use provided root if path isn't given
            .unwrap_or_else(|| PathBuf::from(".")); // Default to current dir

        // --- Logic Branching ---

        let env_result = if temporary {
            // --- 1. Temporary Environment ---
            // If path is specified for a temporary env, use it as the config root,
            // otherwise use effective_root.
            let temp_root = path.unwrap_or(effective_root);

            let mut builder = config::Config::builder()
                .root(temp_root) // Root needed for relative paths in config
                .temporary(true)
                .require_ontology_names(require_ontology_names)
                .strict(strict)
                .offline(offline)
                .use_cached_ontologies(CacheMode::from(use_cached_ontologies))
                .resolution_policy(resolution_policy)
                .no_search(no_search); // Typically true for temp? Check Rust logic.

            if let Some(dirs) = search_directories { builder = builder.locations(dirs); }
            if let Some(incl) = includes { builder = builder.includes(incl); }
            if let Some(excl) = excludes { builder = builder.excludes(excl); }

            let cfg = builder.build().map_err(anyhow_to_pyerr)?;
            // 'recreate' is ignored for temporary
            OntoEnvRs::init(cfg, false)

        } else if recreate {
             // --- 2. Recreate Environment ---
            // Use explicit 'path' if given, otherwise 'effective_root'.
            let create_at_root = path.unwrap_or(effective_root);

            // If the creation path ends in ".ontoenv", use its parent.
            let final_create_root = if create_at_root.file_name() == Some(OsStr::new(".ontoenv")) {
                 create_at_root.parent().unwrap_or(&create_at_root).to_path_buf()
            } else {
                 create_at_root
            };

            let mut builder = config::Config::builder()
                .root(final_create_root) // Explicitly set root to creation path
                .temporary(false) // Ensure not temporary
                .require_ontology_names(require_ontology_names)
                .strict(strict)
                .offline(offline)
                .use_cached_ontologies(CacheMode::from(use_cached_ontologies))
                .resolution_policy(resolution_policy)
                .no_search(no_search); // Apply no_search if specified

            if let Some(dirs) = search_directories { builder = builder.locations(dirs); }
            if let Some(incl) = includes { builder = builder.includes(incl); }
            if let Some(excl) = excludes { builder = builder.excludes(excl); }

            let cfg = builder.build().map_err(anyhow_to_pyerr)?;
            // Force recreate (true)
            OntoEnvRs::init(cfg, true)

        } else if let Some(p) = path {
            // --- 3. Load from Explicit Path ---
            // 'recreate' is false here. Attempt to load ONLY from this path.
            // If 'p' ends in ".ontoenv", load from there directly.
            // Otherwise, assume 'p' is the root and look for '.ontoenv' inside it.
            let load_path = if p.file_name() == Some(OsStr::new(".ontoenv")) {
                p // Load directly from .ontoenv dir
            } else {
                p.join(".ontoenv") // Look inside the provided path
            };

             OntoEnvRs::load_from_directory(load_path, read_only)

        } else {
            // --- 4. Discover and Load/Create ---
            // 'path' is None, 'recreate' is false, 'temporary' is false.
            // Start discovery UPWARDS from 'effective_root'.
             match ::ontoenv::api::find_ontoenv_root_from(&effective_root) {
                Some(found_root) => {
                    // Found an existing environment (the .ontoenv dir itself), load it.
                    OntoEnvRs::load_from_directory(found_root, read_only)
                }
                None => {
                    // Not found. Create a *new* one AT 'effective_root'.
                    // Check read_only - cannot create if read_only.
                     if read_only {
                        // Construct the expected path for the error message
                        let dot_ontoenv_path = effective_root.join(".ontoenv");
                        // Use the specific error message format expected by the test
                        return Err(PyValueError::new_err(format!(
                            "OntoEnv directory not found at: \"{}\"", // Keep this format
                             dot_ontoenv_path.display()
                        )));
                     }

                    // Proceed to create at 'effective_root'
                    let mut builder = config::Config::builder()
                        .root(effective_root.clone()) // Create at the discovery start point
                        .temporary(false)
                        .require_ontology_names(require_ontology_names)
                        .strict(strict)
                        .offline(offline)
                        .use_cached_ontologies(CacheMode::from(use_cached_ontologies))
                        .resolution_policy(resolution_policy)
                        .no_search(no_search);

                    // If search_directories are provided *without* an explicit path,
                    // they become the locations relative to the new root.
                    if let Some(dirs) = search_directories {
                         builder = builder.locations(dirs);
                    } else if !no_search {
                        // Default search location is the root itself if not specified
                         builder = builder.locations(vec![effective_root.clone()]);
                    }


                    if let Some(incl) = includes { builder = builder.includes(incl); }
                    if let Some(excl) = excludes { builder = builder.excludes(excl); }

                    let cfg = builder.build().map_err(anyhow_to_pyerr)?;
                    // Create non-recreating (false)
                    OntoEnvRs::init(cfg, false)
                }
             }
        };

        // --- Final Result Handling ---
        // Map any Err from the above logic branches to PyValueError
        env_result
            .map_err(anyhow_to_pyerr) // Use your existing helper
            .map(|env| OntoEnv { inner: Arc::new(Mutex::new(Some(env))) }) // Wrap success
    }


    #[staticmethod]
    fn load_from_directory(path: PathBuf, read_only: bool) -> PyResult<Self> {
        // Assume path might be the root or the .ontoenv dir itself
        let load_path = if path.file_name() == Some(OsStr::new(".ontoenv")) {
             path
        } else {
             path.join(".ontoenv")
        };
        ::ontoenv::api::OntoEnv::load_from_directory(load_path, read_only)
            .map_err(anyhow_to_pyerr) // Map load errors using your helper
            .map(|env| OntoEnv { inner: Arc::new(Mutex::new(Some(env))) })
    }


    #[pyo3(signature = (all=false))]
    fn update(&self, all: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        // Use lock().unwrap() with Mutex
        let mut guard = inner.lock().unwrap();
        if let Some(env) = guard.as_mut() {
            env.update_all(all).map_err(anyhow_to_pyerr)?;
            // Only save if not temporary
            if !env.is_temporary() {
                env.save_to_directory().map_err(anyhow_to_pyerr)
            } else {
                Ok(())
            }
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn __repr__(&self) -> PyResult<String> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
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

    fn import_graph(
        &self,
        py: Python,
        destination_graph: &Bound<'_, PyAny>,
        uri: &str,
    ) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_mut()
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;
        let rdflib = py.import_bound("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()))
            .ok_or_else(|| {
                PyValueError::new_err(format!(
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
                .map_err(|e| PyValueError::new_err(e.to_string()))?;
            let base_ontology = NamedOrBlankNodeRef::NamedNode(ontology.as_ref());

            transform::rewrite_sh_prefixes_graph(&mut graph, base_ontology);
            transform::remove_ontology_declarations_graph(&mut graph, base_ontology);
        }
        // remove the owl:import statement for the 'uri' ontology
        transform::remove_owl_imports_graph(&mut graph, Some(&[iri.as_ref()]));

        // Use Python::with_gil for operations involving PyAny, PyTuple etc.
        Python::with_gil(|py| {
            let rdflib = py.import_bound("rdflib")?; // Re-import within GIL scope
            for triple in graph.into_iter() {
                let s: Term = triple.subject.into();
                let p: Term = triple.predicate.into();
                let o: Term = triple.object.into();

                let t = PyTuple::new_bound(
                    py,
                    &[
                        term_to_python(py, &rdflib, s)?,
                        term_to_python(py, &rdflib, p)?,
                        term_to_python(py, &rdflib, o)?,
                    ],
                );

                destination_graph.call_method1("add", (t,))?;
            }
            Ok::<(), PyErr>(())
        })?;
        Ok(())
    }

    /// List the ontologies in the imports closure of the given ontology
    #[pyo3(signature = (uri, recursion_depth = -1))]
    fn list_closure(&self, _py: Python, uri: &str, recursion_depth: i32) -> PyResult<Vec<String>> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_ref() // Use as_ref with MutexGuard
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()))
            .ok_or_else(|| {
                PyValueError::new_err(format!(
                    "Failed to resolve graph for URI: {uri}"
                ))
            })?;
        // Use id() method of OntologyRs
        let closure = env
            .get_closure(&graphid, recursion_depth)
            .map_err(anyhow_to_pyerr)?;
        let names: Vec<String> = closure.iter().map(|ont_id| ont_id.to_uri_string()).collect();
        Ok(names)
    }


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
        let rdflib = py.import_bound("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_ref() // Use as_ref
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()))
            .ok_or_else(|| {
                PyValueError::new_err(format!("No graph with URI: {uri}"))
            })?;

        let closure_ids = env
            .get_closure(&graphid, recursion_depth)
            .map_err(anyhow_to_pyerr)?;

        // Fetch OntologyRs objects for the IDs
        let closure_onts: HashSet<OntologyRs> = closure_ids
            .iter()
            .map(|id| env.get_ontology(id))
            .collect::<Result<HashSet<_>, _>>() // Collect into Result<HashSet<_>, Error>
            .map_err(anyhow_to_pyerr)?;


        let closure_names: Vec<String> = closure_ids.iter().map(|id| id.to_uri_string()).collect();

        let destination_graph = match destination_graph {
            Some(g) => g.clone(),
            None => rdflib.call_method0("Graph")?,
        };

        let union = env
            .get_union_graph(
                &closure_onts, // Pass the HashSet<OntologyRs>
                Some(rewrite_sh_prefixes),
                Some(remove_owl_imports),
            )
            .map_err(anyhow_to_pyerr)?;

        for triple in union.dataset.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();
            let t = PyTuple::new_bound(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            );
            destination_graph.call_method1("add", (t,))?;
        }

        // Remove owl:import statements
        if remove_owl_imports {
            let imports_term = term_to_python(py, &rdflib, IMPORTS.into())?;
             // Use graph_ids from the UnionGraphResult
             for ont_id in union.graph_ids {
                let ont_term = term_to_python(py, &rdflib, Term::NamedNode(ont_id))?;
                let remove_pattern = (py.None(), imports_term.clone(), ont_term);
                destination_graph.call_method1("remove", (remove_pattern,))?;
             }
        }
        Ok((destination_graph, closure_names))
    }


    /// Print the contents of the OntoEnv
    #[pyo3(signature = (includes=None))]
    fn dump(&self, _py: Python, includes: Option<String>) -> PyResult<()> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_ref() { // Use as_ref
            env.dump(includes.as_deref());
            Ok(())
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }


    #[pyo3(signature = (graph, recursion_depth=-1, fetch_missing=false))]
    fn import_dependencies<'a>(
        &self,
        py: Python<'a>,
        graph: &Bound<'a, PyAny>,
        recursion_depth: i32,
        fetch_missing: bool,
    ) -> PyResult<Vec<String>> {
        let rdflib = py.import_bound("rdflib")?;
        let py_imports_pred = term_to_python(py, &rdflib, Term::NamedNode(IMPORTS.into()))?;

        let kwargs = [("predicate", py_imports_pred)].into_py_dict(py)?;
        let objects_iter = graph.call_method("objects", (), Some(&kwargs))?;
        let builtins = py.import_bound("builtins")?;
        let objects_list = builtins.call_method1("list", (objects_iter,))?;
        let imports: Vec<String> = objects_list.extract()?;

        if imports.is_empty() {
            return Ok(Vec::new());
        }

        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_mut() // Use as_mut
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;

        let is_strict = env.is_strict();
        let mut all_ontologies = HashSet::new(); // Store OntologyRs
        let mut all_closure_names: Vec<String> = Vec::new();

        for uri in &imports {
            let iri = NamedNode::new(uri.as_str())
                .map_err(|e| PyValueError::new_err(e.to_string()))?;

            let mut graphid_opt = env.resolve(ResolveTarget::Graph(iri.clone()));

            if graphid_opt.is_none() && fetch_missing {
                let location = OntologyLocation::from_str(uri.as_str()).map_err(anyhow_to_pyerr)?;
                match env.add(location, Overwrite::Preserve, RefreshStrategy::UseCache) {
                    Ok(new_id) => {
                        graphid_opt = Some(new_id);
                    }
                    Err(e) => {
                        if is_strict {
                            return Err(anyhow_to_pyerr(e));
                        }
                        println!("Failed to fetch {uri}: {e}"); // Consider logging instead
                    }
                }
            }

            let graphid = match graphid_opt {
                Some(id) => id,
                None => {
                    if is_strict {
                        return Err(PyValueError::new_err(format!(
                            "Failed to resolve graph for URI: {}",
                            uri
                        )));
                    }
                    println!("Could not find {uri:?}"); // Consider logging
                    continue;
                }
            };

            // Use id() method of OntologyRs
            let closure_ids = env
                .get_closure(&graphid, recursion_depth)
                .map_err(anyhow_to_pyerr)?;

            for c_id in closure_ids {
                 all_closure_names.push(c_id.to_uri_string());
                 // Fetch and insert the OntologyRs object
                 let c_ont = env.get_ontology(&c_id).map_err(anyhow_to_pyerr)?;
                 all_ontologies.insert(c_ont);
            }
        }

        if all_ontologies.is_empty() {
            return Ok(Vec::new());
        }

        let union = env
            .get_union_graph(&all_ontologies, Some(true), Some(true)) // Pass HashSet<OntologyRs>
            .map_err(anyhow_to_pyerr)?;

        for triple in union.dataset.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();
            let t = PyTuple::new_bound(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            );
            graph.call_method1("add", (t,))?;
        }

        // Remove all owl:imports from the original graph
        let py_imports_pred_for_remove = term_to_python(py, &rdflib, IMPORTS.into())?;
        let remove_pattern = (py.None(), py_imports_pred_for_remove, py.None());
        graph.call_method1("remove", (remove_pattern,))?;

        all_closure_names.sort();
        all_closure_names.dedup();

        Ok(all_closure_names)
    }


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
        let rdflib = py.import_bound("rdflib")?;
        let py_imports_pred = term_to_python(py, &rdflib, Term::NamedNode(IMPORTS.into()))?;

        let kwargs = [("predicate", py_imports_pred)].into_py_dict(py)?;
        let objects_iter = graph.call_method("objects", (), Some(&kwargs))?;
        let builtins = py.import_bound("builtins")?;
        let objects_list = builtins.call_method1("list", (objects_iter,))?;
        let imports: Vec<String> = objects_list.extract()?;

        let destination_graph = match destination_graph {
            Some(g) => g.clone(),
            None => rdflib.call_method0("Graph")?,
        };

        if imports.is_empty() {
            return Ok((destination_graph, Vec::new()));
        }

        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_mut() // Use as_mut
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;

        let is_strict = env.is_strict();
        let mut all_ontologies = HashSet::new(); // Store OntologyRs
        let mut all_closure_names: Vec<String> = Vec::new();

        for uri in &imports {
            let iri = NamedNode::new(uri.as_str())
                .map_err(|e| PyValueError::new_err(e.to_string()))?;

            let mut graphid_opt = env.resolve(ResolveTarget::Graph(iri.clone()));

            if graphid_opt.is_none() && fetch_missing {
                let location = OntologyLocation::from_str(uri.as_str()).map_err(anyhow_to_pyerr)?;
                match env.add(location, Overwrite::Preserve, RefreshStrategy::UseCache) {
                    Ok(new_id) => {
                        graphid_opt = Some(new_id);
                    }
                    Err(e) => {
                        if is_strict {
                            return Err(anyhow_to_pyerr(e));
                        }
                        println!("Failed to fetch {uri}: {e}"); // Consider logging
                    }
                }
            }

            let graphid = match graphid_opt {
                Some(id) => id,
                None => {
                    if is_strict {
                        return Err(PyValueError::new_err(format!(
                            "Failed to resolve graph for URI: {}",
                            uri
                        )));
                    }
                    println!("Could not find {uri:?}"); // Consider logging
                    continue;
                }
            };

            // Use id() method
            let closure_ids = env
                .get_closure(&graphid, recursion_depth)
                .map_err(anyhow_to_pyerr)?;

             for c_id in closure_ids {
                 all_closure_names.push(c_id.to_uri_string());
                 // Fetch and insert the OntologyRs object
                 let c_ont = env.get_ontology(&c_id).map_err(anyhow_to_pyerr)?;
                 all_ontologies.insert(c_ont);
             }
        }

        if all_ontologies.is_empty() {
            return Ok((destination_graph, Vec::new()));
        }

        let union = env
            .get_union_graph(
                &all_ontologies, // Pass HashSet<OntologyRs>
                Some(rewrite_sh_prefixes),
                Some(remove_owl_imports),
            )
            .map_err(anyhow_to_pyerr)?;

        for triple in union.dataset.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();
            let t = PyTuple::new_bound(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            );
            destination_graph.call_method1("add", (t,))?;
        }

        if remove_owl_imports {
             let imports_term = term_to_python(py, &rdflib, IMPORTS.into())?;
             // Use graph_ids from the UnionGraphResult
             for ont_id in union.graph_ids {
                let ont_term = term_to_python(py, &rdflib, Term::NamedNode(ont_id))?;
                let remove_pattern = (py.None(), imports_term.clone(), ont_term);
                destination_graph.call_method1("remove", (remove_pattern,))?;
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
        let mut guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_mut() // Use as_mut
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;

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
        let mut guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_mut() // Use as_mut
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;
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
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_ref() // Use as_ref
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;
        let importers = env.get_importers(&iri).map_err(anyhow_to_pyerr)?;
        let names: Vec<String> = importers.iter().map(|ont_id| ont_id.to_uri_string()).collect();
        Ok(names)
    }

    /// Get the ontology metadata with the given URI
    fn get_ontology(&self, uri: &str) -> PyResult<PyOntology> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_ref() // Use as_ref
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()))
            .ok_or_else(|| {
                PyValueError::new_err(format!(
                    "Failed to resolve graph for URI: {uri}"
                ))
            })?;
        let ont = env.get_ontology(&graphid).map_err(anyhow_to_pyerr)?;
        Ok(PyOntology { inner: ont })
    }

    /// Get the graph with the given URI as an rdflib.Graph
    fn get_graph(&self, py: Python, uri: &Bound<'_, PyString>) -> PyResult<Py<PyAny>> {
        let rdflib = py.import_bound("rdflib")?;
        let iri = NamedNode::new(uri.to_string())
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        let graph = { // Scoped lock
            let inner = self.inner.clone();
            let guard = inner.lock().unwrap(); // Use lock()
            let env = guard.as_ref().ok_or_else(|| { // Use as_ref
                PyValueError::new_err("OntoEnv is closed")
            })?;
            let graphid = env.resolve(ResolveTarget::Graph(iri)).ok_or_else(|| {
                PyValueError::new_err(format!(
                    "Failed to resolve graph for URI: {uri}"
                ))
            })?;

            env.get_graph(&graphid).map_err(anyhow_to_pyerr)?
        }; // Lock released here

        let res = rdflib.call_method0("Graph")?;
        for triple in graph.into_iter() {
            let s: Term = triple.subject.into();
            let p: Term = triple.predicate.into();
            let o: Term = triple.object.into();

            let t = PyTuple::new_bound(
                py,
                &[
                    term_to_python(py, &rdflib, s)?,
                    term_to_python(py, &rdflib, p)?,
                    term_to_python(py, &rdflib, o)?,
                ],
            );

            res.call_method1("add", (t,))?;
        }
        Ok(res.into())
    }

    /// Get the names of all ontologies in the OntoEnv
    fn get_ontology_names(&self) -> PyResult<Vec<String>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_ref() // Use as_ref
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;
        let names: Vec<String> = env.ontologies().keys().map(|k| k.to_uri_string()).collect();
        Ok(names)
    }

    /// Convert the OntoEnv to an in-memory rdflib.Dataset populated with all named graphs
    fn to_rdflib_dataset(&self, py: Python) -> PyResult<Py<PyAny>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        let env = guard
            .as_ref() // Use as_ref
            .ok_or_else(|| PyValueError::new_err("OntoEnv is closed"))?;
        let rdflib = py.import_bound("rdflib")?;
        let dataset_cls = rdflib.getattr("Dataset")?;
        let ds = dataset_cls.call0()?;
        let uriref = rdflib.getattr("URIRef")?;

        for (_gid, ont) in env.ontologies().iter() {
            let id_str = ont.id().name().as_str();
            let id_py = uriref.call1((id_str,))?;
            let kwargs = [("identifier", id_py.clone())].into_py_dict(py)?;
            let ctx = ds.call_method("graph", (), Some(&kwargs))?;

            let graph = env.get_graph(ont.id()).map_err(anyhow_to_pyerr)?;
            for t in graph.iter() {
                let s: Term = t.subject.into();
                let p: Term = t.predicate.into();
                let o: Term = t.object.into();
                let triple = PyTuple::new_bound(
                    py,
                    &[
                        term_to_python(py, &rdflib, s)?,
                        term_to_python(py, &rdflib, p)?,
                        term_to_python(py, &rdflib, o)?,
                    ],
                );
                ctx.call_method1("add", (triple,))?;
            }
        }

        Ok(ds.into())
    }

    // Config accessors
    fn is_offline(&self) -> PyResult<bool> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_ref() { // Use as_ref
            Ok(env.is_offline())
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn set_offline(&mut self, offline: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_mut() { // Use as_mut
            env.set_offline(offline);
            if !env.is_temporary() {
                env.save_to_directory().map_err(anyhow_to_pyerr)
            } else {
                Ok(())
            }
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn is_strict(&self) -> PyResult<bool> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_ref() { // Use as_ref
            Ok(env.is_strict())
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn set_strict(&mut self, strict: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_mut() { // Use as_mut
            env.set_strict(strict);
            if !env.is_temporary() {
                env.save_to_directory().map_err(anyhow_to_pyerr)
            } else {
                Ok(())
            }
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn requires_ontology_names(&self) -> PyResult<bool> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_ref() { // Use as_ref
            Ok(env.requires_ontology_names())
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn set_require_ontology_names(&mut self, require: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_mut() { // Use as_mut
            env.set_require_ontology_names(require);
            if !env.is_temporary() {
                env.save_to_directory().map_err(anyhow_to_pyerr)
            } else {
                Ok(())
            }
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn no_search(&self) -> PyResult<bool> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_ref() { // Use as_ref
            Ok(env.no_search())
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn set_no_search(&mut self, no_search: bool) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_mut() { // Use as_mut
            env.set_no_search(no_search);
            if !env.is_temporary() {
                env.save_to_directory().map_err(anyhow_to_pyerr)
            } else {
                Ok(())
            }
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn resolution_policy(&self) -> PyResult<String> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_ref() { // Use as_ref
            Ok(env.resolution_policy().to_string())
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    fn set_resolution_policy(&mut self, policy: String) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_mut() { // Use as_mut
            env.set_resolution_policy(policy);
            if !env.is_temporary() {
                env.save_to_directory().map_err(anyhow_to_pyerr)
            } else {
                Ok(())
            }
        } else {
            Err(PyValueError::new_err("OntoEnv is closed"))
        }
    }

    pub fn store_path(&self) -> PyResult<Option<String>> {
        let inner = self.inner.clone();
        let guard = inner.lock().unwrap(); // Use lock()
        if let Some(env) = guard.as_ref() { // Use as_ref
            match env.store_path() {
                Some(path) => {
                    // Return the .ontoenv directory path itself
                     Ok(Some(path.to_string_lossy().to_string()))
                }
                None => Ok(None), // Temporary env
            }
        } else {
            Ok(None) // Env is closed
        }
    }

    pub fn close(&mut self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap(); // Use lock()
            if let Some(env) = guard.as_mut() { // Use as_mut
                if !env.is_temporary() {
                    env.save_to_directory().map_err(anyhow_to_pyerr)?;
                }
                env.flush().map_err(anyhow_to_pyerr)?;
            }
            *guard = None; // Set inner to None
            Ok(())
        })
    }

    pub fn flush(&mut self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let inner = self.inner.clone();
            let mut guard = inner.lock().unwrap(); // Use lock()
            if let Some(env) = guard.as_mut() { // Use as_mut
                env.flush().map_err(anyhow_to_pyerr)
            } else {
                Err(PyValueError::new_err("OntoEnv is closed"))
            }
        })
    }
}

#[pymodule(pyontoenv)] // Correct module name
fn pyontoenv(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> { // Correct function name
    // Initialize logging when the python module is loaded.
    // Use try_init to avoid panic if logging is already initialized (e.g., in tests).
    // Note: This might conflict if the user configures logging differently. Consider making it optional.
    let _ = env_logger::builder().is_test(true).try_init(); // Use is_test for test runs
    ::ontoenv::api::init_logging(); // Your custom logging init, ensure it's idempotent

    m.add_class::<OntoEnv>()?;
    m.add_class::<PyOntology>()?;
    m.add_function(wrap_pyfunction!(run_cli, m)?)?;
    // add version attribute
    m.add("version", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
