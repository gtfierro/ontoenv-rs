from pathlib import Path
from typing import Iterator, Optional, List, Union, Tuple, Dict
from rdflib import Graph, Dataset
from rdflib.query import Result
from rdflib.store import Store

# Exposed module metadata
version: str


class Ontology:
    """Read-only view of ontology metadata."""

    @property
    def id(self) -> str: ...

    @property
    def name(self) -> str: ...

    @property
    def imports(self) -> List[str]: ...

    @property
    def location(self) -> Optional[str]: ...

    @property
    def last_updated(self) -> Optional[str]: ...

    @property
    def version_properties(self) -> Dict[str, str]: ...

    @property
    def namespace_map(self) -> Dict[str, str]: ...

    def __repr__(self) -> str: ...


class OntoEnv:
    """Ontology environment for managing ontologies and graphs."""

    def __init__(
        self,
        path: Optional[Union[str, Path]] = None,
        recreate: bool = False,
        create_or_use_cached: bool = False,
        read_only: bool = False,
        search_directories: Optional[List[str]] = None,
        require_ontology_names: bool = False,
        strict: bool = False,
        offline: bool = False,
        use_cached_ontologies: bool = False,
        resolution_policy: str = "default",
        root: str = ".",
        includes: Optional[List[str]] = None,
        excludes: Optional[List[str]] = None,
        include_ontologies: Optional[List[str]] = None,
        exclude_ontologies: Optional[List[str]] = None,
        temporary: bool = False,
        remote_cache_ttl_secs: Optional[int] = None,
        graph_store: Optional[object] = None,
        init_from_store: bool = False,
    ) -> None:
        """Create or open an ontology environment.

        When ``graph_store`` is supplied the environment delegates all graph
        storage to the provided object (which must implement ``add_graph``,
        ``get_graph``, ``remove_graph``, and ``graph_ids``).

        Set ``init_from_store=True`` together with ``graph_store`` to
        reconstruct the environment's metadata from graphs that are already
        present in the store, instead of starting from an empty state.

        Query indexes (PSO/POS/SPO/OSP posting lists and a transitive-closure
        table for property paths) are built in memory on first use from the
        persistent ``.r5tu`` store; nothing is written to disk and no
        configuration is required.
        """
        ...

    def __repr__(self) -> str: ...

    def update(self, all: bool = False) -> None:
        """Refresh the environment from its search directories.

        Pass ``all=True`` to force re-ingestion of every known ontology
        regardless of whether its source has changed.
        """
        ...

    def refresh_from_store(self) -> None:
        """Re-read all graphs from the attached graph store and rebuild the
        environment's ontology metadata and import dependency graph.

        Call this whenever the graph store has been mutated externally and
        the in-memory view needs to catch up.  Only meaningful when the
        environment was created with ``graph_store``.
        """
        ...

    # ------------------------------------------------------------------ #
    # Adding ontologies                                                    #
    # ------------------------------------------------------------------ #

    def add(
        self,
        location: Union[str, Path, Graph],
        overwrite: bool = False,
        fetch_imports: bool = True,
        force: bool = False,
        rename: Optional[str] = None,
    ) -> str:
        """Add an ontology to the environment and return its IRI.

        *location* may be a file path, a URL string, or an in-memory
        ``rdflib.Graph``.  Set ``fetch_imports=False`` to skip recursive
        import resolution; ``force=True`` forces re-fetching even when a
        cached copy is fresh.

        *rename* optionally overrides the ontology's declared IRI.  When
        supplied, every occurrence of the original IRI in the stored graph
        (subject and object positions, except ``owl:versionIRI`` values) is
        rewritten to *rename* before the graph is stored.  The environment
        registers the ontology under the new IRI; the original IRI is no
        longer directly addressable.

        Example — load a third-party ontology under a local canonical IRI::

            env.add("https://example.org/upstream.ttl",
                    rename="https://my-org.com/local/upstream")
        """
        ...

    def add_no_imports(
        self,
        location: Union[str, Path, Graph],
        overwrite: bool = False,
        force: bool = False,
        rename: Optional[str] = None,
    ) -> str:
        """Add an ontology without following its ``owl:imports`` declarations.

        Accepts the same *rename* parameter as :py:meth:`add`.
        """
        ...

    def rename_graph_iri(self, uri: str, new_iri: str) -> str:
        """Rename the IRI of an ontology already in the environment.

        Reads the stored graph for *uri*, rewrites all occurrences of the
        current IRI to *new_iri* (subject and object positions, excluding
        ``owl:versionIRI`` values), stores the result under the new name,
        removes the old named graph, and rebuilds the import dependency graph.

        Returns the new IRI string.

        Rewrite rules:

        * ``<old> rdf:type owl:Ontology`` → ``<new> rdf:type owl:Ontology``
        * ``<old> owl:imports <X>`` → ``<new> owl:imports <X>``
        * ``<old> sh:prefixes <old>`` → ``<new> sh:prefixes <new>``
          (self-referential link rewritten on both sides)
        * ``<X> sh:prefixes <old>`` → ``<X> sh:prefixes <new>``
          (object-only rewrite when subject differs)
        * ``<old> owl:versionIRI <old>`` → ``<new> owl:versionIRI <old>``
          (subject rewritten; version value **preserved**)
        """
        ...

    # ------------------------------------------------------------------ #
    # Aliases                                                              #
    # ------------------------------------------------------------------ #

    def add_alias(self, alias_iri: str, canonical_iri: str) -> None:
        """Add an alias for a canonical ontology IRI.

        The alias will route to the same graph as the canonical IRI.
        Aliases only point to canonical IRIs (not other aliases) to avoid chains.

        Example:

            env.add_alias("http://example.com/ont-alias", "http://example.com/ont")
            env["http://example.com/ont-alias"]  # Returns same graph as env["http://example.com/ont"]
        """
        ...

    def remove_alias(self, alias_iri: str) -> None:
        """Remove an alias."""
        ...

    def resolve_alias(self, alias_iri: str) -> Optional[str]:
        """Get the canonical IRI for an alias.

        Returns None if the IRI is not an alias.
        """
        ...

    def get_aliases_for(self, canonical_iri: str) -> List[str]:
        """List all aliases that point to a given canonical IRI."""
        ...

    def is_canonical_iri(self, iri: str) -> bool:
        """Check if an IRI is a canonical ontology (not an alias)."""
        ...

    # ------------------------------------------------------------------ #
    # Querying ontologies                                                  #
    # ------------------------------------------------------------------ #

    def get_graph(self, uri: str) -> Graph:
        """Return a read-only store-backed view of the named graph for *uri*.

        Mutating the returned graph raises ``ValueError``; use
        :py:meth:`copy_graph` for a mutable in-memory copy.
        """
        ...

    def copy_graph(self, uri: str, graph: Optional[Graph] = None) -> Graph:
        """Copy the named graph for *uri* into a mutable ``rdflib.Graph``.

        If *graph* is provided, triples are added to it and the same graph is
        returned. Otherwise a new in-memory graph is returned.

        With a custom ``graph_store=``: if the store implements
        ``copy_graph(iri)`` that method is called; otherwise falls back to
        ``get_graph(iri)``.
        """
        ...

    def get_closure(
        self,
        uri: str,
        recursion_depth: int = -1,
    ) -> Tuple[Graph, List[str]]:
        """Return a read-only merged view over the imports closure of *uri*.

        Behaves like a merged ``rdflib.Graph`` but routes triple-pattern
        lookups across the underlying named graphs without materializing
        a copy. Returns ``(view, closure_names)``; ``view`` is a
        :py:class:`ontoenv.ClosureGraphView`. Use :py:meth:`copy_closure` for
        a mutable in-memory merge.
        """
        ...

    def get_closure_view(
        self,
        uri: str,
        recursion_depth: int = -1,
    ) -> Tuple[Graph, List[str]]:
        """Deprecated alias for :py:meth:`get_closure`.

        Emits ``DeprecationWarning``.
        """
        ...

    def iter_triples(self, uri: str) -> Iterator[Tuple[object, object, object]]:
        """Stream ``(s, p, o)`` triples for one named graph as rdflib terms.

        Skips the rdflib ``Graph`` wrapper; use when you only need to scan.
        """
        ...

    def iter_closure_triples(
        self,
        uri: str,
        recursion_depth: int = -1,
    ) -> Iterator[Tuple[object, object, object]]:
        """Stream ``(s, p, o)`` triples across the imports closure of *uri*.

        Triples are not de-duplicated across named graphs; wrap in
        ``set()`` if you need set semantics.
        """
        ...

    def get_ontology(self, uri: str) -> Ontology:
        """Return the metadata object for the ontology identified by *uri*."""
        ...

    def get_ontology_names(self) -> List[str]:
        """Return the IRIs of all ontologies currently in the environment."""
        ...

    def get_importers(self, uri: str) -> List[str]:
        """Return IRIs of ontologies that directly import *uri*."""
        ...

    def list_closure(
        self,
        uri: Union[str, Graph],
        recursion_depth: int = -1,
    ) -> List[str]:
        """Return the IRIs of all ontologies in the transitive import closure.

        *uri* may be:

        * a **string IRI** of an ontology already in the environment — the
          closure is computed entirely from the stored dependency graph.
        * an **``rdflib.Graph``** not yet in the environment — its
          ``owl:imports`` triples are extracted and the closure is resolved
          from the environment without modifying it.  The graph's own
          ontology IRI (if present) appears first in the result.

        *recursion_depth* limits traversal depth; ``-1`` means unlimited.
        """
        ...

    def copy_closure(
        self,
        uri: str,
        graph: Optional[Graph] = None,
        rewrite_sh_prefixes: bool = True,
        remove_owl_imports: bool = True,
        recursion_depth: int = -1,
    ) -> Tuple[Graph, List[str]]:
        """Copy the transitive ``owl:imports`` closure of *uri* into a mutable graph.

        Returns ``(graph, closure_iris)``. If *graph* is provided the triples
        are added to it and the same graph is returned; otherwise a new
        ``rdflib.Graph`` is returned.

        Use :py:meth:`get_closure` for a read-only store-backed view that
        avoids materializing triples.

        With a custom ``graph_store=``: each graph in the closure is fetched
        via the store's ``copy_graph`` if available, otherwise ``get_graph``.
        """
        ...

    def get_union(
        self,
        uris: List[str],
        include_closures: bool = False,
        recursion_depth: int = -1,
    ) -> Tuple[Graph, List[str]]:
        """Return a read-only merged view over an explicitly listed set of graphs.

        Returns ``(view, graph_iris)`` where ``view`` is a
        :py:class:`ontoenv.ClosureGraphView` that dispatches triple-pattern
        lookups across the listed named graphs without materializing a copy.
        Mutation raises ``ValueError``; use :py:meth:`copy_union` for a
        mutable in-memory merge.

        Set ``include_closures=True`` to expand each listed graph's
        transitive ``owl:imports`` closure into the view.
        """
        ...

    def copy_union(
        self,
        uris: List[str],
        root: str,
        graph: Optional[Graph] = None,
        include_closures: bool = False,
        rewrite_sh_prefixes: bool = True,
        remove_owl_imports: bool = True,
        recursion_depth: int = -1,
    ) -> Tuple[Graph, List[str]]:
        """Copy the union of explicitly enumerated ontology graphs into a mutable graph.

        Returns ``(graph, graph_iris)``. If *graph* is provided the triples
        are added to it and the same graph is returned; otherwise a new
        ``rdflib.Graph`` is returned.

        Set ``include_closures=True`` to include each listed graph's
        transitive ``owl:imports`` closure. ``root`` is used for ontology
        declaration cleanup and optional SHACL prefix rewriting; it does not
        need to be one of the listed graphs.

        Use :py:meth:`get_union` for a read-only store-backed view that
        avoids materializing triples.
        """
        ...

    def import_graph(
        self,
        destination_graph: Graph,
        uri: str,
        recursion_depth: int = -1,
    ) -> None:
        """Merge the closure of *uri* into *destination_graph* in place."""
        ...

    def import_dependencies(
        self,
        graph: Graph,
        recursion_depth: int = -1,
        fetch_missing: bool = False,
    ) -> List[str]:
        """Resolve the ``owl:imports`` of *graph* and merge them into it.

        Returns the list of ontology IRIs that were merged.
        Set ``fetch_missing=True`` to fetch imports that are not yet in the
        environment.
        """
        ...

    def get_dependencies(
        self,
        graph: Graph,
        graph_name: Optional[str] = None,
        recursion_depth: int = -1,
        fetch_missing: bool = False,
    ) -> Tuple[Graph, List[str]]:
        """Return the merged dependency graph and the closure IRI list.

        Similar to ``import_dependencies`` but returns a *new* graph
        containing only the dependencies (the input graph is not modified).
        *graph_name* overrides the ontology IRI used for sh:prefixes
        rewriting.
        """
        ...

    def missing_imports(
        self,
        uri: Optional[Union[str, Graph]] = None,
    ) -> List[str]:
        """Return IRIs of ``owl:imports`` that cannot be resolved.

        *uri* may be:

        * ``None`` — scan every ontology in the environment and return the
          union of all unresolvable imports (de-duplicated).
        * a **string IRI** of an ontology already in the environment — walk
          its full transitive closure and return every unresolvable import.
        * an **``rdflib.Graph``** not yet in the environment — extract its
          direct ``owl:imports``; imports absent from the environment are
          reported immediately; imports that are present are checked
          transitively for their own missing dependencies.
        """
        ...

    def get_namespaces(
        self,
        ontology: Optional[str] = None,
        include_closure: bool = False,
    ) -> Dict[str, str]:
        """Return a prefix→namespace mapping.

        If *ontology* is ``None`` all namespaces across the environment are
        merged.  Set ``include_closure=True`` to include namespaces from the
        transitive import closure of *ontology*.
        """
        ...

    def get_dataset(self) -> Dataset:
        """Return a read-only store-backed ``rdflib.Dataset`` view.

        Mutation raises ``ValueError``. The returned dataset is cached and
        refreshed automatically after environment mutations.
        """
        ...

    def copy_dataset(self, dataset: Optional[Dataset] = None) -> Dataset:
        """Copy the environment into a mutable ``rdflib.Dataset``.

        If *dataset* is provided, quads are added to it and the same dataset is
        returned. Otherwise a new in-memory dataset is returned.
        """
        ...

    def snapshot_as_dataset(
        self,
        backend: str = "auto",
        store: Optional[Store] = None,
    ) -> Dataset:
        """Deprecated alias for :meth:`get_dataset` / :meth:`copy_dataset`.

        Emits ``DeprecationWarning``. ``backend="copy"`` without *store*
        returns :meth:`copy_dataset`; otherwise this returns a read-only
        Dataset view using the requested backend.
        """
        ...

    def as_dataset(
        self,
        backend: str = "auto",
        store: Optional[Store] = None,
    ) -> Dataset:
        """Deprecated alias for :meth:`get_dataset` / :meth:`copy_dataset`.

        Emits ``DeprecationWarning``. ``backend="copy"`` without *store*
        returns :meth:`copy_dataset`; otherwise this returns a read-only
        Dataset view using the requested backend.
        """
        ...

    def refresh_dataset(self, dataset: Dataset) -> None:
        """Re-snapshot ``self`` into an existing ``OntoEnvStore``-backed Dataset.

        Raises ``TypeError`` if ``dataset.store`` is not an
        :class:`ontoenv.OntoEnvStore`.
        """
        ...

    def to_rdflib_dataset(self, mode: str = "auto") -> Dataset:
        """Deprecated alias for :meth:`get_dataset` / :meth:`copy_dataset`.

        Emits ``DeprecationWarning``. ``mode="copy"`` returns
        :meth:`copy_dataset`; otherwise this returns a read-only Dataset view
        using the requested mode.
        """
        ...

    def dump(self, includes: Optional[str] = None) -> None:
        """Print a Turtle serialisation of the environment to stdout."""
        ...

    # ------------------------------------------------------------------ #
    # Configuration accessors                                              #
    # ------------------------------------------------------------------ #

    def is_offline(self) -> bool: ...
    def set_offline(self, offline: bool) -> None: ...

    def is_strict(self) -> bool: ...
    def set_strict(self, strict: bool) -> None: ...

    def requires_ontology_names(self) -> bool: ...
    def set_require_ontology_names(self, require: bool) -> None: ...

    def resolution_policy(self) -> str: ...
    def set_resolution_policy(self, policy: str) -> None: ...

    def store_path(self) -> Optional[str]: ...

    def flush(self) -> None: ...
    def close(self) -> None: ...

    # ------------------------------------------------------------------ #
    # Pythonic sugar                                                       #
    # ------------------------------------------------------------------ #

    def __bool__(self) -> bool:
        """Always ``True`` — use ``env is None`` to detect absence."""
        ...

    def __len__(self) -> int:
        """Number of ontologies in the environment."""
        ...

    def __contains__(self, uri: str) -> bool:
        """``uri in env`` — True if *uri* resolves to a known ontology."""
        ...

    def __getitem__(self, uri: str) -> Graph:
        """``env[uri]`` — shorthand for :py:meth:`get_graph`."""
        ...

    def __iter__(self) -> Iterator[str]:
        """Iterate over the URIs of every ontology in the environment."""
        ...

    def __enter__(self) -> "OntoEnv": ...
    def __exit__(
        self,
        exc_type: object,
        exc_value: object,
        traceback: object,
    ) -> None: ...


class OntoEnvStore:
    """rdflib Store implementation backed by OntoEnv's native query engine."""

    def __init__(self, configuration: Optional[str] = None, identifier: Optional[object] = None) -> None: ...
    @classmethod
    def from_env(cls, env: OntoEnv, mode: str = "auto") -> OntoEnvStore: ...
    def open(self, configuration: Optional[str], create: bool = False) -> int: ...
    def close(self, commit_pending_transaction: bool = False) -> None: ...
    def destroy(self, configuration: str) -> None: ...
    def refresh_from_env(self, env: OntoEnv, mode: Optional[str] = None) -> None: ...
    def add(self, triple: Tuple[object, object, object], context: object, quoted: bool = False) -> None: ...
    def addN(self, quads: List[Tuple[object, object, object, object]]) -> None: ...
    def remove(self, triple_pattern: Tuple[Optional[object], Optional[object], Optional[object]], context: Optional[object] = None) -> None: ...
    def triples(self, triple_pattern: Tuple[Optional[object], Optional[object], Optional[object]], context: Optional[object] = None) -> object: ...
    def contexts(self, triple: Optional[Tuple[object, object, object]] = None) -> object: ...
    def query(self, query: object, initNs: Dict[str, object], initBindings: Dict[str, object], queryGraph: str, **kwargs: object) -> Result: ...
