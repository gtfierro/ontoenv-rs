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

    fn selected_ids(&self, uris: Option<Vec<String>>) -> PyResult<Vec<GraphIdentifier>> {
        let snapshot = self.snapshot()?;
        match uris {
            Some(uris) => {
                let mut seen = HashSet::new();
                let mut ids = Vec::new();
                for uri in uris {
                    let name = NamedNode::new(&uri).map_err(|e| anyhow_to_pyerr(e.into()))?;
                    let id = snapshot.resolve(name.as_ref()).ok_or_else(|| {
                        pyo3::exceptions::PyValueError::new_err(format!("No graph with URI: {uri}"))
                    })?;
                    if seen.insert(id.clone()) {
                        ids.push(id);
                    }
                }
                Ok(ids)
            }
            None => {
                let mut ids: Vec<_> = snapshot.ontologies().keys().cloned().collect();
                ids.sort_by(|a, b| a.name().as_str().cmp(b.name().as_str()));
                Ok(ids)
            }
        }
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
        Ok(self.snapshot()?.get_all_namespaces())
    }
    fn get_namespaces_for_graphs(&self, uris: Vec<String>) -> PyResult<HashMap<String, String>> {
        self.snapshot()?
            .get_namespaces_for_graphs(&self.selected_ids(Some(uris))?)
            .map_err(anyhow_to_pyerr)
    }
    fn resolve_alias(&self, name: &str) -> PyResult<Option<String>> {
        let name = NamedNode::new(name).map_err(|e| anyhow_to_pyerr(e.into()))?;
        Ok(self
            .snapshot()?
            .resolve(name.as_ref())
            .map(|id| id.to_uri_string()))
    }
    #[pyo3(signature = (uris = None))]
    fn _backend(&self, uris: Option<Vec<String>>) -> PyResult<PyRdfLibStoreBackend> {
        let inner = self.snapshot()?;
        let names = self
            .selected_ids(uris)?
            .iter()
            .map(GraphIdentifier::to_uri_string)
            .collect();
        Ok(PyRdfLibStoreBackend {
            backend: Arc::new(Mutex::new(RdfLibStoreBackend::EnvSnapshotRdf5d {
                store_path: inner.config().root.join(".ontoenv-snapshots"),
                snapshot: inner.rdf(),
                scope: Some(Arc::new(names)),
                patch: None,
            })),
            term_cache: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    pub(crate) fn get_dataset(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.view_dataset(py, None)
    }
    /// Fixed dataset selection. None selects all graphs; [] selects none.
    #[pyo3(signature = (uris = None))]
    fn view_dataset(&self, py: Python<'_>, uris: Option<Vec<String>>) -> PyResult<Py<PyAny>> {
        let owner = Py::new(py, Self::new(Arc::clone(self.snapshot()?)))?;
        let names: Vec<_> = self
            .selected_ids(uris)?
            .iter()
            .map(GraphIdentifier::to_uri_string)
            .collect();
        Ok(py
            .import("ontoenv.rdflib_store")?
            .getattr("dataset_from_snapshot")?
            .call1((owner, names))?
            .unbind())
    }
    /// Mutable dataset union preserving canonical graph names and empty contexts.
    #[pyo3(signature = (uris = None))]
    fn copy_dataset(&self, py: Python<'_>, uris: Option<Vec<String>>) -> PyResult<Py<PyAny>> {
        let view = self.view_dataset(py, uris)?;
        Ok(py
            .import("ontoenv.rdflib_store")?
            .getattr("_copy_dataset_view")?
            .call1((view,))?
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

    /// Read-only graph view; the returned graph owns its selected backing.
    fn view_graph(&self, py: Python<'_>, uri: &str) -> PyResult<Py<PyAny>> {
        self.get_graph(py, uri)
    }

    fn copy_graph(&self, py: Python<'_>, uri: &str) -> PyResult<Py<PyAny>> {
        let id = self.selected_ids(Some(vec![uri.to_owned()]))?.remove(0);
        Ok(graph_to_rdflib(
            py,
            &self.snapshot()?.get_graph(&id).map_err(anyhow_to_pyerr)?,
        )?
        .unbind())
    }

    /// Resolve closure membership without fetching missing sources.
    #[pyo3(signature = (uri, recursion_depth = -1))]
    fn closure_iris(&self, uri: &str, recursion_depth: i32) -> PyResult<Vec<String>> {
        let id = self.selected_ids(Some(vec![uri.to_owned()]))?.remove(0);
        Ok(self
            .snapshot()?
            .get_closure(&id, recursion_depth)
            .map_err(anyhow_to_pyerr)?
            .iter()
            .map(GraphIdentifier::to_uri_string)
            .collect())
    }

    #[pyo3(signature = (uri, recursion_depth = -1, remove_owl_imports = true, rewrite_sh_prefixes = true))]
    fn view_closure(
        &self,
        py: Python<'_>,
        uri: &str,
        recursion_depth: i32,
        remove_owl_imports: bool,
        rewrite_sh_prefixes: bool,
    ) -> PyResult<(Py<PyAny>, Vec<String>)> {
        let names = self.closure_iris(uri, recursion_depth)?;
        let namespaces = self.get_namespaces_for_graphs(names.clone())?;
        let dataset = self.get_dataset(py)?;
        let config = ClosureViewConfig {
            root_iri: uri.to_owned(),
            closure_iris: names.clone(),
            remove_owl_imports,
            rewrite_sh_prefixes,
        };
        Ok((
            build_view_graph(py, &dataset, &names, Some(&config), namespaces)?,
            names,
        ))
    }

    #[pyo3(signature = (uri, recursion_depth = -1, remove_owl_imports = true, rewrite_sh_prefixes = true))]
    fn copy_closure(
        &self,
        py: Python<'_>,
        uri: &str,
        recursion_depth: i32,
        remove_owl_imports: bool,
        rewrite_sh_prefixes: bool,
    ) -> PyResult<(Py<PyAny>, Vec<String>)> {
        let (view, names) = self.view_closure(
            py,
            uri,
            recursion_depth,
            remove_owl_imports,
            rewrite_sh_prefixes,
        )?;
        let copy = py
            .import("ontoenv.rdflib_store")?
            .getattr("_copy_graph_view")?
            .call1((view,))?
            .unbind();
        Ok((copy, names))
    }

    /// Raw graph projection: duplicate triples across contexts occur once.
    fn view_union(&self, py: Python<'_>, uris: Vec<String>) -> PyResult<(Py<PyAny>, Vec<String>)> {
        let ids = self.selected_ids(Some(uris))?;
        let names: Vec<_> = ids.iter().map(GraphIdentifier::to_uri_string).collect();
        let namespaces = self
            .snapshot()?
            .get_namespaces_for_graphs(&ids)
            .map_err(anyhow_to_pyerr)?;
        let dataset = self.get_dataset(py)?;
        Ok((
            build_view_graph(py, &dataset, &names, None, namespaces)?,
            names,
        ))
    }

    fn copy_union(&self, py: Python<'_>, uris: Vec<String>) -> PyResult<(Py<PyAny>, Vec<String>)> {
        let (view, names) = self.view_union(py, uris)?;
        let copy = py
            .import("ontoenv.rdflib_store")?
            .getattr("_copy_graph_view")?
            .call1((view,))?
            .unbind();
        Ok((copy, names))
    }

    fn get_importers(&self, uri: &str) -> PyResult<Vec<String>> {
        let name = NamedNode::new(uri).map_err(|e| anyhow_to_pyerr(e.into()))?;
        Ok(self
            .snapshot()?
            .get_importers(&name)
            .map_err(anyhow_to_pyerr)?
            .iter()
            .map(GraphIdentifier::to_uri_string)
            .collect())
    }

    fn get_import_paths(&self, uri: &str) -> PyResult<Vec<Vec<String>>> {
        let name = NamedNode::new(uri).map_err(|e| anyhow_to_pyerr(e.into()))?;
        Ok(self
            .snapshot()?
            .get_import_paths(&name)
            .map_err(anyhow_to_pyerr)?
            .into_iter()
            .map(|path| path.iter().map(GraphIdentifier::to_uri_string).collect())
            .collect())
    }

    /// Resolved paths or paths ending at importers of a missing ontology.
    fn explain_import(&self, py: Python<'_>, uri: &str) -> PyResult<Py<PyDict>> {
        let name = NamedNode::new(uri).map_err(|e| anyhow_to_pyerr(e.into()))?;
        let (present, paths) = match self.snapshot()?.explain_import(&name) {
            ontoenv::api::ImportPaths::Present(paths) => (true, paths),
            ontoenv::api::ImportPaths::Missing { importers } => (false, importers),
        };
        let paths: Vec<Vec<String>> = paths
            .into_iter()
            .map(|path| path.iter().map(GraphIdentifier::to_uri_string).collect())
            .collect();
        let result = PyDict::new(py);
        result.set_item("present", present)?;
        result.set_item("paths", paths)?;
        Ok(result.unbind())
    }
}
