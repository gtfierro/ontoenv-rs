Work offline and control caching
================================

To disable network access on an existing environment without refreshing its
sources, change the saved configuration:

.. code-block:: console

   $ ontoenv config set offline true

This writes ``offline=true`` to the environment configuration. Later commands
will not make HTTP requests until the setting is changed back to ``false``.

Turn off all network access
---------------------------

Offline mode uses only graphs and remote ontologies already stored on disk.
OntoEnv reports an unresolved remote import instead of fetching it.

To create a new environment in offline mode:

.. code-block:: console

   $ ontoenv init ./ontologies --offline

``init`` creates ``.ontoenv/`` and scans ``./ontologies`` for local RDF files.
The ``--offline`` flag prevents it from fetching any remote imports and saves
offline mode in the new environment.

To rescan the configured local sources of an existing environment while
keeping network access disabled:

.. code-block:: console

   $ ontoenv update --offline

``update`` reads new and changed local files. The ``--offline`` flag prevents
HTTP requests during that scan and saves offline mode for later commands.

The equivalent Python operations are:

.. code-block:: python

   # Open or create the environment with offline mode enabled.
   env = OntoEnv.connect("./ontology-env", offline=True)

   # Change the setting on an environment that is already open.
   env.set_offline(True)
   print(env.is_offline())

To allow network access again without refreshing any sources:

.. code-block:: console

   $ ontoenv config set offline false

Run ``ontoenv update`` separately if you then want to scan local sources and
refresh stale remote ontologies.

Set the cache lifetime
----------------------

When network access is allowed, ``update`` fetches a cached remote ontology
again once the cached response is older than ``remote_cache_ttl_secs``. The
default is 86,400 seconds (24 hours).

.. code-block:: console

   # Save a seven-day lifetime, then run the update with that value.
   $ ontoenv update --remote-cache-ttl-secs 604800

   # Change the saved lifetime without running an update.
   $ ontoenv config set remote_cache_ttl_secs 604800

The first command saves the seven-day lifetime and then refreshes sources;
cached remote ontologies younger than seven days are kept. The second command
changes the saved lifetime but does not read any ontology sources.

.. code-block:: python

   # Set the lifetime while opening the environment.
   env = OntoEnv.connect("./ontology-env", remote_cache_ttl_secs=604800)

   # Or change it on an environment that is already open.
   env.set_remote_cache_ttl_secs(604800)
   print(env.remote_cache_ttl_secs())

Passing the option to ``connect`` overrides the saved value and persists the
new value on a writable connection. The setter changes the same saved setting
on an environment that is already open.

Refresh regardless of the cache
-------------------------------

To re-read every known source even if the local modification time or remote
cache age says it is current:

.. code-block:: console

   $ ontoenv update --all

``--all`` bypasses those freshness checks. It does not bypass offline mode: if
the environment is offline, remote sources are not fetched.

.. code-block:: python

   env.update(force=True)

In Python, ``force=True`` performs the same freshness-check bypass as the CLI
``--all`` flag.

To force just one source:

.. code-block:: python

   env.update("https://example.org/site.ttl", force=True)

Passing a location restricts the update to that source and the imports reached
from it, rather than re-reading every known source.

Move an environment to an offline machine
------------------------------------------

Build and check the environment on a machine with network access:

.. code-block:: console

   # Scan the local ontology directory and follow its imports.
   $ ontoenv init ./ontologies

   # Fetch Brick and the ontologies it imports.
   $ ontoenv add https://brickschema.org/schema/1.4.4/Brick.ttl

   # Confirm that every recorded import now resolves locally.
   $ ontoenv list missing

``init`` creates the environment from local files. ``add`` stores Brick and
its reachable imports in that environment. ``list missing`` should produce no
output; any listed IRI still requires a graph that has not been stored.

Copy the project, including its ``.ontoenv/`` directory, to the offline
machine. On that machine, disable network access in the saved configuration:

.. code-block:: console

   $ ontoenv config set offline true

This command changes only the configuration. It does not scan sources or
attempt a network request.

The stored graphs and their catalog live under ``.ontoenv/``. Offline mode
cannot retrieve a graph that was not copied, which is why ``list missing`` is
run on the networked machine before the project is moved.
