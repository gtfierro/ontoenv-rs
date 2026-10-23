//! Python handle for a committed version. It never owns a writer or a live env.
use super::*;

#[pyclass(name = "EnvironmentSnapshot", frozen)]
pub(crate) struct PyEnvironmentSnapshot {
    pub(crate) inner: Arc<ontoenv::snapshots::EnvironmentSnapshot>,
}

#[pymethods]
impl PyEnvironmentSnapshot {
    #[getter]
    fn info(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let info = self.inner.info();
        let result = PyDict::new(py);
        result.set_item("environment_id", &info.environment_id)?;
        result.set_item("generation", info.generation)?;
        result.set_item("parent_generation", info.parent_generation)?;
        result.set_item("committed_at", info.committed_at.to_rfc3339())?;
        result.set_item("opened_at", info.opened_at.to_rfc3339())?;
        Ok(result.unbind())
    }
    #[getter]
    fn config(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let json =
            serde_json::to_string(self.inner.config()).map_err(|e| anyhow_to_pyerr(e.into()))?;
        Ok(py.import("json")?.call_method1("loads", (json,))?.unbind())
    }
    fn get_ontology_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .inner
            .ontologies()
            .keys()
            .map(GraphIdentifier::to_uri_string)
            .collect();
        names.sort();
        names
    }
    fn get_ontologies(&self) -> Vec<PyOntology> {
        let mut values: Vec<_> = self.inner.ontologies().values().cloned().collect();
        values.sort_by_key(|ontology| ontology.name().as_str().to_owned());
        values
            .into_iter()
            .map(|inner| PyOntology { inner })
            .collect()
    }
    fn get_bound_namespaces(&self) -> BTreeMap<String, String> {
        self.inner.config().namespaces.clone()
    }
    fn get_namespaces(&self) -> HashMap<String, String> {
        let mut result = HashMap::new();
        let mut values: Vec<_> = self.inner.ontologies().values().collect();
        values.sort_by_key(|ontology| ontology.name().as_str().to_owned());
        for ontology in values {
            result.extend(ontology.source_prefixes().clone());
            result.extend(ontology.namespace_map().clone());
        }
        let bound: HashSet<_> = self.inner.config().namespaces.values().collect();
        result.retain(|_, ns| !bound.contains(ns));
        result.extend(self.inner.config().namespaces.clone());
        result
    }
    fn resolve_alias(&self, name: &str) -> PyResult<Option<String>> {
        let name = NamedNode::new(name).map_err(|e| anyhow_to_pyerr(e.into()))?;
        Ok(self
            .inner
            .resolve(name.as_ref())
            .map(|id| id.to_uri_string()))
    }
    fn _backend(&self) -> PyRdfLibStoreBackend {
        PyRdfLibStoreBackend {
            backend: Arc::new(Mutex::new(RdfLibStoreBackend::EnvSnapshotRdf5d {
                store_path: self.inner.config().root.join(".ontoenv-snapshots"),
                snapshot: self.inner.rdf(),
                patch: None,
            })),
            term_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub(crate) fn get_dataset(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let owner = Py::new(
            py,
            Self {
                inner: Arc::clone(&self.inner),
            },
        )?;
        Ok(py
            .import("ontoenv.rdflib_store")?
            .getattr("dataset_from_snapshot")?
            .call1((owner,))?
            .unbind())
    }
    pub(crate) fn get_graph(&self, py: Python<'_>, uri: &str) -> PyResult<Py<PyAny>> {
        let name = NamedNode::new(uri).map_err(|e| anyhow_to_pyerr(e.into()))?;
        let id = self.inner.resolve(name.as_ref()).ok_or_else(|| {
            pyo3::exceptions::PyValueError::new_err(format!("No graph with URI: {uri}"))
        })?;
        let dataset = self.get_dataset(py)?;
        let identifier = py
            .import("rdflib")?
            .getattr("URIRef")?
            .call1((id.name().as_str(),))?;
        Ok(dataset
            .bind(py)
            .call_method1("graph", (identifier,))?
            .unbind())
    }
}
