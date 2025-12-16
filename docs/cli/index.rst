CLI Usage
=========

The ``ontoenv`` CLI wraps the Rust core with commands for discovering and materializing ontology imports. Every command operates on the nearest ``.ontoenv`` directory (walked upward from the current working directory) unless you override ``ONTOENV_DIR``.

Install
-------

.. code-block:: bash

   cargo install ontoenv-cli
   # or from the workspace:
   cargo build -p ontoenv-cli --release
   ./target/release/ontoenv --help

Lifecycle overview
------------------

* ``ontoenv init`` - create or reset the environment metadata under ``.ontoenv``.
* ``ontoenv add <path-or-iri>`` - register an ontology; fetch its ``owl:imports`` unless ``--no-imports`` is passed.
* ``ontoenv update`` - refresh cached ontologies (use ``--all`` to force a refresh).
* ``ontoenv dump`` / ``ontoenv list`` - inspect what is stored.

Global options of note
----------------------

When you invoke any command you can also pass:

* ``-i/--includes`` and ``-e/--excludes``: gitignore-style globs for files (``**`` and bare directory prefixes like ``lib/tests`` are supported). By default the CLI looks for ``*.ttl``, ``*.xml``, and ``*.n3``.
* ``--include-ontology`` / ``--exclude-ontology``: regex filters applied to ontology IRIs *after* discovery. Includes act as a whitelist, excludes run last.
* ``--remote-cache-ttl-secs``: the maximum age (in seconds) for cached remote ontologies before they are re-fetched. The default is 86,400 (24 hours).
* ``--require-ontology-names``, ``--strict``, ``--offline``, ``-p/--policy``, and ``-t/--temporary`` mirror the corresponding builder flags in the Rust/Python APIs.

``ontoenv init`` semantics
--------------------------

``ontoenv init`` separates environment creation from discovery. Pass one or more ``LOCATION`` arguments when you want the command to immediately scan directories; omit them to create an empty container that you can populate later via ``ontoenv add`` or a subsequent ``ontoenv init`` with locations.

.. code-block:: console

   # Discover ontologies under the current directory
   ontoenv init .

   # Bootstrap an empty environment (no discovery yet)
   ontoenv init

   # Seed from multiple directories
   ontoenv init ./ontologies ./models

``--overwrite`` rebuilds ``.ontoenv`` in place. Combine it with ``--offline`` to stay strictly local.

Filtering ontologies by IRI
---------------------------

Use regex filters when directory-based filtering is not enough:

.. code-block:: console

   # Keep only IRIs under example.com, drop anything mentioning experimental
   ontoenv init . \
     --include-ontology '^https://example\\.com/' \
     --exclude-ontology 'experimental'

The same switches are accepted on every command so that subsequent ``update`` or ``add`` runs keep applying the filter set.

.. note::

   The ``ontoenv config`` helper does not yet support modifying the regex lists. Set them during ``init`` (they are persisted inside ``.ontoenv/config.json``) or edit that file directly.

Tuning cache strategy
---------------------

Remote ontologies are stored on disk. The CLI keeps them for 24 hours by default; raise or lower the threshold per command:

.. code-block:: console

   ontoenv update --remote-cache-ttl-secs 172800

If you prefer to reuse whatever is already in ``.ontoenv`` without implicit fetches, enable cached-only mode inside the config before running commands that would ordinarily touch the network:

.. code-block:: console

   ontoenv config set remote_cache_ttl_secs 604800
   # For list-like fields (locations/includes/excludes) use add/remove:
   ontoenv config add locations ./ontologies

``ontoenv config list`` prints the persisted values (including include/exclude patterns and regex filters) so you can confirm what will be used by the next command.

Reference help
--------------

.. code-block:: text

   $ ontoenv --help
   Ontology environment manager

   Usage: ontoenv [OPTIONS] <COMMAND>

   Commands:
     init        Create a new ontology environment
     version     Prints the version of the ontoenv binary
     status      Prints the status of the ontology environment
     update      Update the ontology environment
     closure     Compute the owl:imports closure of an ontology and write it to a file
     get         Retrieve a single graph from the environment and write it to STDOUT or a file
     add         Add an ontology to the environment
     list        List various properties of the environment
     dump        Print out the current state of the ontology environment
     dep-graph   Generate a PDF of the dependency graph
     why         Lists which ontologies import the given ontology
     doctor      Run the doctor to check the environment for issues
     reset       Reset the ontology environment by removing the .ontoenv directory
     config      Manage ontoenv configuration
     help        Print this message or the help of the given subcommand(s)

   Options:
     -v, --verbose
     --debug
     -p, --policy <POLICY>
     -t, --temporary
     --require-ontology-names
     --strict
     -o, --offline
     -i, --includes <INCLUDES>...
     -e, --excludes <EXCLUDES>...
     --include-ontology <INCLUDE_ONTOLOGIES>...
     --exclude-ontology <EXCLUDE_ONTOLOGIES>...
     --remote-cache-ttl-secs <REMOTE_CACHE_TTL_SECS>
     -h, --help
