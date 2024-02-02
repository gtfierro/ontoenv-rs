use ::ontoenv as ontoenvrs;
use ::ontoenv::consts::{TYPE, ONTOLOGY};
use log::{error, info, debug};
use std::borrow::Borrow;
use anyhow::Error;
use oxigraph::model::{BlankNode, Literal, NamedNode, Term};
use pyo3::{
    prelude::*,
    types::{PyBool, PyString, PyTuple},
};

fn anyhow_to_pyerr(e: Error) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
}

struct MyTerm(Term);
impl From<Result<&PyAny, pyo3::PyErr>> for MyTerm {
    fn from(s: Result<&PyAny, pyo3::PyErr>) -> Self {
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

fn term_to_python<'a>(py: Python, rdflib: &'a PyModule, node: Term) -> PyResult<&'a PyAny> {
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

    let res: &PyAny = match &node {
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
    fn new(
        root: Bound<'_, PyString>,
        search_directories: Vec<Bound<'_, PyString>>,
        includes: Vec<Bound<'_, PyString>>,
        excludes: Vec<Bound<'_, PyString>>,
        require_ontology_names: Bound<'_, PyBool>,
    ) -> PyResult<Self> {
        Ok(Config {
            cfg: ontoenvrs::config::Config::new(
                root.to_string().into(),
                search_directories
                    .iter()
                    .map(|s| s.to_string().into())
                    .collect(),
                includes
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>(),
                excludes
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<String>>(),
                require_ontology_names.is_true(),
            )
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?,
        })
    }
}

#[pyclass]
struct OntoEnv {
    inner: ontoenvrs::OntoEnv,
}

#[pymethods]
impl OntoEnv {
    #[new]
    fn new(py: Python, config: &Bound<'_, Config>) -> PyResult<Self> {
        env_logger::init();
        let config_path = config.borrow().cfg.root.join(".ontoenv").join("ontoenv.json");
        // if config.root/.ontoenv/ontoenv.json exists, load ontoenv from there
        // else create a new OntoEnv
        let mut env: OntoEnv = if let Ok(env) = ontoenvrs::OntoEnv::from_file(&config_path) {
            info!("Loaded OntoEnv from file");
            OntoEnv { 
                inner: env,
            }
        } else {
            info!("Creating new OntoEnv");
            OntoEnv {
                inner: ontoenvrs::OntoEnv::new(config.borrow().cfg.clone()).map_err(anyhow_to_pyerr)?,
            }
        };

        env.inner.update().map_err(anyhow_to_pyerr)?;
        env.inner.save_to_directory().map_err(anyhow_to_pyerr)?;
        Ok(env)
    }

    fn update(&mut self) -> PyResult<()> {
        self.inner.update().map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    // define __repr__ for OntoEnv. It prints out the # of graphs in the OntoEnv
    // and the total # of triples across all graphs
    fn __repr__(&self) -> PyResult<String> {
        Ok(format!("<OntoEnv: {} graphs, {} triples>", self.inner.num_graphs(), self.inner.num_triples().map_err(anyhow_to_pyerr)?))
    }


    fn get_closure(&self, py: Python, uri: &str, destination_graph: &Bound<'_, PyAny>) -> PyResult<()> {
        let rdflib = py.import("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let ont = self
            .inner
            .get_ontology_by_name(iri.as_ref())
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("Ontology not found"))?;
        let closure = self
            .inner
            .get_dependency_closure(ont.id())
            .map_err(anyhow_to_pyerr)?;
        let graph = self
            .inner
            .get_union_graph(&closure)
            .map_err(anyhow_to_pyerr)?;
            Python::with_gil(|py| {
                for triple in graph.into_iter() {
                    let s: Term = triple.subject.into();
                    let p: Term = triple.predicate.into();
                    let o: Term = triple.object.into();

                    let t = PyTuple::new_bound(destination_graph.py(), &[
                        term_to_python(destination_graph.py(), rdflib, s)?,
                        term_to_python(destination_graph.py(), rdflib, p)?,
                        term_to_python(destination_graph.py(), rdflib, o)?,
                    ]);

                    destination_graph.getattr("add")?.call1((t,))?;
                }
                Ok::<(), PyErr>(())
            })?;
        Ok(())
    }

    fn dump(&self) -> PyResult<()> {
        self.inner.dump();
        Ok(())
    }

    fn import_dependencies(&mut self, py: Python, graph: &Bound<'_, PyAny>) -> PyResult<()> {
        let rdflib = py.import("rdflib")?;
        // get the subject of rdf:type owl:Ontology from the provided rdflib graph
        let py_rdf_type = term_to_python(py, rdflib, Term::NamedNode(TYPE.into()))?;
        let py_ontology = term_to_python(py, rdflib, Term::NamedNode(ONTOLOGY.into()))?;
        let value_fun: Py<PyAny> = graph.getattr("value")?.into();
        let ontology = value_fun.call1(py, (py_rdf_type, py_ontology))?;

        // if ontology is null, return
        // else, get the ontology IRI and add it to the OntoEnv
        if ontology.is_none(py) {
            return Ok(());
        }

        // turn ontology into a string
        let ontology = ontology.to_string();

        self.get_closure(py, &ontology, graph)
    }
}

/// A Python module implemented in Rust.
#[pymodule]
fn ontoenv(_py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<Config>()?;
    m.add_class::<OntoEnv>()?;
    Ok(())
}
