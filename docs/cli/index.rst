CLI Reference
=============

.. raw:: html

   <div class="oe-section-intro">
     The <strong>ontoenv</strong> CLI wraps the Rust core with commands for discovering,
     fetching, and querying RDF ontology imports. Every command locates the nearest
     <code>.ontoenv/</code> directory by walking up from the current working directory —
     override with <code>ONTOENV_DIR</code>.
   </div>

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

.. raw:: html

   <div class="oe-cmd-grid">

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv list</span>
       <p>List things stored in the environment.
          Subcommands: <code>ontologies</code> (declared IRIs), <code>locations</code>
          (file paths), <code>missing</code> (unresolved imports).</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv status</span>
       <p>Print a summary of the environment: how many ontologies are loaded, where
          <code>.ontoenv/</code> lives, and active config settings.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv dump</span>
       <p>Print the full current state of the environment — all stored ontologies
          and their metadata — to <code>STDOUT</code>. Pass a string to filter by name.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv why</span>
       <p>Show all import paths that lead to a given ontology IRI — each path runs
          from the most distant importer down to the target.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv dep-graph</span>
       <p>Generate a PDF visualisation of the import dependency graph (requires
          Graphviz). Pass one or more root IRIs to limit the graph to a subgraph.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv doctor</span>
       <p>Check the environment for common problems: duplicate ontology IRIs, files
          missing an <code>owl:Ontology</code> declaration, and conflicting namespace
          prefixes.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv namespaces</span>
       <p>Show prefix-to-IRI namespace mappings extracted from ontology files. If no
          IRI is given, merges namespaces from all ontologies. <code>--closure</code>
          includes transitive imports.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv version</span>
       <p>Print the version of the installed <code>ontoenv</code> binary.</p>
     </div>

   </div>

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

.. raw:: html

   <div class="oe-cmd-grid">

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv init</span>
       <p>Create or overwrite the environment. Pass directory paths to trigger immediate
          discovery, or omit them to start empty. <code>--overwrite</code> rebuilds in place.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv add</span>
       <p>Register a single ontology by file path or URL. Fetches
          <code>owl:imports</code> unless <code>--no-imports</code> is passed.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv update</span>
       <p>Re-ingest modified local files and re-fetch stale remote ontologies.
          <code>--all</code> forces a full refresh regardless of modification times or
          cache age.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv config</span>
       <p>Read or update the persisted configuration. Supports
          <code>get</code>, <code>set</code>, <code>unset</code>, <code>add</code>,
          <code>remove</code>, and <code>list</code> subcommands.</p>
     </div>

     <div class="oe-cmd-card">
       <span class="cmd-name">ontoenv reset</span>
       <p>Remove the <code>.ontoenv/</code> directory entirely, wiping all cached
          state and configuration.</p>
     </div>

   </div>

.. code-block:: console

   # create an environment scanning two directories
   ontoenv init ./ontologies ./vendor

   # rebuild the environment in place
   ontoenv init ./ontologies --overwrite

   # add a local file (follows owl:imports by default)
   ontoenv add ./ontologies/myont.ttl

   # add a remote ontology without following its imports
   ontoenv add https://brickschema.org/schema/Brick --no-imports

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

Commands: extract graphs
------------------------

Two commands export graph data; they differ in scope and how imports are handled:

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

Global flags
------------

These flags are accepted by every subcommand:

.. raw:: html

   <div class="oe-protocol-box">
     <h3>Filtering</h3>
     <ul>
       <li><code>-i/--includes</code>, <code>-e/--excludes</code> — gitignore-style globs on
           file paths. Bare directories expand to <code>dir/**</code> automatically.</li>
       <li><code>--include-ontology</code>, <code>--exclude-ontology</code> — regex filters on
           ontology IRIs after parsing. Includes are a whitelist; excludes run last.</li>
     </ul>
   </div>

   <div class="oe-protocol-box">
     <h3>Behaviour</h3>
     <ul>
       <li><code>--remote-cache-ttl-secs</code> — max age of cached remote ontologies before
           re-fetch (default 86,400 s).</li>
       <li><code>-o/--offline</code> — skip all network access; use only what is already
           on disk.</li>
       <li><code>--require-ontology-names</code> — reject files that lack an
           <code>owl:Ontology</code> declaration.</li>
       <li><code>--strict</code> — treat missing imports as errors instead of warnings.</li>
       <li><code>-t/--temporary</code> — keep everything in memory; do not write
           <code>.ontoenv/</code> to disk.</li>
       <li><code>-p/--policy</code> — conflict-resolution policy when multiple files
           declare the same ontology IRI.</li>
       <li><code>-v/--verbose</code>, <code>--debug</code> — increase log verbosity.</li>
     </ul>
   </div>

Filtering by IRI
----------------

Use regex filters when path-based globs are not enough:

.. code-block:: console

   ontoenv init . \
     --include-ontology '^https://example\.com/' \
     --exclude-ontology 'experimental'

.. raw:: html

   <div class="oe-tip">
     <span class="oe-tip-icon">&#x1F4CC;</span>
     <p>
       <strong>Persisted filters:</strong> Regex lists are saved inside
       <code>.ontoenv/config.json</code> at init time and re-applied on every subsequent command.
       The <code>ontoenv config</code> helper currently supports <code>locations</code>,
       <code>includes</code>, and <code>excludes</code> via <code>add</code>/<code>remove</code>;
       edit the JSON file directly to change regex filters after init.
     </p>
   </div>

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
