CLI reference
=============

.. code-block:: bash

   cargo install --locked ontoenv-cli    # with a Rust toolchain
   pip install ontoenv                   # or as part of the Python package

Every command locates the nearest ``.ontoenv/`` directory by walking up from
the current working directory. Set ``ONTOENV_DIR`` to override that.

Global flags
------------

Accepted by every subcommand.

.. list-table::
   :header-rows: 1
   :widths: 34 66

   * - Flag
     - Meaning
   * - ``-i``, ``--includes <GLOB>...``
     - gitignore-style globs on file paths. Supports ``**`` and ``?``; a bare
       directory expands to ``dir/**``. Default:
       ``['*.ttl', '*.xml', '*.n3']``.
   * - ``-e``, ``--excludes <GLOB>...``
     - Globs on file paths to exclude.
   * - ``--include-ontology <REGEX>...``
     - Regex whitelist on ontology IRIs, applied after parsing.
   * - ``--exclude-ontology <REGEX>...``
     - Regex exclusions on ontology IRIs, applied after includes.
   * - ``-o``, ``--offline[=true|false]``
     - Skip all network access.
   * - ``--strict[=true|false]``
     - Treat missing imports as errors instead of warnings.
   * - ``--require-ontology-names[=true|false]``
     - Reject files that lack an ``owl:Ontology`` declaration.
   * - ``--remote-cache-ttl-secs <SECS>``
     - Max age of a cached remote ontology before re-fetch. Default 86400.
   * - ``-t``, ``--temporary``
     - Keep everything in memory; write no ``.ontoenv/``.
   * - ``-p``, ``--policy <POLICY>``
     - Resolution policy when several files declare the same ontology IRI:
       ``default``, ``latest``, or ``version``.
   * - ``-v``, ``--verbose``
     - Log at info level.
   * - ``--debug``
     - Log at debug level.

Omitted mode flags preserve their saved values when the environment already
exists. Boolean flags take explicit values — ``--offline=false``,
``--strict=false``, ``--require-ontology-names=false`` — so a saved ``true``
can be turned off. Explicit changes are persisted without rescanning.

Creating and updating
---------------------

.. rubric:: ``ontoenv init [LOCATION]...``

Create the environment under ``.ontoenv/``. Directory arguments are scanned
immediately; with none, the current directory is used.

``--overwrite``
   Rebuild in place if an environment already exists.

.. code-block:: console

   $ ontoenv init ./ontologies ./vendor
   $ ontoenv init ./ontologies --overwrite
   $ ontoenv init . --includes '*.ttl' --exclude-ontology 'experimental'

.. rubric:: ``ontoenv add <LOCATION>``

Register one ontology from a file path or URL, following ``owl:imports`` by
default.

``--no-imports``
   Do not follow ``owl:imports``.
``--rename <IRI>``
   Store the graph under *IRI* instead of the one it declares. See
   :doc:`../how-to/rename-and-alias`.

.. code-block:: console

   $ ontoenv add ./ontologies/site.ttl
   $ ontoenv add https://brickschema.org/schema/Brick --no-imports
   $ ontoenv add ./vendor/upstream.ttl --rename https://my-org.com/local/upstream

.. rubric:: ``ontoenv update``

Re-ingest modified local files and re-fetch stale remote ontologies, following
imports throughout.

``-a``, ``--all``
   Refresh everything regardless of modification times or cache age.
``-q``, ``--quiet``
   Suppress per-ontology output.
``--json``
   Machine-readable output.

.. code-block:: console

   $ ontoenv update
   $ ontoenv update --all
   $ ontoenv update --remote-cache-ttl-secs 604800

.. rubric:: ``ontoenv recover``

Rebuild the catalog from the persistent graph store after an interrupted
mutation. Removes ``.ontoenv/catalog.pending`` only once the replacement
catalog is published. Unavailable with ``--temporary``. See
:doc:`../how-to/recover-an-environment`.

.. rubric:: ``ontoenv reset``

Delete ``.ontoenv/`` entirely, including cached remote ontologies.

``-f``, ``--force``
   Skip the confirmation prompt.

Exporting graphs
----------------

Three commands write graph data. They differ in scope and in what they do to
the result.

.. list-table::
   :header-rows: 1
   :widths: 16 30 30 24

   * - Command
     - Returns
     - Imports
     - Output
   * - ``get``
     - one stored graph
     - not followed
     - ``STDOUT`` or ``--output``
   * - ``closure``
     - ontology + transitive imports, flattened
     - fully resolved and merged
     - ``[DESTINATION]``, default ``output.ttl``
   * - ``union``
     - an explicit list of graphs, raw
     - only with ``--include-closures``
     - ``--output``, default ``output.ttl``

.. rubric:: ``ontoenv get <ONTOLOGY>``

``-l``, ``--location <PATH|URL>``
   Disambiguate when several sources provide the same IRI.
``--output <FILE>``
   Write to a file instead of ``STDOUT``.
``-f``, ``--format <FORMAT>``
   ``turtle`` (default), ``ntriples``, ``rdfxml``, or ``jsonld``.

.. code-block:: console

   $ ontoenv get https://brickschema.org/schema/Brick
   $ ontoenv get https://brickschema.org/schema/Brick --output brick.ttl --format turtle

.. rubric:: ``ontoenv closure <ONTOLOGY> [DESTINATION]``

``--keep-owl-imports``
   Keep resolved ``owl:imports`` statements (removed by default).
``--no-rewrite-sh-prefixes``
   Do not consolidate SHACL ``sh:prefixes`` onto the root (rewritten by
   default).
``--recursion-depth <N>``
   ``<0`` unlimited (default), ``0`` no imports, ``>0`` that many levels.

.. code-block:: console

   $ ontoenv closure https://brickschema.org/schema/Brick brick_closure.ttl
   $ ontoenv closure https://brickschema.org/schema/Brick out.ttl --keep-owl-imports

.. rubric:: ``ontoenv union --root <ROOT> <ONTOLOGY>...``

``--root <IRI>``
   Required. The IRI used as the root for ontology-declaration and SHACL
   prefix cleanup.
``--include-closures``
   Also expand each listed graph's transitive imports.
``--keep-owl-imports``, ``--no-rewrite-sh-prefixes``, ``--recursion-depth <N>``
   As for ``closure``.
``--output <FILE>``
   Destination, default ``output.ttl``.

.. code-block:: console

   $ ontoenv union --root https://example.org/C \
       https://example.org/A https://example.org/B --output merged.ttl

   $ ontoenv union --root https://example.org/C --include-closures \
       --recursion-depth 2 https://example.org/A --output merged_with_deps.ttl

Inspecting the environment
--------------------------

.. rubric:: ``ontoenv status``

Summary: where ``.ontoenv/`` lives, how many ontologies are loaded, when it
was last updated, and the on-disk store size. ``--json`` for machine-readable
output.

.. rubric:: ``ontoenv list <SUBCOMMAND>``

``ontologies``
   Declared ontology IRIs.
``locations``
   Source URLs the ontologies came from, as ``file://`` or ``http(s)://``
   URLs. Use ``ontoenv dump`` to see which location belongs to which IRI.
``missing``
   ``owl:imports`` targets nothing in the environment resolves.

``--json`` is accepted.

.. rubric:: ``ontoenv dump [CONTAINS]``

Print every stored ontology and its metadata to ``STDOUT``. An optional
substring filters by name.

.. rubric:: ``ontoenv why [ONTOLOGIES]...``

Print every import path leading to each given IRI, from the most distant
importer down to the target. ``--json`` is accepted.

.. rubric:: ``ontoenv doctor``

Check for duplicate ontology IRIs, files with no ``owl:Ontology``
declaration, and conflicting namespace prefixes. ``--json`` is accepted.

.. rubric:: ``ontoenv namespaces [ONTOLOGY]``

Print prefix-to-IRI mappings taken from ``@prefix``/``PREFIX`` declarations
and SHACL ``sh:declare`` entries. With no argument, merges every ontology in
the environment.

``--closure``
   Include namespaces from the ontology's transitive imports.
``--json``
   Output a JSON object instead of ``prefix: namespace`` lines.

.. rubric:: ``ontoenv dep-graph [ROOTS]... --output <FILE>``

Render the import dependency graph as a PDF. Requires Graphviz. With root
IRIs, limits the render to their subgraph.

``--output <FILE>``
   Destination, default ``dep_graph.pdf``. There is no short form — ``-o`` is
   the global ``--offline`` flag.

.. rubric:: ``ontoenv version``

Print the version of the installed binary.

Configuration
-------------

.. rubric:: ``ontoenv config <SUBCOMMAND>``

``list``
   Show every persisted key and value.
``get <KEY>`` / ``set <KEY> <VALUE>`` / ``unset <KEY>``
   Read, write, or revert one key.
``add <KEY> <VALUE>`` / ``remove <KEY> <VALUE>``
   Modify a list-valued key.

.. code-block:: console

   $ ontoenv config list
   $ ontoenv config set remote_cache_ttl_secs 604800
   $ ontoenv config add locations ./more-ontologies
   $ ontoenv config remove locations ./old-path

``add``/``remove`` handle ``locations``, ``includes``, and ``excludes``. The
ontology-IRI regex lists must be edited in ``.ontoenv/config.json`` directly.

See :doc:`configuration` for every key and its default.
