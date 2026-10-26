Work offline and control caching
================================

OntoEnv fetches remote ontologies over HTTP and caches them on disk. This page
covers turning the network off entirely and tuning how long cached copies are
trusted.

Turn off all network access
---------------------------

Offline mode makes OntoEnv use only what is already on disk. Unresolvable
remote imports are reported rather than fetched.

.. code-block:: console

   $ ontoenv init ./ontologies --offline
   $ ontoenv update -o

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", offline=True)

   # Or toggle it on an open environment:
   env.set_offline(True)
   print(env.is_offline())

Offline mode is saved with the environment, so later commands stay offline
until you change it back:

.. code-block:: console

   $ ontoenv update --offline=false

Set the cache lifetime
----------------------

A cached remote ontology is re-fetched by ``update`` once it is older than
``remote_cache_ttl_secs``. The default is 86,400 seconds (24 hours).

.. code-block:: console

   # For one command
   $ ontoenv update --remote-cache-ttl-secs 604800

   # Persisted as the environment default
   $ ontoenv config set remote_cache_ttl_secs 604800

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", remote_cache_ttl_secs=604800)

   env.set_remote_cache_ttl_secs(604800)
   print(env.remote_cache_ttl_secs())

Refresh regardless of the cache
-------------------------------

To re-read every known source even if its cached copy looks current:

.. code-block:: console

   $ ontoenv update --all

.. code-block:: python

   env.update(force=True)

To force just one source:

.. code-block:: python

   env.update("https://example.org/site.ttl", force=True)

Prepare an environment for an offline machine
---------------------------------------------

Build the environment somewhere with a network, then copy the whole directory:

.. code-block:: console

   # On a networked machine
   $ ontoenv init ./ontologies
   $ ontoenv add https://brickschema.org/schema/1.4.4/Brick.ttl
   $ ontoenv status

   # Ship .ontoenv/ along with your project, then on the target machine:
   $ ontoenv status --offline

Everything OntoEnv needs — the graphs and the index describing them — lives
under ``.ontoenv/``. On the offline machine, set ``offline`` so an accidental
``update`` cannot try to reach the network.

Verify nothing is missing before you go offline:

.. code-block:: console

   $ ontoenv list missing

An empty result means every ``owl:imports`` in the environment resolves to
something already stored.
