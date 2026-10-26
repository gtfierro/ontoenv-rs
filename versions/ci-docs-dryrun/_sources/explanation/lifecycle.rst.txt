Opening an environment
======================

.. _choosing-an-environment-lifecycle:

Short answer: use ``connect``
-----------------------------

.. code-block:: python

   env = OntoEnv.connect("./ontology-env")

``connect`` creates the environment if it is missing and reopens it quickly if
it exists. That covers ordinary application startup, and most programs never
need anything else.

The rest of this page is about the other four entry points, which exist so a
program can *refuse* the lifecycle states it does not expect.

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

Why failing is a feature
------------------------

``connect`` is forgiving because in most programs "the environment is not
there yet" is not an error — it is the first run.

But sometimes it is an error, and a very informative one. If your deployment
pipeline is supposed to have built the environment, a process that silently
creates an empty one instead will start up fine and then behave as though
every ontology vanished. ``OntoEnv.open(path)`` turns that into an immediate,
obvious failure at the point where the assumption actually broke.

The same reasoning applies in reverse. A setup command or a test fixture that
means to create a *new* environment should not quietly adopt whatever was left
over from a previous run. ``OntoEnv.create(path)`` fails instead, and
``overwrite=True`` says you meant it.

Each named method encodes an assumption. Reach for one when violating that
assumption should stop your program.

Connect does not read your files
--------------------------------

This surprises people, so it is worth stating plainly:

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", search_directories=["./ontologies"])
   env.update()   # <- this is what reads ./ontologies

``connect`` loads the saved catalog. ``update`` scans sources. Keeping them
apart is what makes restarts cheap: a service that restarts a hundred times
does not re-parse every RDF file a hundred times, and the one time you *do*
want a rescan, you asked for it.

The first ``connect`` on a brand-new environment creates the directory, saves
settings, and initializes empty storage. It still does not scan — the
environment is genuinely empty until ``update()`` or ``add()`` runs.

:doc:`staying-in-sync` covers the refresh side in full.

``recreate=True`` is not a reconnect
------------------------------------

The direct constructor accepts ``recreate=True``, and it is destructive:

.. code-block:: python

   env = OntoEnv(path=".demo-env", recreate=True, search_directories=["./brick"])

Each call deletes ``.demo-env/.ontoenv/``, creates fresh storage, and
immediately scans ``./brick``. The saved catalog and all cached graphs are
gone. (The surrounding directory and your source files are untouched.)

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
``.ontoenv`` directory, opens what it finds, and raises ``FileNotFoundError``
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

How long should the object live?
--------------------------------

The ``with`` statement controls exactly one thing: when ``close()`` is called.
It changes nothing about what OntoEnv saves or how it behaves.

For a script, that automatic cleanup is convenient. For a server, it is the
wrong shape entirely — connect once at startup, keep the object in application
state, close it at shutdown. See :doc:`../how-to/use-in-a-service`.

Deprecated: ``create_or_use_cached``
------------------------------------

.. code-block:: python

   env = OntoEnv("./env", create_or_use_cached=True)   # deprecated
   env = OntoEnv.connect("./env")                      # equivalent, supported

``create_or_use_cached=True`` had the same create-or-reopen intent but no
vocabulary for the other lifecycle states, and no way to say anything about
synchronization. It emits ``DeprecationWarning`` throughout 0.6.x and is
planned for removal in 0.7.
