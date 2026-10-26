Use OntoEnv in a long-running service
=====================================

Connect once at startup, keep the ``env`` object in application state, and close
it at shutdown. Context managers are optional. The class method requires a path:
``OntoEnv.connect(".")`` uses the current directory.

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect("/srv/ontology-env")
   try:
       iri = env.add("/srv/ontologies/site.ttl")  # Explicit source loading.
       graph = env.get_graph(iri)
       closure, members = env.get_closure(iri)
       # Serve requests, run jobs, or query these views.
   finally:
       env.close()

``connect`` creates an empty environment if it is missing. Load sources with
``add`` or ``update`` before querying a new environment. If deployment must
already have prepared it, use ``OntoEnv.open(path)`` instead.

Live ``env.get_graph``, ``get_closure``, ``get_union`` and ``get_dataset`` calls
remain supported. Capture ``env.snapshot()`` when several related reads must
use the same version. Managed writes publish before returning; shutdown releases
resources and the writer lock rather than committing pending changes.

Framework lifespan
------------------

For example, FastAPI can own a prepared environment through its lifespan hook.
The handler assumes the requested ontology was loaded by setup or the updater:

.. code-block:: python

   from contextlib import asynccontextmanager

   from fastapi import FastAPI
   from ontoenv import OntoEnv


   @asynccontextmanager
   async def lifespan(app: FastAPI):
       env = OntoEnv.connect("/srv/ontology-env")
       app.state.ontoenv = env
       try:
           yield
       finally:
           env.close()


   app = FastAPI(lifespan=lifespan)

   @app.get("/closure/{iri:path}")
   def closure(iri: str):
       with app.state.ontoenv.snapshot() as snapshot:
           view, members = snapshot.view_closure(iri)
           return {
               "generation": snapshot.info["generation"],
               "graphs": members,
               "triples": len(view),
           }

The captured generation and closure agree even if an update happens between
reads. A request needing just a closure can call ``env.get_closure(iri)``
directly. Do not reopen the environment for each request. Use this writable
lifespan in one process; read-only worker lifespans instead open
``OntoEnv.open("/srv/ontology-env", read_only=True)``.

Owned views and editable copies
-------------------------------

.. code-block:: python

   with env.snapshot() as snapshot:
       graph = snapshot.view_graph(iri)
       closure, members = snapshot.view_closure(iri)
       dataset = snapshot.view_dataset(members)
       editable = snapshot.copy_graph(iri)

   # The snapshot handle is closed; escaped results still own their version.
   rows = closure.query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")

Views remain usable after parent close, reader refresh, writer publication or
writer restart. They keep the captured RDF, catalog and namespaces. Editing a
``copy_*`` result changes that independent copy. Dataset views preserve named
graph contexts; closure views present a flattened graph.

The snapshot scope releases ownership only. There is no read transaction to
commit. :doc:`../explanation/views-and-copies` covers all view/copy pairs.

Refresh sources without restarting
----------------------------------

On the writable handle, explicitly refresh ontology sources:

.. code-block:: python

   env.update()                 # changed local sources and expired remotes
   env.update(force=True)       # every known source regardless of age
   env.update("https://example.org/site.ttl")   # one source and its imports

These operations follow ``owl:imports`` and publish a coherent result before
returning. Previously returned views retain their original version; obtain a
new view for the updated content. ``connect`` itself does not scan source files
or fetch URLs.

One updater and multiple reader processes
-----------------------------------------

The following describes managed local storage in 0.7. One writable handle owns
the writer lock until it closes. Independent read-only processes can stay open
while that writer updates or restarts. External stores retain their own
concurrency and recovery guarantees.

.. code-block:: python

   # Updater process: keep the sole writable connection in application state.
   writer = OntoEnv.connect(
       "/srv/ontology-env", search_directories=["/srv/ontologies"]
   )
   writer.update()

   # Reader process: one handle per worker, opened after initial setup.
   # The updater may remain open and continue publishing.
   reader = OntoEnv.open("/srv/ontology-env", read_only=True)
   old = reader.snapshot()

   # After another process publishes a source update:
   reader.refresh()
   current = reader.snapshot()
   # old still reads its original version; current uses the new selection.

Schedule ``reader.refresh()`` at a request boundary, on a timer, or in response
to an application notification. It selects a saved version without fetching
ontology sources. Use the newly selected snapshot for related reads in that
request. Existing snapshots and results keep their versions. Each process
closes its own environment handle at shutdown.

Read-only handles do not publish environment changes. Opening configuration
overrides apply to that session only. Ordinary updates do not require draining
or restarting reader workers. Legacy migration and development-protocol upgrade
have separate offline requirements; see :doc:`migrate-storage`.

Startup and write failures
--------------------------

Use ``OntoEnv.open(path, read_only=True)`` when a worker must fail if deployment
has not prepared the environment. ``open`` never creates or scans sources.
:doc:`../explanation/lifecycle` compares lifecycle choices.

After an abrupt managed writer exit, opening selects a complete published
version without a recovery writer. Parse or import failures do not expose a
partially updated managed environment. If publication reports an uncertain I/O
outcome, close and reopen the writer, inspect its selected version, and decide
whether to retry. Existing views keep their confirmed version.

``CatalogRecoveryError`` and ``OntoEnv.recover`` apply to interrupted legacy or
external-store catalog changes; see :doc:`recover-an-environment`. Windows'
accepted power-loss limit is documented in
:doc:`../explanation/transactional-snapshots`.

Read without materializing copies
---------------------------------

Use views for query handlers and ``copy_*`` when a caller needs editable RDF.
A view avoids constructing an independent Python graph for every request.
Serialization and export also work on views.

.. code-block:: python

   view, _ = env.get_closure(iri)
   rows = view.query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")

   editable, _ = env.copy_closure(iri)

For streaming responses, the environment also exposes
``iter_triples(iri)`` and ``iter_closure_triples(iri)``. Managed iterators retain
their backing version after the environment closes. Closure triple iterators
preserve duplicates across named graphs; graph views deduplicate the projection.

.. seealso::

   :doc:`../explanation/views-and-copies` and
   :doc:`../explanation/performance` for query and copy costs.
