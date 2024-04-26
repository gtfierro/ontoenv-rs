use ::ontoenv as ontoenvrs;
use ::ontoenv::consts::{ONTOLOGY, TYPE};
use ::ontoenv::ontology::OntologyLocation;
use ::ontoenv::transform;
use anyhow::Error;
use oxigraph::model::{BlankNode, Literal, NamedNode, NamedNodeRef, Term, SubjectRef};
use pyo3::{
    prelude::*,
    types::{PyBool, PyString, PyTuple, IntoPyDict},
};
use std::borrow::Borrow;
use std::path::{Path, PathBuf};
use std::sync::Once;

static INIT: Once = Once::new();

fn anyhow_to_pyerr(e: Error) -> PyErr {
    PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string())
}

#[allow(dead_code)]
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

fn term_to_python<'a>(
    py: Python,
    rdflib: &'a Bound<PyModule>,
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

    let res: Bound<PyAny> = match &node {
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
    #[pyo3(signature = (search_directories, require_ontology_names=false, strict=false, offline=false, resolution_policy="default".to_owned(), root=".".to_owned(), includes=vec![], excludes=vec![]))]
    fn new(
        search_directories: Vec<String>,
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
                Some(
                    search_directories
                        .iter()
                        .map(|s| s.to_string().into())
                        .collect::<Vec<PathBuf>>(),
                ),
                includes
                    .unwrap_or_else(|| vec![])
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
                excludes
                    .unwrap_or_else(|| vec![])
                    .iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>(),
                require_ontology_names,
                strict,
                offline,
                resolution_policy.to_string(),
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
    #[pyo3(signature = (config=None, path=Path::new(".").to_owned()))]
    fn new(
        _py: Python,
        config: Option<&Bound<'_, Config>>,
        path: Option<PathBuf>,
    ) -> PyResult<Self> {
        // wrap env_logger::init() in a Once to ensure it's only called once. This can
        // happen if a user script creates multiple OntoEnv instances
        INIT.call_once(|| {
            env_logger::init();
        });

        let mut env = if let Some(p) = path {
            let config_path = p.join(".ontoenv").join("ontoenv.json");
            if let Ok(e) = ontoenvrs::OntoEnv::from_file(&config_path) {
                println!("Loaded OntoEnv from file");
                OntoEnv { inner: e }
            } else {
                println!("Creating new OntoEnv");
                OntoEnv {
                    inner: ontoenvrs::OntoEnv::new(config.unwrap().borrow().cfg.clone())
                        .map_err(anyhow_to_pyerr)?,
                }
            }
        } else {
            OntoEnv {
                inner: ontoenvrs::OntoEnv::new(config.unwrap().borrow().cfg.clone())
                    .map_err(anyhow_to_pyerr)?,
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
        Ok(format!(
            "<OntoEnv: {} graphs, {} triples>",
            self.inner.num_graphs(),
            self.inner.num_triples().map_err(anyhow_to_pyerr)?
        ))
    }

    fn import_graph(&self, py: Python, destination_graph: &Bound<'_, PyAny>, uri: &str) -> PyResult<()> {
        let rdflib = py.import_bound("rdflib")?;
        let iri = NamedNode::new(uri)
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let ont = self
            .inner
            .get_ontology_by_name(iri.as_ref())
            .ok_or_else(|| PyErr::new::<pyo3::exceptions::PyValueError, _>("Ontology not found"))?;
        let mut graph = ont.graph().map_err(anyhow_to_pyerr)?;

        // get the owl:Ontology IRI from the destination_graph
        // call value with TYPE and ONTOLOGY
        let uriref_constructor = rdflib.getattr("URIRef")?;
        let type_uri = uriref_constructor.call1((TYPE.as_str(),))?;
        let ontology_uri = uriref_constructor.call1((ONTOLOGY.as_str(),))?;
        let kwargs = [("predicate", type_uri), ("object", ontology_uri)].into_py_dict_bound(py);
        let result = destination_graph.call_method("value", (), Some(&kwargs))?;
        if !result.is_none() {
            // extract the IRI from the result, turn into a SubjectRef
            let ontology = NamedNode::new(result.extract::<String>()?).map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
            let base_ontology: SubjectRef = SubjectRef::NamedNode(ontology.as_ref());

            // apply the transforms to make the graph compatible with the destination graph
            transform::rewrite_sh_prefixes_graph(&mut graph, base_ontology);
            transform::remove_ontology_declarations_graph(&mut graph, base_ontology);
        }
        transform::remove_owl_imports_graph(&mut graph);

        Python::with_gil(|_py| {
            for triple in graph.into_iter() {
                let s: Term = triple.subject.into();
                let p: Term = triple.predicate.into();
                let o: Term = triple.object.into();

                let t = PyTuple::new_bound(
                    destination_graph.py(),
                    &[
                        term_to_python(destination_graph.py(), &rdflib, s)?,
                        term_to_python(destination_graph.py(), &rdflib, p)?,
                        term_to_python(destination_graph.py(), &rdflib, o)?,
                    ],
                );

                destination_graph.getattr("add")?.call1((t,))?;
            }
            Ok::<(), PyErr>(())
        })?;
        Ok(())
    }

    #[pyo3(signature = (uri, destination_graph, rewrite_sh_prefixes=false, remove_owl_imports=false))]
    fn get_closure(
        &self,
        py: Python,
        uri: &str,
        destination_graph: &Bound<'_, PyAny>,
        rewrite_sh_prefixes: bool,
        remove_owl_imports: bool,
    ) -> PyResult<()> {
        let rdflib = py.import_bound("rdflib")?;
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

                let t = PyTuple::new_bound(
                    destination_graph.py(),
                    &[
                        term_to_python(destination_graph.py(), &rdflib, s)?,
                        term_to_python(destination_graph.py(), &rdflib, p)?,
                        term_to_python(destination_graph.py(), &rdflib, o)?,
                    ],
                );

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
        let rdflib = py.import_bound("rdflib")?;
        // get the subject of rdf:type owl:Ontology from the provided rdflib graph
        let py_rdf_type = term_to_python(py, &rdflib, Term::NamedNode(TYPE.into()))?;
        let py_ontology = term_to_python(py, &rdflib, Term::NamedNode(ONTOLOGY.into()))?;
        let value_fun: Py<PyAny> = graph.getattr("value")?.into();
        let ontology = value_fun.call1(py, (py_rdf_type, py_ontology))?;

        // if ontology is null, return
        // else, get the ontology IRI and add it to the OntoEnv
        if ontology.is_none(py) {
            return Ok(());
        }

        // turn ontology into a string
        let ontology = ontology.to_string();

        self.get_closure(py, &ontology, graph, true, true)
    }

    fn add(&mut self, _py: Python, location: &Bound<'_, PyAny>) -> PyResult<()> {
        let location =
            OntologyLocation::from_str(&location.to_string()).map_err(anyhow_to_pyerr)?;
        self.inner.add(location).map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    fn refresh(&mut self) -> PyResult<()> {
        self.inner.update().map_err(anyhow_to_pyerr)?;
        Ok(())
    }

    #[pyo3(signature = (uri))]
    fn get_graph(&self, py: Python, uri: &Bound<'_, PyString>) -> PyResult<Py<PyAny>> {
        let rdflib = py.import_bound("rdflib")?;
        let iri = NamedNode::new(uri.to_string())
            .map_err(|e| PyErr::new::<pyo3::exceptions::PyValueError, _>(e.to_string()))?;
        let graph = self
            .inner
            .get_graph_by_name(iri.as_ref())
            .map_err(anyhow_to_pyerr)?;
        let res = rdflib.getattr("Graph")?.call0()?;
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

            res.getattr("add")?.call1((t,))?;
        }
        Ok(res.unbind())
    }

    fn get_ontology_names(&self) -> PyResult<Vec<String>> {
        let names: Vec<String> = self
            .inner
            .ontologies()
            .keys()
            .map(|k| k.name().to_string())
            .collect();
        Ok(names)
    }
}

/// A Python module implemented in Rust.
#[pymodule]
fn ontoenv(_py: Python, m: Bound<PyModule>) -> PyResult<()> {
    m.add_class::<Config>()?;
    m.add_class::<OntoEnv>()?;
    Ok(())
}
