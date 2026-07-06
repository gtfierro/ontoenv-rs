CLI Reference
=============

The **ontoenv** CLI wraps the Rust core with commands for discovering,
fetching, and querying RDF ontology imports. Every command locates the nearest
``.ontoenv/`` directory by walking up from the current working directory —
override with ``ONTOENV_DIR``.

Install
-------

.. code-block:: bash

   cargo install ontoenv-cli

   # or build from this workspace after cloning:
   cargo build -p ontoenv-cli --release
   ./target/release/ontoenv --help

Typical workflow
----------------

#. ``ontoenv init`` — create or reset the environment under ``.ontoenv/``. Pass directories to
   discover immediately, or omit them to start empty.
#. ``ontoenv add`` — register an ontology by file path or IRI. Follows ``owl:imports`` by default;
   pass ``--no-imports`` to skip.
#. ``ontoenv update`` — re-ingest modified local files and re-fetch stale remote ontologies.
   Use ``--all`` to force a full refresh regardless of modification times.
#. ``ontoenv closure`` / ``ontoenv get`` — export a merged graph of an ontology plus all its
   imports, or retrieve a single graph.

Commands: status and inspection
--------------------------------

- ``ontoenv list`` — List things stored in the environment. Subcommands: ``ontologies``
  (declared IRIs), ``locations`` (file paths), ``missing`` (unresolved imports).
- ``ontoenv status`` — Print a summary of the environment: how many ontologies are loaded,
  where ``.ontoenv/`` lives, and active config settings.
- ``ontoenv dump`` — Print the full current state of the environment — all stored ontologies
  and their metadata — to ``STDOUT``. Pass a string to filter by name.
- ``ontoenv why`` — Show all import paths that lead to a given ontology IRI — each path runs
  from the most distant importer down to the target.
- ``ontoenv dep-graph`` — Generate a PDF visualisation of the import dependency graph (requires
  Graphviz). Pass one or more root IRIs to limit the graph to a subgraph.
- ``ontoenv doctor`` — Check the environment for common problems: duplicate ontology IRIs, files
  missing an ``owl:Ontology`` declaration, and conflicting namespace prefixes.
- ``ontoenv namespaces`` — Show prefix-to-IRI namespace mappings extracted from ontology files.
  If no IRI is given, merges namespaces from all ontologies. ``--closure`` includes transitive imports.
- ``ontoenv version`` — Print the version of the installed ``ontoenv`` binary.

.. code-block:: console

   # list all discovered ontologies
   ontoenv list ontologies

   # list ontology file locations on disk
   ontoenv list locations

   # list imports that could not be resolved
   ontoenv list missing

   # show environment summary (or as JSON)
   ontoenv status
   ontoenv status --json

   # dump full environment state, optionally filtered
   ontoenv dump
   ontoenv dump brick

   # find what imports a given ontology
   ontoenv why https://brickschema.org/schema/Brick

   # generate a full dependency graph PDF
   ontoenv dep-graph
   # limit to one root's subgraph
   ontoenv dep-graph https://brickschema.org/schema/Brick -o brick_deps.pdf

   # check for environment problems
   ontoenv doctor

   # show all known namespace prefixes
   ontoenv namespaces
   # show prefixes for one ontology and its imports
   ontoenv namespaces https://brickschema.org/schema/Brick --closure

   # print the binary version
   ontoenv version

Commands: update and manage
----------------------------

- ``ontoenv init`` — Create or overwrite the environment. Pass directory paths to trigger
  immediate discovery, or omit them to start empty. ``--overwrite`` rebuilds in place.
- ``ontoenv add`` — Register a single ontology by file path or URL. Fetches ``owl:imports``
  unless ``--no-imports`` is passed.  Pass ``--rename <IRI>`` to store the graph under a
  different IRI than the one declared in the file (see :ref:`renaming-on-add`).
- ``ontoenv update`` — Re-ingest modified local files and re-fetch stale remote ontologies.
  ``--all`` forces a full refresh regardless of modification times or cache age.
- ``ontoenv config`` — Read or update the persisted configuration. Supports ``get``, ``set``,
  ``unset``, ``add``, ``remove``, and ``list`` subcommands.
- ``ontoenv reset`` — Remove the ``.ontoenv/`` directory entirely, wiping all cached state
  and configuration.

.. code-block:: console

   # create an environment scanning two directories
   ontoenv init ./ontologies ./vendor

   # rebuild the environment in place
   ontoenv init ./ontologies --overwrite

   # add a local file (follows owl:imports by default)
   ontoenv add ./ontologies/myont.ttl

   # add a remote ontology without following its imports
   ontoenv add https://brickschema.org/schema/Brick --no-imports

   # add a file and store it under a different canonical IRI
   ontoenv add ./vendor/upstream.ttl --rename https://my-org.com/local/upstream

   # re-ingest changed local files and re-fetch stale remote ontologies
   ontoenv update

   # force a full refresh of everything regardless of modification times
   ontoenv update --all

   # show all persisted config keys and values
   ontoenv config list

   # read or change a single value
   ontoenv config get locations
   ontoenv config set remote_cache_ttl_secs 3600

   # add or remove a value from a list-type key
   ontoenv config add locations ./more-ontologies
   ontoenv config remove locations ./old-path

   # wipe the environment (prompts for confirmation)
   ontoenv reset
   # skip the confirmation prompt
   ontoenv reset --force

.. _renaming-on-add:

Renaming an ontology on add
----------------------------

``--rename <IRI>`` overrides the ontology IRI that gets stored in the
environment.  The flag is useful when you want to load a third-party ontology
under a local or canonical IRI without modifying the source file.

.. code-block:: console

   # Store an upstream ontology under your own canonical IRI
   ontoenv add ./vendor/upstream.ttl \
     --rename https://my-org.com/local/upstream

   # Same, but do not follow owl:imports
   ontoenv add ./vendor/upstream.ttl \
     --rename https://my-org.com/local/upstream \
     --no-imports

What gets rewritten inside the stored graph:

* ``<original> rdf:type owl:Ontology`` → ``<new> rdf:type owl:Ontology``
* ``<original> owl:imports <X>`` → ``<new> owl:imports <X>``
* ``<original> sh:prefixes <original>`` → ``<new> sh:prefixes <new>``
  (self-referential links are rewritten on both sides)
* ``<X> sh:prefixes <original>`` → ``<X> sh:prefixes <new>``
  (object-only rewrite when the subject is a different node)
* ``<original> owl:versionIRI <original>`` → ``<new> owl:versionIRI <original>``
  (subject rewritten; the **version value is preserved** intentionally)

The original IRI is no longer directly addressable after the rename.  Other
ontologies that import the original IRI by name will not automatically resolve
to the renamed copy; re-add or update them as needed.

Commands: extract graphs
------------------------

Three commands export graph data; they differ in scope and how imports are handled:

.. list-table::
   :header-rows: 1
   :widths: 18 30 30 22

   * - Command
     - What is returned
     - Import handling
     - Output
   * - ``ontoenv get``
     - The single stored graph for one ontology IRI
     - None — raw graph only, imports not followed
     - ``STDOUT`` or ``-o <file>``
   * - ``ontoenv closure``
     - The ontology merged with all transitive ``owl:imports``
     - Full transitive closure resolved and merged
     - ``-o <file>`` (required)
   * - ``ontoenv union``
     - An explicitly listed set of ontology IRIs, optionally with
       transitive imports
     - ``--include-closures`` to also resolve closures; ``--recursion-depth``
       to limit depth
     - ``--output <file>`` (default: ``output.ttl``)

.. code-block:: console

   # print a single ontology graph to STDOUT
   ontoenv get https://brickschema.org/schema/Brick

   # write it to a file in a specific format
   ontoenv get https://brickschema.org/schema/Brick \
     --output brick.ttl --format turtle

   # compute the full transitive closure and write to a file
   ontoenv closure https://brickschema.org/schema/Brick brick_closure.ttl

   # closure but keep owl:imports statements in the output
   ontoenv closure https://brickschema.org/schema/Brick brick_closure.ttl \
     --keep-owl-imports

   # union of two ontologies (without transitive imports)
   ontoenv union \
     --root https://example.org/C \
     https://example.org/A https://example.org/B \
     --output merged.ttl

   # union with transitive closure expansion
   ontoenv union \
     --root https://example.org/C \
     --include-closures \
     https://example.org/A \
     --output merged_with_deps.ttl

   # union with full options: keep owl:imports, skip sh:prefix rewriting, depth limit
   ontoenv union \
     --root https://example.org/C \
     --include-closures \
     --keep-owl-imports \
     --no-rewrite-sh-prefixes \
     --recursion-depth 2 \
     https://example.org/A \
     --output customized_union.ttl

Global flags
------------

These flags are accepted by every subcommand.

**Filtering**

- ``-i/--includes``, ``-e/--excludes`` — gitignore-style globs on file paths. Bare
  directories expand to ``dir/**`` automatically.
- ``--include-ontology``, ``--exclude-ontology`` — regex filters on ontology IRIs after
  parsing. Includes are a whitelist; excludes run last.

**Behaviour**

- ``--remote-cache-ttl-secs`` — max age of cached remote ontologies before re-fetch
  (default 86,400 s).
- ``-o/--offline`` — skip all network access; use only what is already on disk.
- ``--require-ontology-names`` — reject files that lack an ``owl:Ontology`` declaration.
- ``--strict`` — treat missing imports as errors instead of warnings.
- ``-t/--temporary`` — keep everything in memory; do not write ``.ontoenv/`` to disk.
- ``-p/--policy`` — conflict-resolution policy when multiple files declare the same
  ontology IRI.
- ``-v/--verbose``, ``--debug`` — increase log verbosity.

Filtering by IRI
----------------

Use regex filters when path-based globs are not enough:

.. code-block:: console

   ontoenv init . \
     --include-ontology '^https://example\.com/' \
     --exclude-ontology 'experimental'

.. note::

   **Persisted filters:** Regex lists are saved inside ``.ontoenv/config.json`` at init
   time and re-applied on every subsequent command. The ``ontoenv config`` helper currently
   supports ``locations``, ``includes``, and ``excludes`` via ``add``/``remove``; edit the
   JSON file directly to change regex filters after init.

Tuning cache behaviour
----------------------

.. code-block:: console

   # Keep remote copies for a week
   ontoenv update --remote-cache-ttl-secs 604800

   # Persist the TTL and add a new search location
   ontoenv config set remote_cache_ttl_secs 604800
   ontoenv config add locations ./ontologies

   # Confirm what is stored
   ontoenv config list
