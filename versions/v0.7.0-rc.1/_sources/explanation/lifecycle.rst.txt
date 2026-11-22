Opening an environment
======================

.. _choosing-an-environment-lifecycle:

Use ``connect`` when either state is valid
------------------------------------------

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect("./ontology-env")

``connect`` is a class method with a required path; ``OntoEnv.connect(".")``
uses the current directory. The returned handle can live for the whole program.

``connect`` creates the environment if it is missing and reopens it if it
exists. Use a different entry point when one of those states should be an
error.

The rest of this page is about the other five entry points, which exist so a
program can *refuse* the lifecycle states it does not expect.

The design question is not how many constructors an environment needs. It is
which startup mistakes should stop the program immediately.

.. list-table::
   :header-rows: 1
   :widths: 24 38 38

   * - Entry point
     - Use when
     - If the environment already exists / is missing
   * - ``OntoEnv.connect(path)``
     - Normal startup
     - Reopens it / creates it
   * - ``OntoEnv.create(path)``
     - Setup must create a new one
     - Fails unless ``overwrite=True`` / creates it
   * - ``OntoEnv.open(path)``
     - Deployment already prepared it
     - Opens it / fails
   * - ``OntoEnv.adopt(path, store)``
     - A custom store is already populated
     - Fails unless ``overwrite=True`` / indexes the store
   * - ``OntoEnv.recover(path)``
     - Startup raised ``CatalogRecoveryError``
     - Rebuilds the index from the graph store
   * - ``OntoEnv(temporary=True)``
     - Nothing should be saved
     - Always a fresh in-memory environment

Choose which assumption to enforce
----------------------------------

``connect`` accepts both an existing environment and a missing one.

If a missing environment is an error, accepting both states hides useful
information. If your deployment
pipeline is supposed to have built the environment, a process that silently
creates an empty one instead will start up fine and then behave as though
every ontology vanished. ``OntoEnv.open(path)`` fails during startup instead.

The same reasoning applies in reverse. A setup command or a test fixture that
means to create a *new* environment should not quietly adopt whatever was left
over from a previous run. ``OntoEnv.create(path)`` fails instead, and
``overwrite=True`` says you meant it.

Each named method turns an assumption into a startup check. Use one of the
specialized methods when violating its assumption should stop the program:

- ``open`` requires an environment that was prepared already.
- ``create`` requires a path that is safe to initialize.
- ``adopt`` requires a populated external graph store.
- ``recover`` requires evidence of an interrupted catalog mutation.

Use ``connect`` when both “open the saved environment” and “this is the first
run” are valid outcomes.

Connect does not read your files
--------------------------------

``connect`` and ``update`` perform separate operations:

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", search_directories=["./ontologies"])
   env.update()   # <- this is what reads ./ontologies

``connect`` loads the saved catalog. ``update`` scans sources. A process can
therefore reopen an environment without scanning its RDF files. Source content
changes are not visible until ``update`` runs.

The first ``connect`` on a brand-new environment creates the directory, saves
settings, and initializes empty storage. It still does not scan — the
environment is genuinely empty until ``update()`` or ``add()`` runs.

:doc:`staying-in-sync` covers the refresh side in full.

``recreate=True`` is not a reconnect
------------------------------------

The direct constructor accepts ``recreate=True`` to replace the selected state:

.. code-block:: python

   env = OntoEnv(path=".demo-env", recreate=True, search_directories=["./brick"])

With managed storage, each call stages an empty replacement catalog and any
requested source scan, then publishes that replacement. Old readers retain
their versions. Cleanup in ``.demo-env/.ontoenv-snapshots/`` retains current and
previous publications plus all files pinned by live views. Eligible files are
reclaimed on writer startup, successful publication or ``env.prune()``.
Close the active writer before recreating.
Source files are untouched.

That is occasionally what you want, but it is not "open my environment". The
non-destructive equivalent is:

.. code-block:: python

   with OntoEnv.connect(".demo-env", search_directories=["./brick"]) as env:
       env.update()

And when you do mean the destructive version, the named form says so out loud:

.. code-block:: python

   env = OntoEnv.create(".demo-env", overwrite=True, search_directories=["./brick"])

One more constructor behavior worth knowing: ``OntoEnv(path="./project")``
without ``recreate`` searches ``./project`` *and its parents* for an existing
managed environment directory, opens what it finds, and raises ``FileNotFoundError``
otherwise. Convenient interactively; too implicit for application code, where
``connect`` or ``open`` name the path and the intent.

The constructor remains supported and is used internally. For new code, the
named methods are clearer.

Configuration on reopen
-----------------------

Settings persist with the environment. When you reopen it:

- **Omitting an option keeps the saved value.**
- **Passing a value overrides it** — including ``False``, ``"default"``, and
  ``[]``, which are real overrides rather than "unset".

.. code-block:: python

   OntoEnv.connect("./env")                        # everything as saved
   OntoEnv.connect("./env", strict=True)           # override strict, keep the rest
   OntoEnv.connect("./env", search_directories=[]) # explicitly clear search paths

Booleans are effectively tri-state on reopen: ``None`` preserves, ``True`` and
``False`` both override. This applies to ``strict``, ``offline``,
``require_ontology_names``, ``use_cached_ontologies``, ``remote_cache_ttl_secs``,
``resolution_policy``, search directories, and every filter list.
``OntoEnv.open`` behaves identically.

A writable connection saves overrides. A read-only one applies them to that
session only — a read-only worker cannot change what the writer configured.

Changing configuration never triggers a scan. Runtime modes such as ``offline``
and ``strict`` take effect immediately; changed discovery paths and filters
apply on the next ``update()``.

Managed graph and configuration edits publish before their methods return.
``flush()`` flushes adapters and ``close()`` releases the handle; neither adds a
managed commit boundary. Rust callers can group edits with ``transaction``.
Existing views keep their captured version after close.

How long should the object live?
--------------------------------

Keep the returned object in application state and reuse it:

.. code-block:: python

   env = OntoEnv.connect("./ontology-env")
   try:
       # Startup has already loaded this ontology.
       view, members = env.get_closure("https://example.org/site")
       # Continue serving requests or running jobs with env.
   finally:
       env.close()  # Shutdown

Context managers provide the same cleanup for a shorter scope. Neither form
adds a managed commit boundary: writes publish before returning. Returned views
keep their captured version after ``close``.

A writable managed handle owns the single writer lock until close. Read-only
workers can remain open alongside it:

.. code-block:: python

   reader = OntoEnv.open("./ontology-env", read_only=True)
   try:
       old = reader.snapshot()
       reader.refresh()  # Select another process's latest publication.
       current = reader.snapshot()
       # old and its returned views keep the original version.
   finally:
       reader.close()

Use live ``env.get_*`` graph reads for individual results. For several reads
that must agree, capture ``env.snapshot()`` and use its ``view_*`` / ``copy_*``
methods. Reader ``refresh`` selects saved state; writer ``update`` reads sources.
See :doc:`../how-to/use-in-a-service` for framework and multi-process patterns.

Removed constructor flags
-------------------------

The 0.7 API removes ``create_or_use_cached`` and ``init_from_store``.
Use ``OntoEnv.connect(path)`` to create or reopen an environment, and
``OntoEnv.adopt(path, store)`` to catalog an existing external store.
For temporary stores, call ``refresh_from_store(full=True)`` explicitly.
