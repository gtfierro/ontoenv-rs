"""rdflib ``Store`` implementation backed by an OntoEnv snapshot.

This module exposes :class:`OntoEnvStore` — a read-only rdflib ``Store`` that
serves SPARQL queries through the Rust backend — and the high-level helpers
:func:`dataset_from_env` and :func:`refresh_dataset_from_env`. End users
typically don't import from here directly; they call ``env.get_dataset()``
on an :class:`ontoenv.OntoEnv`, which delegates to :func:`dataset_from_env`.

Two backend strategies are available:

- ``rdf5d`` — zero-copy view backed by the persistent ``.ontoenv/store.r5tu``
  snapshot file. Fastest open and lowest memory. Requires a persistent local
  env; not available for temporary envs or envs using a custom ``graph_store=``.
- ``copy`` — materialize the env's quads into an in-memory ``OxDataset`` once.
  Works for every env kind. Snapshot is independent of the env after the copy.

The ``auto`` mode picks ``rdf5d`` when a persistent snapshot file exists and
falls back to ``copy`` otherwise.
"""

from __future__ import annotations

import warnings
from collections.abc import Generator, Iterable, Iterator, Mapping
from pathlib import Path
from typing import Any, Literal

from rdflib import Dataset, Graph, URIRef, plugin
from rdflib.paths import Path as RdflibPath
from rdflib.query import Result
from rdflib.store import NO_STORE, VALID_STORE, Store
from rdflib.term import Identifier

from ontoenv._native import _RdfLibStoreBackend

Mode = Literal["auto", "rdf5d", "copy"]


def _context_identifier(context: Any) -> Any:
    if context is None:
        return None
    return getattr(context, "identifier", context)


def _inject_prefixes(query: str, init_ns: Mapping[str, Any] | None) -> str:
    if not init_ns:
        return query
    prefix_lines = [f"PREFIX {prefix}: <{namespace}>" for prefix, namespace in init_ns.items()]
    return "\n".join(prefix_lines + [query])


def _normalize_mode(mode: str) -> Mode:
    if mode not in {"auto", "rdf5d", "copy"}:
        raise ValueError(
            f"Unsupported snapshot backend: {mode!r} (expected 'auto', 'rdf5d', or 'copy')"
        )
    return mode  # type: ignore[return-value]


def _bind_dataset_namespaces(dataset: Dataset, env: Any) -> None:
    user_bindings = env.get_bound_namespaces()
    for prefix, namespace in env.get_namespaces().items():
        # Saved user choices take precedence over destination bindings;
        # ontology declarations preserve the destination's preferred prefixes.
        user_defined = prefix in user_bindings
        dataset.bind(prefix, URIRef(namespace), override=user_defined, replace=user_defined)


def add_triples_to_graph(graph: Graph, triples: Iterable[tuple[Any, Any, Any]]) -> None:
    """Add a batch of triples to ``graph``.

    Rust callers use this to avoid one Python function call per triple while
    materializing large closure copies.
    """
    add = graph.add
    for triple in triples:
        add(triple)


def _snapshot_store_file(env: Any) -> Path | None:
    store_dir = env.store_path()
    if not store_dir:
        return None
    store_file = Path(store_dir) / "store.r5tu"
    return store_file if store_file.is_file() else None


def _require_snapshot_store_file(env: Any) -> Path:
    store_file = _snapshot_store_file(env)
    if store_file is None:
        raise ValueError(
            "backend='rdf5d' requires a persistent local OntoEnv backed by "
            ".ontoenv/store.r5tu; temporary environments and graph_store-backed "
            "environments must use backend='copy'"
        )
    return store_file


def _copy_env_into_store(env: Any, store: "OntoEnvStore") -> None:
    store._backend.bind_env_snapshot(env)


def dataset_from_env(
    env: Any,
    store: Store | None = None,
    mode: Mode = "auto",
) -> Dataset:
    """Return an ``rdflib.Dataset`` backed by an OntoEnv snapshot.

    Prefer ``env.get_dataset()`` or ``env.copy_dataset()`` in user code;
    this function is the underlying implementation.

    Args:
        env: An :class:`ontoenv.OntoEnv` instance.
        store: Optional existing rdflib ``Store`` to bind the Dataset to. If
            ``None``, a fresh :class:`OntoEnvStore` is created. If an
            :class:`OntoEnvStore` is passed, it is refreshed against ``env``
            using ``mode``. If any other ``Store`` is passed, ``mode='rdf5d'``
            is rejected and the env is copied into the store via rdflib.
        mode: ``"auto"``, ``"rdf5d"``, or ``"copy"``. See the module docstring.

    Returns:
        A read-only :class:`rdflib.Dataset` whose named graphs are keyed by
        ontology IRI, with namespaces bound from the env.
    """
    normalized_mode = _normalize_mode(mode)
    if store is None:
        store = OntoEnvStore.from_env(env, mode=normalized_mode)
        return Dataset(store=store)

    if isinstance(store, OntoEnvStore):
        store.refresh_from_env(env, mode=normalized_mode)
        return Dataset(store=store)

    if normalized_mode == "rdf5d":
        raise ValueError("backend='rdf5d' requires an OntoEnvStore instance")

    dataset = Dataset(store=store)
    _bind_dataset_namespaces(dataset, env)
    for ontology_name in env.get_ontology_names():
        target_graph = dataset.graph(URIRef(ontology_name))
        target_graph += env.copy_graph(ontology_name)
    return dataset


def refresh_dataset_from_env(dataset: Dataset, env: Any) -> None:
    """Re-snapshot ``env`` into an existing OntoEnvStore-backed ``dataset``.

    Snapshots are point-in-time; subsequent ``env.add()`` / ``env.flush()``
    calls aren't reflected in the Dataset until you call this. The originally
    chosen backend (``rdf5d`` vs ``copy``) is preserved.

    Raises:
        TypeError: if ``dataset.store`` is not an :class:`OntoEnvStore`.
    """
    if not isinstance(dataset.store, OntoEnvStore):
        raise TypeError("refresh_dataset_from_env() requires a dataset backed by OntoEnvStore")
    dataset.store.refresh_from_env(env)
    _bind_dataset_namespaces(dataset, env)


class _PrefixTable:
    """Prefix <-> namespace bookkeeping shared by the read-only stores."""

    def _init_prefixes(self) -> None:
        self._prefix_to_namespace: dict[str, URIRef] = {}
        self._namespace_to_prefix: dict[URIRef, str] = {}

    def bind(self, prefix: str, namespace: URIRef, override: bool = True) -> None:
        namespace = URIRef(namespace)
        bound_namespace = self._prefix_to_namespace.get(prefix)
        bound_prefix = self._namespace_to_prefix.get(namespace)
        if override:
            if bound_prefix is not None:
                self._prefix_to_namespace.pop(bound_prefix, None)
            if bound_namespace is not None:
                self._namespace_to_prefix.pop(bound_namespace, None)
            self._prefix_to_namespace[prefix] = namespace
            self._namespace_to_prefix[namespace] = prefix
        else:
            self._prefix_to_namespace.setdefault(prefix, namespace)
            self._namespace_to_prefix.setdefault(namespace, prefix)

    def namespace(self, prefix: str) -> URIRef | None:
        return self._prefix_to_namespace.get(prefix)

    def prefix(self, namespace: URIRef) -> str | None:
        return self._namespace_to_prefix.get(URIRef(namespace))

    def namespaces(self) -> Iterator[tuple[str, URIRef]]:
        return iter(list(self._prefix_to_namespace.items()))


class OntoEnvStore(_PrefixTable, Store):
    """A read-only rdflib ``Store`` backed by an OntoEnv snapshot.

    SPARQL queries are executed by the Rust backend rather than rdflib's
    Python query engine. Writes (``add``, ``addN``, ``remove``) raise
    ``ValueError`` — snapshots are immutable; mutate the underlying
    :class:`ontoenv.OntoEnv` and call :func:`refresh_dataset_from_env` instead.

    Construct via :meth:`from_env` or, more commonly, via
    ``env.get_dataset()``. Creating an ``OntoEnvStore()`` directly
    yields an empty store, which is mostly useful as the rdflib plugin
    ``Graph(store='ontoenv')``.
    """

    context_aware = True
    graph_aware = True
    formula_aware = False
    transaction_aware = False

    def __init__(self, configuration: str | None = None, identifier: Identifier | None = None):
        super().__init__(configuration)
        self.identifier = identifier
        self.context_aware = True
        self.graph_aware = True
        self.formula_aware = False
        self.transaction_aware = False
        self._backend = _RdfLibStoreBackend()
        self._init_prefixes()
        self._env_mode: Mode | None = None

    @classmethod
    def from_env(cls, env: Any, mode: Mode = "auto") -> "OntoEnvStore":
        """Build a new ``OntoEnvStore`` and bind it to a snapshot of ``env``."""
        store = cls()
        store.refresh_from_env(env, mode=mode)
        return store

    def open(self, configuration: str | None, create: bool = False) -> int:
        return VALID_STORE

    def close(self, commit_pending_transaction: bool = False) -> None:
        return None

    def destroy(self, configuration: str) -> None:
        self._backend = _RdfLibStoreBackend()
        self._prefix_to_namespace.clear()
        self._namespace_to_prefix.clear()
        self._env_mode = None

    def refresh_from_env(self, env: Any, mode: Mode | None = None) -> None:
        """Rebind this store to a fresh snapshot of ``env``.

        If ``mode`` is omitted, the previously chosen backend is reused (or
        ``"auto"`` on first call). Namespace bindings are cleared and
        re-populated from ``env.get_namespaces()``, which includes bindings
        saved with ``env.bind_namespace()``; unsaved ``dataset.bind()`` calls
        are discarded.
        """
        normalized_mode = _normalize_mode(mode or self._env_mode or "auto")
        if normalized_mode == "rdf5d":
            store_file = _require_snapshot_store_file(env)
            self._backend.bind_rdf5d_snapshot(str(store_file))
            self._env_mode = "rdf5d"
        elif normalized_mode == "copy":
            _copy_env_into_store(env, self)
            self._env_mode = "copy"
        else:
            store_file = _snapshot_store_file(env)
            if store_file is not None:
                self._backend.bind_rdf5d_snapshot(str(store_file))
                self._env_mode = "rdf5d"
            else:
                _copy_env_into_store(env, self)
                self._env_mode = "copy"

        self._prefix_to_namespace.clear()
        self._namespace_to_prefix.clear()
        for prefix, namespace in env.get_namespaces().items():
            self.bind(prefix, URIRef(namespace), override=True)

    def add(
        self,
        triple: tuple[Identifier, Identifier, Identifier],
        context: Any,
        quoted: bool = False,
    ) -> None:
        subject, predicate, obj = triple
        self._backend.add(subject, predicate, obj, _context_identifier(context))

    def addN(
        self,
        quads: Iterable[tuple[Identifier, Identifier, Identifier, Any]],
    ) -> None:
        for subject, predicate, obj, context in quads:
            self.add((subject, predicate, obj), context)

    def remove(
        self,
        triple_pattern: tuple[Identifier | None, Identifier | None, Identifier | None],
        context: Any | None = None,
    ) -> None:
        subject, predicate, obj = triple_pattern
        self._backend.remove(subject, predicate, obj, _context_identifier(context))

    def triples(
        self,
        triple_pattern: tuple[Identifier | None, Identifier | None, Identifier | None],
        context: Any | None = None,
    ) -> Generator[
        tuple[
            tuple[Identifier, Identifier, Identifier],
            Generator[Any | None, None, None],
        ],
        None,
        None,
    ]:
        subject, predicate, obj = triple_pattern
        rows = self._backend.triples(subject, predicate, obj, _context_identifier(context))
        for triple, contexts in rows:
            yield triple, (ctx for ctx in contexts)

    def add_graph(self, graph: Any) -> None:
        return None

    def remove_graph(self, graph: Any) -> None:
        self.remove((None, None, None), graph)

    def __len__(self, context: Any | None = None) -> int:
        return self._backend.len(_context_identifier(context))

    def contexts(
        self,
        triple: tuple[Identifier, Identifier, Identifier] | None = None,
    ) -> Generator[Any | None, None, None]:
        if triple is None:
            values = self._backend.contexts(None, None, None)
        else:
            values = self._backend.contexts(*triple)
        yield from values

    def query(
        self,
        query: Any,
        initNs: Mapping[str, Any],
        initBindings: Mapping[str, Identifier],
        queryGraph: str,
        **kwargs: Any,
    ) -> Result:
        query_text = _inject_prefixes(str(query), initNs)
        return self._backend.query(
            query_text,
            dict(initBindings) if initBindings else None,
            queryGraph,
        )

    def update(
        self,
        update: Any,
        initNs: Mapping[str, Any],
        initBindings: Mapping[str, Identifier],
        queryGraph: str,
        **kwargs: Any,
    ) -> None:
        raise NotImplementedError("SPARQL Update is not supported for OntoEnvStore snapshots")

    def commit(self) -> None:
        return None

    def rollback(self) -> None:
        return None


try:
    plugin.register("ontoenv", Store, "ontoenv.rdflib_store", "OntoEnvStore")
except Exception:
    pass


def _warn_dict_namespaces() -> None:
    warnings.warn(
        "Dict-style access to ViewGraph.namespaces is deprecated; call "
        "ViewGraph.namespaces() to iterate (prefix, URIRef) pairs, or use "
        "ViewGraph.namespace(prefix) to look up a single prefix.",
        DeprecationWarning,
        stacklevel=3,
    )


class _CallableNamespaces(dict):
    """A ``dict`` of prefix bindings that can also be called like
    :meth:`rdflib.Graph.namespaces`, yielding ``(prefix, URIRef)`` pairs.

    Dict-style access is deprecated and emits a ``DeprecationWarning``.
    """

    def __call__(self) -> Iterator[tuple[str, URIRef]]:
        for prefix, namespace in dict.items(self):
            yield prefix, URIRef(namespace)

    def __getitem__(self, key):
        _warn_dict_namespaces()
        return super().__getitem__(key)

    def __iter__(self):
        _warn_dict_namespaces()
        return super().__iter__()

    def __contains__(self, key):
        _warn_dict_namespaces()
        return super().__contains__(key)

    def __len__(self):
        _warn_dict_namespaces()
        return super().__len__()

    def get(self, *args):
        _warn_dict_namespaces()
        return super().get(*args)

    def keys(self):
        _warn_dict_namespaces()
        return super().keys()

    def values(self):
        _warn_dict_namespaces()
        return super().values()

    def items(self):
        _warn_dict_namespaces()
        return super().items()


class _ViewStore(_PrefixTable, Store):
    """Read-only rdflib ``Store`` over a scope of named graphs in a backend.

    Backs :class:`ViewGraph`. Triple lookups, ``len`` and SPARQL are delegated
    to the Rust backend, scoped to ``scope`` (or every graph if ``None``). If
    the backend carries a closure patch the scope presents as one flattened,
    de-duplicated graph.
    """

    # Context-aware so that consumers that wrap a graph's store in a Dataset
    # (e.g. pySHACL) accept it; every context is ignored and reads always see
    # the whole scope.
    context_aware = True
    graph_aware = True
    formula_aware = False
    transaction_aware = False

    def __init__(self, backend: Any, scope: tuple[str, ...] | None = None):
        super().__init__()
        self._backend = backend
        self._scope = scope
        self._init_prefixes()

    def open(self, configuration: str | None, create: bool = False) -> int:
        return VALID_STORE

    def close(self, commit_pending_transaction: bool = False) -> None:
        return None

    # -- read-only contract --

    def add(self, triple: Any, context: Any, quoted: bool = False) -> None:
        raise ValueError(
            "ViewGraph is a read-only view; use copy_closure/copy_union for a mutable graph"
        )

    def addN(self, quads: Iterable[Any]) -> None:  # noqa: N802
        raise ValueError("ViewGraph is a read-only view")

    def remove(self, triple: Any, context: Any = None) -> None:
        raise ValueError(
            "ViewGraph is a read-only view; use copy_closure/copy_union for a mutable graph"
        )

    def add_graph(self, graph: Any) -> None:
        return None

    def remove_graph(self, graph: Any) -> None:
        raise ValueError("ViewGraph is a read-only view")

    def contexts(self, triple: Any = None) -> Generator[Any, None, None]:
        for name in self._scope or ():
            yield Graph(store=self, identifier=URIRef(name))

    def update(self, *args: Any, **kwargs: Any) -> None:
        raise ValueError("ViewGraph is a read-only view; SPARQL Update is not supported")

    # -- reads --

    def triples(
        self,
        triple_pattern: tuple[Any, Any, Any],
        context: Any = None,
    ) -> Generator[tuple[tuple[Any, Any, Any], Iterator[Any]], None, None]:
        subject, predicate, obj = triple_pattern
        unbound = subject is None and predicate is None and obj is None
        if self._scope:
            scope = list(self._scope)
            if unbound:
                for triple in self._backend.iter_triples_scoped(scope):
                    yield triple, iter(())
            else:
                for triple, _contexts in self._backend.triples_scoped(
                    scope, subject, predicate, obj
                ):
                    yield triple, iter(())
        else:
            for triple, _contexts in self._backend.triples(subject, predicate, obj, None):
                yield triple, iter(())

    def __len__(self, context: Any = None) -> int:
        if self._scope:
            return self._backend.len_in_graphs(list(self._scope))
        return self._backend.len(None)

    def contains(self, triple: tuple[Any, Any, Any]) -> bool:
        s, p, o = triple
        if self._scope and s is not None and p is not None and o is not None:
            return self._backend.contains_in_graphs(s, p, o, list(self._scope))
        return any(True for _ in self.triples((s, p, o)))

    def query(
        self,
        query: Any,
        initNs: Mapping[str, Any] | None,
        initBindings: Mapping[str, Identifier] | None,
        queryGraph: Any = None,
        **kwargs: Any,
    ) -> Result:
        query_text = _inject_prefixes(str(query), initNs)
        bindings = dict(initBindings) if initBindings else None
        if self._scope:
            return self._backend.query_scoped(query_text, list(self._scope), bindings)
        return self._backend.query(query_text, bindings, None)


class ViewGraph(Graph):
    """Read-only :class:`rdflib.Graph` over a set of the snapshot's named graphs.

    A genuine ``rdflib.Graph`` (``isinstance(view, rdflib.Graph)`` is true)
    backed by a read-only store that delegates triple lookups, ``len`` and
    SPARQL to the Rust backend, reading straight from the rdf5d mmap snapshot
    without materializing a copy. Every rdflib convenience (``value``,
    ``subject_objects``, property paths, ``qname``, ``+``/``-`` operators,
    ``namespaces()``, ``serialize``, ``query(initNs=..., initBindings=...)``)
    is inherited, and the view can be passed wherever rdflib/pySHACL expect a
    graph. Mutation (``add``/``remove``/``+=``) raises ``ValueError``;
    ``bind`` only changes the view's own prefix table.

    Two flavours, distinguished by how the backend is configured:

    - :py:meth:`ontoenv.OntoEnv.get_closure` returns a view whose backend
      carries a *closure patch*. It presents a **single flattened,
      de-duplicated graph** with the same triple set as
      :py:meth:`ontoenv.OntoEnv.copy_closure` (resolved ``owl:imports``
      stripped, ontology declarations collapsed onto the root, SHACL
      ``sh:prefixes``/``sh:declare`` consolidated).
    - :py:meth:`ontoenv.OntoEnv.get_union` returns a raw merge: every triple
      of each named graph in scope, with no transform and no cross-graph
      de-duplication.

    Construct via ``env.get_closure(uri)`` or ``env.get_union(uris)`` rather
    than directly.

    Args:
        backend: A :class:`ontoenv._native._RdfLibStoreBackend` instance
            bound to an env snapshot.
        scope: Tuple of graph IRIs to scope against, or ``None`` for
            all graphs in the backend.
        namespaces: Optional dict of ``{prefix: namespace}`` bindings.
    """

    def __init__(
        self,
        backend: Any,
        scope: tuple[str, ...] | None = None,
        namespaces: Mapping[str, str] | None = None,
    ):
        store = _ViewStore(backend, scope)
        super().__init__(store=store, bind_namespaces="none")
        self._backend = backend
        self._scope = scope
        for prefix, namespace in (namespaces or {}).items():
            store.bind(prefix, URIRef(namespace), override=True)

    # -- triple pattern access --

    def triples(  # type: ignore[override]
        self,
        triple: Any = None,
        predicate: Any = None,
        obj: Any = None,
    ) -> Generator[tuple[Any, Any, Any], None, None]:
        """Iterate ``(s, p, o)`` triples matching the pattern.

        Accepts rdflib's ``triples((s, p, o))`` (predicate may be a property
        path) and the legacy three-argument form ``triples(s, p, o)``.
        """
        if isinstance(triple, (tuple, list)) and len(triple) == 3 and predicate is None and obj is None:
            return super().triples(tuple(triple))
        return super().triples((triple, predicate, obj))

    def __contains__(self, triple: Any) -> bool:
        return self.store.contains(triple)

    # -- fast scoped accessors (fall back to rdflib for paths) --

    def subjects(self, predicate: Any = None, object: Any = None, unique: bool = False):  # type: ignore[override]
        if self._scope and not isinstance(predicate, RdflibPath) and not isinstance(object, (list, tuple)):
            return iter(self._backend.subjects_scoped(list(self._scope), predicate, object))
        return super().subjects(predicate, object, unique)

    def predicates(self, subject: Any = None, object: Any = None, unique: bool = False):  # type: ignore[override]
        if self._scope:
            return iter(self._backend.predicates_scoped(list(self._scope), subject, object))
        return super().predicates(subject, object, unique)

    def objects(self, subject: Any = None, predicate: Any = None, unique: bool = False):  # type: ignore[override]
        if self._scope and not isinstance(predicate, RdflibPath) and not isinstance(subject, (list, tuple)):
            return iter(self._backend.objects_scoped(list(self._scope), subject, predicate))
        return super().objects(subject, predicate, unique)

    # -- query --

    def query(  # type: ignore[override]
        self,
        query_object: Any,
        processor: str = "sparql",
        result: str = "sparql",
        initNs: Mapping[str, Any] | None = None,  # noqa: N803
        initBindings: Mapping[str, Identifier] | None = None,  # noqa: N803
        use_store_provided: bool = True,
        **kwargs: Any,
    ) -> Result:
        """Run a SPARQL query scoped to this view's graphs.

        Same signature as :meth:`rdflib.Graph.query`; the view's prefix
        bindings are applied unless overridden by ``initNs``. The legacy
        ``init_bindings=`` keyword is still accepted.
        """
        legacy = kwargs.pop("init_bindings", None)
        if legacy is not None:
            initBindings = legacy
        return super().query(
            query_object,
            processor=processor,
            result=result,
            initNs=initNs,
            initBindings=initBindings,
            use_store_provided=use_store_provided,
            **kwargs,
        )

    # -- namespaces --

    @property
    def namespaces(self) -> "_CallableNamespaces":  # type: ignore[override]
        """Prefix bindings; callable like :meth:`rdflib.Graph.namespaces`
        (yields ``(prefix, URIRef)`` pairs). Dict-style access is deprecated.
        """
        return _CallableNamespaces({p: str(n) for p, n in self.store.namespaces()})

    def namespace(self, prefix: str) -> str | None:
        """Resolve a prefix to a namespace IRI (as a plain ``str``)."""
        ns = self.store.namespace(prefix)
        return None if ns is None else str(ns)

    def prefix(self, namespace: str) -> str | None:
        """Resolve a namespace IRI to a prefix."""
        return self.store.prefix(URIRef(namespace))

    def __repr__(self) -> str:
        scope_info = f"{len(self._scope)} graphs" if self._scope else "all graphs"
        try:
            n = len(self)
        except Exception:
            n = -1
        return f"<ViewGraph: {scope_info}, {n} triples>"

    __str__ = __repr__
