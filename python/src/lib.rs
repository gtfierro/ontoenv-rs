#![feature(once_cell_try)]
use ::ontoenv as ontoenvrs;
use ::ontoenv::consts::{ONTOLOGY, TYPE};
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
use std::sync::{Arc, Mutex, Once, OnceLock};

static INIT: Once = Once::new();
static ONTOENV_SINGLETON: OnceLock<Arc<Mutex<ontoenvrs::OntoEnv>>> = OnceLock::new();

fn anyhow_to_pyerr(e: Error) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
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
        Term::Triple(_) => {
            return Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Triples are not supported",
            ))
        }
    };
    Ok(res)
}

#[pyclass]
struct Config {
    cfg: ontoenvrs::config::Config,
}

#[pymethods]
impl Config {
    #[new]
    #[pyo3(signature = (search_directories=None, require_ontology_names=false, strict=false, offline=false, resolution_policy="default".to_owned(), root=".".to_owned(), includes=vec![], excludes=vec![]))]
    fn new(
        search_directories: Option<Vec<String>>,
        require_ontology_names: bool,
        strict: bool,
        offline: bool,
        resolution_policy: String,
        root: String,
        includes: Option<Vec<String>>,
        excludes: Option<Vec<String>>,
    ) -> PyResult<Self> {
        Ok(Config {
            cfg: ontoenvrs::config::Config::new(
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
            )
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?,
        })
    }
}

#[pyclass]
struct OntoEnv {
    inner: Arc<Mutex<ontoenvrs::OntoEnv>>,
}

#[pymethods]
impl OntoEnv {
    #[new]
    #[pyo3(signature = (config=None, path=Path::new(".").to_owned(), recreate=false, read_only=false))]
    fn new(
        _py: Python,
        config: Option<&Config>,
        path: Option<PathBuf>,
        recreate: bool,
        read_only: bool,
    ) -> PyResult<Self> {
        // wrap env_logger::init() in a Once to ensure it's only called once. This can
        // happen if a user script creates multiple OntoEnv instances
        INIT.call_once(|| {
            env_logger::init();
        });

        let config_path = path
            .as_ref()
            .map(|p| p.join(".ontoenv").join("ontoenv.json"));
        println!("Config path: {:?}", config_path);

        let env = ONTOENV_SINGLETON.get_or_try_init(|| {
            // if no Config provided, but there is a path, load the OntoEnv from file
            // otherwise, create a new OntoEnv
            if config.is_none() && config_path.is_some() && config_path.as_ref().unwrap().exists(){
                if let Ok(env) = ontoenvrs::OntoEnv::from_file(&config_path.unwrap(), read_only) {
                    println!("Loaded OntoEnv from file");
                    return Ok(Arc::new(Mutex::new(env)));
                }
            }

            // if config is provided, create a new OntoEnv with the provided config
            if let Some(c) = config {
                println!("Creating new OntoEnv with provided config");
                let inner = ontoenvrs::OntoEnv::new(c.cfg.clone(), recreate)
                    .map_err(anyhow_to_pyerr)?;
                return Ok(Arc::new(Mutex::new(inner)));
            }

            Err(PyErr::new::<pyo3::exceptions::PyValueError, _>(
                "Either a Config or a path must be provided. If path provided, there must be a valid OntoEnv directory at the path",
            ))

        })?;

        {
            let mut env = env.lock().unwrap();
            env.update().map_err(anyhow_to_pyerr)?;
            env.save_to_directory().map_err(anyhow_to_pyerr)?;
        }

        Ok(OntoEnv { inner: env.clone() })
    }

    fn update(&self) -> PyResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.update().map_err(anyhow_to_pyerr)?;
        inner.save_to_directory().map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    fn __repr__(&self) -> PyResult<String> {
        let inner = self.inner.lock().unwrap();
        Ok(format!(
            "<OntoEnv: {} graphs, {} triples>",
            inner.num_graphs(),
            inner.num_triples().map_err(anyhow_to_pyerr)?
        ))
    }

    // The following methods will now access the inner OntoEnv in a thread-safe manner:

    fn import_graph(
        &self,
        py: Python,
        destination_graph: &Bound<'_, PyAny>,
        uri: &str,
    ) -> PyResult<()> {
        let inner = self.inner.lock().unwrap();
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let ont = inner.get_ontology_by_name(iri.as_ref()).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology {} not found", iri))
        })?;
        let mut graph = ont.graph().map_err(anyhow_to_pyerr)?;

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
        transform::remove_owl_imports_graph(&mut graph);

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

    #[pyo3(signature = (uri))]
    fn list_closure(&self, py: Python, uri: &str) -> PyResult<Vec<String>> {
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.lock().unwrap();
        let ont = inner.get_ontology_by_name(iri.as_ref()).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology {} not found", iri))
        })?;
        let closure = inner
            .get_dependency_closure(ont.id())
            .map_err(anyhow_to_pyerr)?;
        let names: Vec<String> = closure.iter().map(|ont| ont.name().to_string()).collect();
        Ok(names)
    }

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
        let inner = self.inner.lock().unwrap();
        let ont = inner.get_ontology_by_name(iri.as_ref()).ok_or_else(|| {
            PyErr::new::<pyo3::exceptions::PyValueError, _>(format!("Ontology {} not found", iri))
        })?;
        let closure = inner
            .get_dependency_closure(ont.id())
            .map_err(anyhow_to_pyerr)?;
        // if destination_graph is null, create a new rdflib.Graph()
        let destination_graph = match destination_graph {
            Some(g) => g.clone(),
            None => rdflib.getattr("Graph")?.call0()?,
        };
        let graph = inner
            .get_union_graph(
                &closure,
                Some(rewrite_sh_prefixes),
                Some(remove_owl_imports),
            )
            .map_err(anyhow_to_pyerr)?;
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
            return Ok::<Bound<'_, PyAny>, PyErr>(destination_graph);
        })
    }

    #[pyo3(signature = (includes=None))]
    fn dump(&self, py: Python, includes: Option<String>) -> PyResult<()> {
        let inner = self.inner.lock().unwrap();
        inner.dump(includes.as_deref());
        Ok(())
    }

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

    fn add(&self, location: &Bound<'_, PyAny>) -> PyResult<()> {
        let mut inner = self.inner.lock().unwrap();
        let location =
            OntologyLocation::from_str(&location.to_string()).map_err(anyhow_to_pyerr)?;
        inner.add(location).map_err(anyhow_to_pyerr)?;
        inner.save_to_directory().map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    fn refresh(&self) -> PyResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.update().map_err(anyhow_to_pyerr)?;
        inner.save_to_directory().map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    fn get_graph(&self, py: Python, uri: &Bound<'_, PyString>) -> PyResult<Py<PyAny>> {
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(uri.to_string())
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let inner = self.inner.lock().unwrap();
        let graph = inner
            .get_graph_by_name(iri.as_ref())
            .map_err(anyhow_to_pyerr)?;
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

    fn get_ontology_names(&self) -> PyResult<Vec<String>> {
        let inner = self.inner.lock().unwrap();
        let names: Vec<String> = inner
            .ontologies()
            .keys()
            .map(|k| k.name().to_string())
            .collect();
        Ok(names)
    }

    /// Convert the OntoEnv to an rdflib.Dataset
    fn to_rdflib_dataset(&self, py: Python) -> PyResult<Py<PyAny>> {
        // rdflib.ConjunctiveGraph(store="Oxigraph")
        let inner = self.inner.lock().unwrap();
        let rdflib = py.import("rdflib")?;
        let dataset = rdflib.getattr("Dataset")?;

        // call Dataset(store="Oxigraph")
        let kwargs = [("store", "Oxigraph")].into_py_dict(py)?;
        let store = dataset.call((), Some(&kwargs))?;
        let path = inner.store_path().map_err(anyhow_to_pyerr)?.to_string();
        store.getattr("open")?.call1((path,))?;
        Ok(store.into())
    }
}

#[pymodule]
fn ontoenv(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Config>()?;
    m.add_class::<OntoEnv>()?;
    Ok(())
}
