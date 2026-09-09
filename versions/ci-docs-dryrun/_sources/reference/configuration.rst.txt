Configuration
=============

Settings live in ``.ontoenv/config.json`` and are re-applied by every command
and every reopen. They can be set at init time, changed with ``ontoenv
config``, passed to a Python lifecycle method, or changed at runtime with a
``set_*`` method.

Settings
--------

.. list-table::
   :header-rows: 1
   :widths: 24 12 18 22 24

   * - Key
     - Type
     - Default
     - CLI flag
     - Python argument
   * - ``locations``
     - list[path]
     - ``[]``
     - ``init <DIR>...``
     - ``search_directories=``
   * - ``includes``
     - list[glob]
     - ``['*.ttl','*.xml','*.n3']``
     - ``-i``, ``--includes``
     - ``includes=``
   * - ``excludes``
     - list[glob]
     - ``[]``
     - ``-e``, ``--excludes``
     - ``excludes=``
   * - ``include_ontologies``
     - list[regex]
     - ``[]``
     - ``--include-ontology``
     - ``include_ontologies=``
   * - ``exclude_ontologies``
     - list[regex]
     - ``[]``
     - ``--exclude-ontology``
     - ``exclude_ontologies=``
   * - ``offline``
     - bool
     - ``false``
     - ``-o``, ``--offline``
     - ``offline=``
   * - ``strict``
     - bool
     - ``false``
     - ``--strict``
     - ``strict=``
   * - ``require_ontology_names``
     - bool
     - ``false``
     - ``--require-ontology-names``
     - ``require_ontology_names=``
   * - ``use_cached_ontologies``
     - bool
     - ``false``
     - —
     - ``use_cached_ontologies=``
   * - ``remote_cache_ttl_secs``
     - int
     - ``86400``
     - ``--remote-cache-ttl-secs``
     - ``remote_cache_ttl_secs=``
   * - ``resolution_policy``
     - string
     - ``"default"``
     - ``-p``, ``--policy``
     - ``resolution_policy=``
   * - ``temporary``
     - bool
     - ``false``
     - ``-t``, ``--temporary``
     - ``temporary=``
   * - ``root``
     - path
     - ``"."``
     - —
     - ``root=``

.. rubric:: Notes

``includes`` / ``excludes``
   gitignore-style globs matched against **file paths**, before parsing. A
   bare directory expands to ``dir/**``.

``include_ontologies`` / ``exclude_ontologies``
   Regular expressions matched against **ontology IRIs**, after parsing.
   Includes act as a whitelist; excludes run last.

``resolution_policy``
   Which definition wins when several files declare the same ontology IRI.
   ``"default"`` prefers the first registered, ``"latest"`` the most recently
   updated, ``"version"`` the highest version property.

``use_cached_ontologies``
   When enabled, discovery is skipped at init time; the environment fills only
   from explicit ``add`` and ``update`` calls.

``remote_cache_ttl_secs``
   How long a cached remote ontology is trusted before ``update`` re-fetches
   it.

Setting values from the CLI
---------------------------

.. code-block:: console

   $ ontoenv config list
   $ ontoenv config get locations

Scalar keys use ``set`` and ``unset``:

.. code-block:: console

   $ ontoenv config set offline true
   $ ontoenv config set strict false
   $ ontoenv config set require_ontology_names true
   $ ontoenv config set resolution_policy latest
   $ ontoenv config set remote_cache_ttl_secs 604800
   $ ontoenv config unset remote_cache_ttl_secs

``set`` accepts exactly those five keys. List keys use ``add`` and ``remove``:

.. code-block:: console

   $ ontoenv config add locations ./more-ontologies
   $ ontoenv config remove locations ./old-path
   $ ontoenv config add includes '*.n3'
   $ ontoenv config add excludes 'vendor'

``add``/``remove`` accept ``locations``, ``includes``, and ``excludes``.
``include_ontologies`` and ``exclude_ontologies`` have no ``config``
subcommand support — pass the flags on a command, or edit
``.ontoenv/config.json``.

Setting values from Python
--------------------------

At open time, any key can be passed as a keyword argument:

.. code-block:: python

   env = OntoEnv.connect(
       "./ontology-env",
       search_directories=["./ontologies"],
       includes=["*.ttl"],
       exclude_ontologies=[r"experimental"],
       offline=True,
       remote_cache_ttl_secs=604800,
   )

At runtime, the ``set_*`` methods change and persist a setting on an open,
writable environment:

.. code-block:: python

   env.set_offline(True)
   env.set_strict(False)
   env.set_require_ontology_names(True)
   env.set_use_cached_ontologies(True)
   env.set_remote_cache_ttl_secs(604800)
   env.set_resolution_policy("latest")

Each has a matching getter — ``is_offline()``, ``is_strict()``,
``requires_ontology_names()``, ``uses_cached_ontologies()``,
``remote_cache_ttl_secs()``, ``resolution_policy()``.

Override rules on reopen
------------------------

When reopening an existing environment:

- **Omitting** an option keeps the saved value.
- **Passing** a value overrides it. ``False``, ``"default"``, and ``[]`` are
  genuine overrides, not "unset".
- A **writable** connection persists the override; a **read-only** one applies
  it to that session only.

.. code-block:: python

   OntoEnv.connect("./env")                          # everything as saved
   OntoEnv.connect("./env", strict=True)             # override one setting
   OntoEnv.connect("./env", search_directories=[])   # explicitly clear

On the CLI, boolean flags take explicit values so a saved ``true`` can be
turned off:

.. code-block:: console

   $ ontoenv update --offline=false --strict=false

Changing configuration never triggers a scan or re-ingestion. Runtime modes
apply immediately; changed discovery paths and filters apply on the next
``update()``.

Environment variables
---------------------

``ONTOENV_DIR``
   Path to the environment to use, overriding the walk-up-from-cwd search. It
   may name the ``.ontoenv`` directory itself or the root containing it.

``RUST_LOG``
   Standard Rust log filter. ``-v`` sets it to ``info`` and ``--debug`` to
   ``debug``.
