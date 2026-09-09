Use OntoEnv in a long-running service
=====================================

A web server or daemon should treat the environment as a resource it owns for
its whole lifetime: connect once at startup, share the object, close it at
shutdown.

The pattern
-----------

.. code-block:: python

   from contextlib import asynccontextmanager

   from fastapi import FastAPI
   from ontoenv import OntoEnv


   @asynccontextmanager
   async def lifespan(app: FastAPI):
       # Startup: open the environment and make it available to handlers.
       env = OntoEnv.connect("/srv/ontology-env")
       app.state.ontoenv = env
       try:
           yield
       finally:
           # Shutdown: flush pending writes and release the environment lock.
           env.close()


   app = FastAPI(lifespan=lifespan)

   @app.get("/closure/{iri:path}")
   def closure(iri: str):
       view, imported = app.state.ontoenv.get_closure(iri)
       return {"graphs": imported, "triples": len(view)}

Do not connect per request. Reopening the environment repeats work that
``connect`` is designed to do once, and it makes it much harder to reason
about who owns the underlying storage.

The ``with`` statement is only sugar for calling ``close()``; it changes
nothing about how the environment behaves. Use it in scripts, not here.

Refresh sources without restarting
----------------------------------

``connect`` does not read your ontology files — that is always an explicit
call. To pick up changes while the process runs:

.. code-block:: python

   env.update()                 # rescan search directories, refresh expired remotes
   env.update(force=True)       # reread every known source regardless of age
   env.update("https://example.org/site.ttl")   # just this one source

All three follow ``owl:imports``, so dependencies are refreshed along with the
ontologies that led to them.

Run this on a timer or from an admin endpoint. Reads happening concurrently
continue to see a consistent view.

Multiple worker processes
-------------------------

A persistent environment allows either **one writer** or **multiple read-only
connections** at a time. A read-only process waits while a writer holds the
environment lock. For a multi-process server, prepare the environment in a
writable process, close it, and then start the read-only workers:

.. code-block:: python

   # Provisioning: this process must finish and close before workers connect.
   with OntoEnv.connect("/srv/ontology-env") as env:
       env.update()

   # Worker startup: several processes can hold shared read-only locks.
   env = OntoEnv.open("/srv/ontology-env", read_only=True)

Read-only connections never write to the environment directory. Configuration
passed while opening a read-only connection applies to that session only and
is not persisted.

To update sources later, stop or drain the read-only workers, open one writable
connection, run ``update()``, close it, and then reopen the workers. A
long-lived writer cannot update alongside read-only worker processes because
it holds the exclusive lock.

Fail fast if the environment is not there
-----------------------------------------

``connect`` creates a missing environment, which is usually what you want. If
deployment is supposed to have prepared the environment already and a missing
one indicates a broken deploy, say so explicitly:

.. code-block:: python

   env = OntoEnv.open("/srv/ontology-env", read_only=True)

``open`` raises if the environment does not exist, and never creates, scans,
or reconciles anything.

:doc:`../explanation/lifecycle` compares all six entry points.

Handle recovery at startup
--------------------------

If a previous process was killed between writing a graph and committing its
index, ``connect`` raises ``CatalogRecoveryError``. Decide up front whether
your service repairs itself or refuses to start:

.. code-block:: python

   from ontoenv import OntoEnv, CatalogRecoveryError

   try:
       env = OntoEnv.connect("/srv/ontology-env")
   except CatalogRecoveryError:
       log.warning("recovering ontology environment after interrupted write")
       env = OntoEnv.recover("/srv/ontology-env")

Recovery reads every stored graph and rebuilds the catalog; ``connect``
normally reads the existing catalog. See :doc:`recover-an-environment`.

Keep memory low
---------------

Prefer ``get_*`` over ``copy_*`` in request handlers that do not mutate the
result. A view reads from the on-disk snapshot; a copy materializes the whole
closure in Python memory on every call.

.. code-block:: python

   # Good — read-only view, no materialization
   view, _ = env.get_closure(iri)
   rows = view.query("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")

   # Only when the caller must mutate or export the graph
   g, _ = env.copy_closure(iri)

For streaming responses, skip the graph wrapper entirely:

.. code-block:: python

   for s, p, o in env.iter_closure_triples(iri):
       yield serialize(s, p, o)

.. seealso::

   :doc:`../explanation/views-and-copies` and
   :doc:`../explanation/performance` for the numbers behind this advice.
