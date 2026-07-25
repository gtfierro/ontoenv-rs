Working with persistent environments
====================================

Most applications can use one entry point for the entire life of an
environment:

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect("./ontology-env")
   site = env.add("./ontologies/site.ttl")

   graph = env.get_graph(site)
   closure, imported = env.get_closure(site)

   env.close()

On the first run, ``connect`` creates the environment directory, saves its
settings, and initializes its graph storage. As ontologies are added, OntoEnv
also saves a small index of their names, imports, aliases, source locations,
namespaces, and hashes.

That saved index is what makes the same call suitable for later runs:

.. code-block:: python

   env = OntoEnv.connect("./ontology-env")
   print(env.get_ontology_names())

OntoEnv loads the index instead of rereading every RDF triple. Import
resolution and closure lookup are therefore ready as soon as ``connect``
returns. You do not need to check whether the directory already exists or
choose between “create” and “open” in ordinary application code.

Choosing how long the object lives
----------------------------------

The ``with`` statement only controls when ``close()`` is called. It does not
change what OntoEnv saves or how the environment behaves.

For a short script, a context manager makes cleanup automatic:

.. code-block:: python

   with OntoEnv.connect("./ontology-env") as env:
       closure, imported = env.get_closure("https://example.com/site")
       print(len(closure))

For a webserver or another long-running process, connect once during startup,
keep the object in application state, and close it during shutdown:

.. code-block:: python

   # Application startup
   application_state.ontoenv = OntoEnv.connect("/srv/ontology-env")

   # Request handlers reuse application_state.ontoenv
   graph = application_state.ontoenv.get_graph("https://example.com/site")

   # Application shutdown
   application_state.ontoenv.close()

Reopening the environment for every request adds unnecessary work and makes
resource ownership harder to reason about. A single long-lived object gives
the process a consistent view of the environment.

A persistent environment allows one writer at a time. In a multi-process
server, route changes through one writer. Read-only workers can each call
``OntoEnv.connect(path, read_only=True)`` after the writable environment has
been created and synchronized.

.. _refreshing-ontology-sources:

Refreshing ontology files and URLs
----------------------------------

When OntoEnv knows where an ontology came from, use ``update`` to refresh its
source:

.. code-block:: python

   env.update()

This checks configured search directories for new, changed, or removed files
and refreshes remote ontologies whose cached copies have expired. It also
walks their ``owl:imports``, so dependencies are kept up to date along with the
ontologies that led to them.

Use ``force=True`` when every known source should be reread regardless of its
timestamp or cache age:

.. code-block:: python

   env.update(force=True)

Pass a source to update just that ontology. Its stored graph is replaced
automatically, and its transitive imports are followed:

.. code-block:: python

   env.update("https://example.com/site.ttl")

Add ``force=True`` when the source should be reread even if its cached copy
appears current:

.. code-block:: python

   env.update("https://example.com/site.ttl", force=True)

These operations refresh from ontology *sources*. They may read files or make
network requests according to the environment's offline and cache settings.
The store-refresh methods below solve a different problem: graph content that
another application wrote directly into a custom store.

Connecting to your own graph store
----------------------------------

The same ``connect`` call works when graph content lives in a store supplied
by your application:

See :doc:`graph-store` for the required methods, optional change-reporting
methods, and a complete minimal store example.

.. code-block:: python

   store = MyGraphStore(...)
   env = OntoEnv.connect("./ontology-env", graph_store=store)

If the store already contains graphs on the first connection, OntoEnv reads
each graph once and records the ontology information it needs for names,
imports, aliases, namespaces, and closure lookup. It does not fetch network
imports while learning about those existing graphs. Later connections reuse
the saved index.

Changes made through ``env`` update the graph store and the saved index
together. No separate synchronization call is needed.

Changes made directly to the store are different because OntoEnv did not see
them happen. By default, ``connect`` asks the store whether anything changed.
If the store can identify the changed graphs, OntoEnv rereads only those
graphs. If the store can report that it changed but cannot identify which
graphs changed, ``connect`` raises an error asking you to make the full scan
explicit:

.. code-block:: python

   env = OntoEnv.connect(
       "./ontology-env",
       graph_store=store,
       sync="full",
   )

This makes startup cost predictable: ``sync="auto"`` never silently turns a
normal restart into a scan of every graph. A store that does not report its
state is still usable; OntoEnv trusts the saved index until you request a
targeted or full refresh. This synchronization concerns graphs changed
directly in the store. It does not inspect ontology files or URLs; use
:ref:`refreshing-ontology-sources` for those sources and their imports.

Refreshing a running environment
--------------------------------

If the process stays alive while another system edits the graph store, refresh
the existing object instead of reconnecting it:

.. code-block:: python

   report = env.refresh_from_store()
   print(report.added, report.changed, report.removed)

With per-graph change information, the no-argument form reads only graphs that
changed. You can also name the exact graphs that should be checked:

.. code-block:: python

   report = env.refresh_from_store(
       graphs=["https://example.com/site"],
   )

``graphs`` is an exact set of backend graph IDs; OntoEnv does not silently
expand it. To refresh a root plus the dependency closure currently recorded in
the environment, compose it with ``list_closure``:

.. code-block:: python

   root = "https://example.com/site"
   known_closure = env.list_closure(root)
   report = env.refresh_from_store(graphs=known_closure)

This is useful when the graphs in a known closure were rewritten together. If
the external edit introduced a completely new imported graph, that graph is
not part of the old closure yet. A no-argument incremental refresh will find
it when the store reports per-graph changes; otherwise use ``full=True`` to
reconstruct the environment from everything in the store.

A targeted refresh deliberately says nothing about other changed graphs. The
report lists those as still pending, and a later normal connection continues
to detect that the store and saved index are not fully synchronized.

Use a full refresh when you deliberately want OntoEnv to forget its saved view
and reconstruct it from every graph currently in the store:

.. code-block:: python

   report = env.refresh_from_store(full=True)

Use ``graphs=[...]`` or ``full=True``, but not both.

Temporary work
--------------

For a notebook, test, or one-off transformation that should leave no
environment files behind, create a temporary environment:

.. code-block:: python

   env = OntoEnv(temporary=True)
   env.add("./ontologies/site.ttl")

Temporary environments keep both graphs and ontology information in memory.
They have no saved index to warm-open later.

If a temporary environment uses a custom store that is already populated,
request the initial scan directly:

.. code-block:: python

   env = OntoEnv(graph_store=store, temporary=True)
   env.refresh_from_store(full=True)

When stricter startup behavior is useful
----------------------------------------

``connect`` is designed to make the normal decision for you. Three narrower
methods are available when you need creating, loading, or inspecting the store
to be an explicit part of your program's contract.

``create`` sets up a new environment directory, saves its settings, and
initializes empty graph storage and an empty ontology index. It fails when an
environment already exists, which is useful in setup commands and tests:

.. code-block:: python

   env = OntoEnv.create("./new-environment")

``open`` requires an existing saved environment. It never creates one, scans
graphs, or reconciles external changes. This is useful when deployment has
already prepared the environment and startup should fail if that assumption
is wrong:

.. code-block:: python

   env = OntoEnv.open("./prepared-environment", read_only=True)

``adopt`` is the explicit version of connecting to a populated custom store
for the first time. It reads every existing graph once and saves the ontology
information needed by OntoEnv. It never follows imports onto the network:

.. code-block:: python

   env = OntoEnv.adopt("./ontology-env", graph_store=store)

Most applications do not need to choose among these methods. Start with
``OntoEnv.connect(path)`` and use a narrower method only when failure on the
other lifecycle states is intentional.

Controlling synchronization at connection time
-----------------------------------------------

The default ``sync="auto"`` is the right choice for normal application
startup. It creates or learns an environment on first use, reuses the saved
index on ordinary restarts, and incorporates external changes when the store
can identify them. It does not fetch ontology sources or check source files
for changes. After connecting, call ``env.update()`` as described in
:ref:`refreshing-ontology-sources` when files, URLs, and their imports should
be refreshed.

``sync="full"`` deliberately rereads every graph. Use it after known
out-of-band changes when the store cannot identify the affected graphs.

``sync="catalog"`` uses the saved ontology information without reading graph
contents. It is intended for controlled deployments where another part of the
system guarantees that the saved view is the one the process should use.
