//! Python handle for a committed version. It never owns a writer or a live env.
use super::*;

#[pyclass(name = "EnvironmentSnapshot")]
pub(crate) struct PyEnvironmentSnapshot {
    inner: Option<Arc<ontoenv::snapshots::EnvironmentSnapshot>>,
}

impl PyEnvironmentSnapshot {
    pub(crate) fn new(inner: Arc<ontoenv::snapshots::EnvironmentSnapshot>) -> Self {
        Self { inner: Some(inner) }
    }
    pub(crate) fn snapshot(&self) -> PyResult<&Arc<ontoenv::snapshots::EnvironmentSnapshot>> {
        self.inner
            .as_ref()
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("EnvironmentSnapshot is closed"))
    }
}

#[pymethods]
impl PyEnvironmentSnapshot {
    /// Release this handle's ownership. Escaped views retain their own backing.
    fn close(&mut self) {
        self.inner = None;
    }

    #[getter]
    fn closed(&self) -> bool {
        self.inner.is_none()
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyResult<PyRef<'_, Self>> {
        slf.snapshot()?;
        Ok(slf)
    }

    fn __exit__(
        &mut self,
        _exc_type: &Bound<'_, PyAny>,
        _exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) {
        self.close();
    }

    #[getter]
    fn info(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let info = self.snapshot()?.info();
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
        let json = serde_json::to_string(self.snapshot()?.config())
            .map_err(|e| anyhow_to_pyerr(e.into()))?;
        Ok(py.import("json")?.call_method1("loads", (json,))?.unbind())
    }
    fn get_ontology_names(&self) -> PyResult<Vec<String>> {
        let mut names: Vec<_> = self
            .snapshot()?
            .ontologies()
            .keys()
            .map(GraphIdentifier::to_uri_string)
            .collect();
        names.sort();
        Ok(names)
    }
    fn get_ontologies(&self) -> PyResult<Vec<PyOntology>> {
        let mut values: Vec<_> = self.snapshot()?.ontologies().values().cloned().collect();
        values.sort_by_key(|ontology| ontology.name().as_str().to_owned());
        Ok(values
            .into_iter()
            .map(|inner| PyOntology { inner })
            .collect())
    }
    fn get_bound_namespaces(&self) -> PyResult<BTreeMap<String, String>> {
        Ok(self.snapshot()?.config().namespaces.clone())
    }
    fn get_namespaces(&self) -> PyResult<HashMap<String, String>> {
        let inner = self.snapshot()?;
        let mut result = HashMap::new();
        let mut values: Vec<_> = inner.ontologies().values().collect();
        values.sort_by_key(|ontology| ontology.name().as_str().to_owned());
        for ontology in values {
            result.extend(ontology.source_prefixes().clone());
            result.extend(ontology.namespace_map().clone());
        }
        let bound: HashSet<_> = inner.config().namespaces.values().collect();
        result.retain(|_, ns| !bound.contains(ns));
        result.extend(inner.config().namespaces.clone());
        Ok(result)
    }
    fn resolve_alias(&self, name: &str) -> PyResult<Option<String>> {
        let name = NamedNode::new(name).map_err(|e| anyhow_to_pyerr(e.into()))?;
        Ok(self
            .snapshot()?
            .resolve(name.as_ref())
            .map(|id| id.to_uri_string()))
    }
    fn _backend(&self) -> PyResult<PyRdfLibStoreBackend> {
        let inner = self.snapshot()?;
        Ok(PyRdfLibStoreBackend {
            backend: Arc::new(Mutex::new(RdfLibStoreBackend::EnvSnapshotRdf5d {
                store_path: inner.config().root.join(".ontoenv-snapshots"),
                snapshot: inner.rdf(),
                patch: None,
            })),
            term_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    pub(crate) fn get_dataset(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let owner = Py::new(py, Self::new(Arc::clone(self.snapshot()?)))?;
        Ok(py
            .import("ontoenv.rdflib_store")?
            .getattr("dataset_from_snapshot")?
            .call1((owner,))?
            .unbind())
    }
    pub(crate) fn get_graph(&self, py: Python<'_>, uri: &str) -> PyResult<Py<PyAny>> {
        let name = NamedNode::new(uri).map_err(|e| anyhow_to_pyerr(e.into()))?;
        let id = self.snapshot()?.resolve(name.as_ref()).ok_or_else(|| {
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
