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

from collections.abc import Generator, Iterable, Mapping
from pathlib import Path
from typing import Any, Literal

from rdflib import Dataset, Graph, URIRef, plugin
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
    for prefix, namespace in env.get_namespaces().items():
        dataset.bind(prefix, URIRef(namespace), override=True)


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
        target_graph += env.get_graph(ontology_name)
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


class OntoEnvStore(Store):
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
        self._prefix_to_namespace: dict[str, URIRef] = {}
        self._namespace_to_prefix: dict[URIRef, str] = {}
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
        re-populated from ``env.get_namespaces()``.
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

    def bind(self, prefix: str, namespace: URIRef, override: bool = True) -> None:
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
        return self._namespace_to_prefix.get(namespace)

    def namespaces(self) -> Iterable[tuple[str, URIRef]]:
        return self._prefix_to_namespace.items()

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


class ClosureGraphView(Graph):
    """Read-only merged view across a fixed set of named graphs in a Dataset.

    Returned by :py:meth:`ontoenv.OntoEnv.get_closure`. Triple lookups
    are dispatched to each underlying named graph and de-duplicated; the
    underlying store is shared with the dataset, so mutation through this
    view raises ``ValueError`` from the store layer.

    Construct via ``env.get_closure(...)`` rather than directly.
    """

    def __init__(self, dataset: Dataset, identifiers: Iterable[str]) -> None:
        ids = tuple(identifiers)
        if not ids:
            raise ValueError("ClosureGraphView requires at least one identifier")
        super().__init__(store=dataset.store, identifier=URIRef(ids[0]))
        self._dataset = dataset
        self._identifiers = tuple(URIRef(i) for i in ids)

    def triples(self, triple: Any) -> Generator[Any, None, None]:
        seen: set[Any] = set()
        for ident in self._identifiers:
            for t in self._dataset.graph(ident).triples(triple):
                if t in seen:
                    continue
                seen.add(t)
                yield t

    def __contains__(self, triple: Any) -> bool:
        return any(triple in self._dataset.graph(ident) for ident in self._identifiers)

    def __iter__(self) -> Generator[Any, None, None]:
        return self.triples((None, None, None))

    def __len__(self) -> int:
        # Count unique triples across all named graphs. The Dataset store has
        # no cheaper way to answer this without dedup.
        return sum(1 for _ in self.triples((None, None, None)))


try:
    plugin.register("ontoenv", Store, "ontoenv.rdflib_store", "OntoEnvStore")
except Exception:
    pass
