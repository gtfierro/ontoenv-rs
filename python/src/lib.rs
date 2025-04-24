use ::ontoenv::api::{OntoEnv as OntoEnvRs, ResolveTarget};
use ::ontoenv::config;
use ::ontoenv::consts::{IMPORTS, ONTOLOGY, TYPE};
use ::ontoenv::ontology::OntologyLocation;
use ::ontoenv::transform;
use anyhow::Error;
use oxigraph::model::{BlankNode, Literal, NamedNode, SubjectRef, Term};
use pyo3::{
    prelude::*,
    types::{IntoPyDict, PyString, PyTuple},
};
use std::borrow::Borrow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Once};

fn anyhow_to_pyerr(e: Error) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
}

static INIT: Once = Once::new();

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
        Term::Triple(_) => {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Triples are not supported",
            ))
        }
    };
    Ok(res)
}

#[pyclass]
#[derive(Clone)]
struct Config {
    cfg: config::Config,
}

#[pymethods]
impl Config {
    #[new]
    #[pyo3(signature = (search_directories=None, require_ontology_names=false, strict=false, offline=false, resolution_policy="default".to_owned(), root=".".to_owned(), includes=None, excludes=None, temporary=false))]
    fn new(
        search_directories: Option<Vec<String>>,
        require_ontology_names: bool,
        strict: bool,
        offline: bool,
        resolution_policy: String,
        root: String,
        includes: Option<Vec<String>>,
        excludes: Option<Vec<String>>,
        temporary: bool,
    ) -> PyResult<Self> {
        Ok(Config {
            cfg: config::Config::new(
                root.to_string().into(),
                search_directories.map(|dirs| {
                    dirs.iter()
                        .map(|s| s.to_string().into())
                        .collect::<Vec<PathBuf>>()
                }),
                includes
                    .unwrap_or_default()
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
                excludes
                    .unwrap_or_default()
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
                require_ontology_names,
                strict,
                offline,
                resolution_policy.to_string(),
                false,
                temporary,
            )
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?,
        })
    }
}

#[pyclass]
struct OntoEnv {
    inner: Arc<Mutex<OntoEnvRs>>,
}

#[pymethods]
impl OntoEnv {
    #[new]
    #[pyo3(signature = (config=None, path=Some(Path::new(".").to_owned()), recreate=false, read_only=false))]
    fn new(
        _py: Python,
        config: Option<Config>,
        path: Option<PathBuf>,
        recreate: bool,
        read_only: bool,
    ) -> PyResult<Self> {
        // wrap env_logger::init() in a Once to ensure it's only called once. This can
        // happen if a user script creates multiple OntoEnv instances
        INIT.call_once(|| {
            env_logger::init();
        });

        let config_path = path.unwrap_or_else(|| PathBuf::from("."));
        let env = if let Some(c) = config {
            // if temporary is true, create a new OntoEnv
            if c.cfg.temporary {
                OntoEnvRs::init(c.cfg, recreate).map_err(anyhow_to_pyerr)
            } else if !recreate && config_path.join(".ontoenv").exists() {
                // if temporary is false, load from the directory
                OntoEnvRs::load_from_directory(config_path, read_only).map_err(anyhow_to_pyerr)
            } else {
                // if temporary is false and recreate is true or the directory doesn't exist, create a new OntoEnv
                OntoEnvRs::init(c.cfg, recreate).map_err(anyhow_to_pyerr)
            }
        } else {
            // If no config but a valid path is given, attempt to load from the directory
            OntoEnvRs::load_from_directory(config_path, read_only).map_err(anyhow_to_pyerr)
        }?;

        let inner = Arc::new(Mutex::new(env));
        let mut env = inner.lock().unwrap();
        env.update().map_err(anyhow_to_pyerr)?;
        env.save_to_directory().map_err(anyhow_to_pyerr)?;

        Ok(OntoEnv {
            inner: inner.clone(),
        })
    }

    fn update(&self) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut env = inner.lock().unwrap();
        env.update().map_err(anyhow_to_pyerr)?;
        env.save_to_directory().map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    // fn is_read_only(&self) -> PyResult<bool> {
    //     let inner = self.inner.clone();
    //     let env = inner.lock().unwrap();
    //     Ok(env.is_read_only())
    // }

    fn __repr__(&self) -> PyResult<String> {
        let inner = self.inner.clone();
        let env = inner.lock().unwrap();
        let stats = env.stats().map_err(anyhow_to_pyerr)?;
        Ok(format!(
            "<OntoEnv: {} ontologies, {} graphs, {} triples>",
            stats.num_ontologies, stats.num_graphs, stats.num_triples,
        ))
    }

    // The following methods will now access the inner OntoEnv in a thread-safe manner:

    fn import_graph(
        &self,
        py: Python,
        destination_graph: &Bound<'_, PyAny>,
        uri: &str,
    ) -> PyResult<()> {
        let inner = self.inner.clone();
        let env = inner.lock().unwrap();
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()).into())
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Failed to resolve graph for URI: {}",
                    uri
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
            let base_ontology: SubjectRef = SubjectRef::NamedNode(ontology.as_ref());

            transform::rewrite_sh_prefixes_graph(&mut graph, base_ontology);
            transform::remove_ontology_declarations_graph(&mut graph, base_ontology);
        }
        // remove the owl:import statement for the 'uri' ontology
        transform::remove_owl_imports_graph(&mut graph, Some(&[(&iri).into()]));

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
    #[pyo3(signature = (uri))]
    fn list_closure(&self, _py: Python, uri: &str) -> PyResult<Vec<String>> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let env = inner.lock().unwrap();
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()).into())
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "Failed to resolve graph for URI: {}",
                    uri
                ))
            })?;
        let ont = env.ontologies().get(&graphid).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology {} not found", iri))
        })?;
        let closure = env
            .get_dependency_closure(ont.id())
            .map_err(anyhow_to_pyerr)?;
        let names: Vec<String> = closure.iter().map(|ont| ont.name().to_string()).collect();
        Ok(names)
    }

    /// Merge all graphs in the imports closure of the given ontology into a single graph. If
    /// destination_graph is provided, add the merged graph to the destination_graph. If not,
    /// return the merged graph.
    #[pyo3(signature = (uri, destination_graph=None, rewrite_sh_prefixes=false, remove_owl_imports=false))]
    fn get_closure<'a>(
        &self,
        py: Python<'a>,
        uri: &str,
        destination_graph: Option<&Bound<'a, PyAny>>,
        rewrite_sh_prefixes: bool,
        remove_owl_imports: bool,
    ) -> PyResult<Bound<'a, PyAny>> {
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let env = inner.lock().unwrap();
        let graphid = env
            .resolve(ResolveTarget::Graph(iri.clone()).into())
            .ok_or_else(|| {
                PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                    "No graph with URI: {}",
                    uri
                ))
            })?;
        let ont = env.ontologies().get(&graphid).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology {} not found", iri))
        })?;
        let closure = env
            .get_dependency_closure(ont.id())
            .map_err(anyhow_to_pyerr)?;
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
        Python::with_gil(|_py| {
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

            // Remove each url in the closure from the destination_graph
            return Ok::<Bound<'_, PyAny>, PyErr>(destination_graph);
        })
    }

    /// Print the contents of the OntoEnv
    #[pyo3(signature = (includes=None))]
    fn dump(&self, _py: Python, includes: Option<String>) -> PyResult<()> {
        let inner = self.inner.clone();
        let env = inner.lock().unwrap();
        env.dump(includes.as_deref());
        Ok(())
    }

    /// Import the dependencies of the given graph into the graph. Removes the owl:imports
    /// of all imported ontologies.
    #[pyo3(signature = (graph))]
    fn import_dependencies<'a>(
        &self,
        py: Python<'a>,
        graph: &Bound<'a, PyAny>,
    ) -> PyResult<Bound<'a, PyAny>> {
        let rdflib = py.import("rdflib")?;
        let py_rdf_type = term_to_python(py, &rdflib, Term::NamedNode(TYPE.into()))?;
        let py_ontology = term_to_python(py, &rdflib, Term::NamedNode(ONTOLOGY.into()))?;
        let value_fun: Py<PyAny> = graph.getattr("value")?.into();
        let kwargs = [("predicate", py_rdf_type), ("object", py_ontology)].into_py_dict(py)?;
        let ontology = value_fun.call(py, (), Some(&kwargs))?;

        if ontology.is_none(py) {
            return Ok(graph.clone());
        }

        let ontology = ontology.to_string();

        self.get_closure(py, &ontology, Some(graph), true, true)
    }

    /// Add a new ontology to the OntoEnv
    fn add(&self, location: &Bound<'_, PyAny>) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut env = inner.lock().unwrap();
        let location =
            OntologyLocation::from_str(&location.to_string()).map_err(anyhow_to_pyerr)?;
        env.add(location, true).map_err(anyhow_to_pyerr)?;
        env.save_to_directory().map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    /// Refresh the OntoEnv by re-loading all remote graphs and loading
    /// any local graphs which have changed since the last update
    fn refresh(&self) -> PyResult<()> {
        let inner = self.inner.clone();
        let mut env = inner.lock().unwrap();
        env.update().map_err(anyhow_to_pyerr)?;
        env.save_to_directory().map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    /// Get the names of all ontologies that depend on the given ontology
    fn get_dependents(&self, uri: &str) -> PyResult<Vec<String>> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.clone();
        let env = inner.lock().unwrap();
        let dependents = env.get_dependents(&iri).map_err(anyhow_to_pyerr)?;
        let names: Vec<String> = dependents
            .iter()
            .map(|ont| ont.name().to_string())
            .collect();
        Ok(names)
    }

    /// Export the graph with the given URI to an rdflib.Graph
    fn get_graph(&self, py: Python, uri: &Bound<'_, PyString>) -> PyResult<Py<PyAny>> {
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(uri.to_string())
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let graph = {
            let inner = self.inner.clone();
            let env = inner.lock().unwrap();
            let graphid = env
                .resolve(ResolveTarget::Graph(iri).into())
                .ok_or_else(|| {
                    PyErr::new::<pyo3::exceptions::PyValueError, _>(format!(
                        "Failed to resolve graph for URI: {}",
                        uri
                    ))
                })?;
            println!("graphid: {:?}", graphid);
            let graph = env.get_graph(&graphid).map_err(anyhow_to_pyerr)?;
            graph
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
        let env = inner.lock().unwrap();
        let names: Vec<String> = env
            .ontologies()
            .keys()
            .map(|k| k.name().to_string())
            .collect();
        Ok(names)
    }

    /// Convert the OntoEnv to an rdflib.Dataset
    fn to_rdflib_dataset(&self, py: Python) -> PyResult<Py<PyAny>> {
        // rdflib.ConjunctiveGraph(store="Oxigraph")
        let inner = self.inner.clone();
        let env = inner.lock().unwrap();
        let rdflib = py.import("rdflib")?;
        let dataset = rdflib.getattr("Dataset")?;

        // call Dataset(store="Oxigraph")
        let kwargs = [("store", "Oxigraph")].into_py_dict(py)?;
        let store = dataset.call((), Some(&kwargs))?;
        let path = env.store_path().unwrap();
        store.getattr("open")?.call1((path,))?;
        Ok(store.into())
    }

    pub fn store_path(&self) -> PyResult<Option<String>> {
        let inner = self.inner.clone();
        let env = inner.lock().unwrap();
        match env.store_path() {
            Some(path) => Ok(Some(path.to_string_lossy().to_string())),
            None => Ok(None), // Return None if the path doesn't exist (e.g., temporary env)
        }
    }

    // Wrapper method to raise error if store_path is None, matching previous panic behavior
    // but providing a Python-level error. Or tests can check for None.
    // Let's keep the Option return type for flexibility and adjust tests.

    pub fn flush(&mut self, py: Python<'_>) -> PyResult<()> {
        py.allow_threads(|| {
            let inner = self.inner.clone();
            let mut env = inner.lock().unwrap();
            env.flush().map_err(anyhow_to_pyerr)?;
            Ok(())
        })
    }
}

#[pymodule]
fn ontoenv(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Config>()?;
    m.add_class::<OntoEnv>()?;
    // add version attribute
    m.add("version", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
