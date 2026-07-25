Getting Started
===============

.. raw:: html

   <div class="oe-section-intro">
     Everything you need to set up a workspace, control what gets loaded, and keep your cached
     ontologies fresh — using the <strong>CLI</strong>, the <strong>Python package</strong>, or both.
   </div>

Install
-------

**1. Install the CLI (Rust)** — requires a Rust toolchain; the binary is published on crates.io.

.. code-block:: bash

   cargo install ontoenv-cli

   # or build from this workspace after cloning:
   cargo build -p ontoenv-cli --release

**2. Install the Python package** — pre-built wheels on PyPI; no Rust toolchain needed.

.. code-block:: bash

   pip install ontoenv

   # To build from source (e.g., after cloning):
   cd python && uv run maturin develop

Initialize a workspace
----------------------

``ontoenv`` stores its metadata under ``.ontoenv/``. Run ``init`` and pass the directories you want
scanned:

.. code-block:: console

   # Discover every ontology under the current directory
   ontoenv init .

   # Create an empty container (add ontologies later with ontoenv add)
   ontoenv init

   # Seed from multiple directories at once
   ontoenv init ./ontologies ./models

.. raw:: html

   <div class="oe-tip">
     <span class="oe-tip-icon">&#x1F4A1;</span>
     <p>
       <strong>Re-running init?</strong> Pass <code>--overwrite</code> to rebuild the environment
       in place. Every command walks up from the current directory to find <code>.ontoenv/</code>
       unless <code>ONTOENV_DIR</code> is set.
     </p>
   </div>

Add and refresh ontologies
--------------------------

Once the workspace exists, register individual files or URLs and keep them fresh:

.. code-block:: console

   # Add a single ontology (fetches owl:imports by default)
   ontoenv add ./ontologies/site.ttl

   # Add without following imports
   ontoenv add ./ontologies/site.ttl --no-imports

   # Refresh stale remote cache entries
   ontoenv update

   # Force every ontology to re-download
   ontoenv update --all

Use ``ontoenv list ontologies`` or ``ontoenv dump`` to inspect what is currently cached.

Control discovery scope
-----------------------

Two independent filter layers govern what gets pulled in:

.. raw:: html

   <div class="oe-protocol-box">
     <h3>Filter layers</h3>
     <ul>
       <li><code>-i/--includes</code> and <code>-e/--excludes</code> — gitignore-style globs on
           <strong>file paths</strong>. Bare directories (e.g. <code>lib/tests</code>) auto-expand
           to <code>lib/tests/**</code>.</li>
       <li><code>--include-ontology</code> / <code>--exclude-ontology</code> — regex patterns run
           against <strong>ontology IRIs</strong> after parsing. Includes act as a whitelist;
           excludes prune what slips through.</li>
     </ul>
   </div>

.. code-block:: console

   ontoenv init ontologies \
     --includes '*.ttl' \
     --exclude-ontology 'experimental' \
     --include-ontology '^https://example\.com/'

Settings are saved into ``.ontoenv/config.json`` and re-applied by every subsequent command.

Cache strategy and TTL
----------------------

Remote ontologies are stored on disk. Two knobs control how aggressively they are refreshed:

.. raw:: html

   <div class="oe-protocol-box">
     <h3>Cache options</h3>
     <ul>
       <li><code>use_cached_ontologies</code> — when enabled, discovery is skipped at init time
           and the environment only fills when you explicitly call <code>add</code> or
           <code>update</code>.</li>
       <li><code>--remote-cache-ttl-secs</code> — maximum age (seconds) of a cached remote
           ontology before <code>update</code> re-fetches it. Default: <strong>86,400</strong>
           (24 hours).</li>
     </ul>
   </div>

.. code-block:: console

   # Keep cached copies for a week before refreshing
   ontoenv update --remote-cache-ttl-secs 604800

   # Persist these as defaults
   ontoenv config set remote_cache_ttl_secs 604800
   ontoenv config add locations ./ontologies

Python quickstart
-----------------

Create or reconnect to a persistent environment, add an ontology, and ask for
its imports closure:

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect("./ontology-env")
   site = env.add("./ontologies/site.ttl")

   # Retrieve a read-only ViewGraph of an ontology and all its transitive imports
   g, imported = env.get_closure(site)
   print(f"Read {len(imported)} graphs, {len(g)} triples total")

   # Use copy_closure when you need a mutable materialized graph
   mutable_g, imported = env.copy_closure(site)

   env.close()

The next ``connect`` reuses the saved environment without scanning every RDF
triple. For a scratch environment that stays entirely in memory, use
``OntoEnv(temporary=True)`` instead.

Directory discovery and filters are also available:

.. code-block:: python

   env = OntoEnv.connect(
       "./ontology-env",
       search_directories=["./ontologies"],
       includes=["*.ttl", "*.xml"],
       exclude_ontologies=[r"experimental"],
       offline=True,
   )
   env.update()

Persistent lifecycle
--------------------

Use ``connect`` for the normal persistent lifecycle:

.. code-block:: python

   # First run: create. Later runs: reconnect quickly.
   env = OntoEnv.connect("./environment")
   print(env.get_ontology_names())
   env.close()

With a custom graph store, the same call reads and indexes existing graphs on
first use, then incorporates identifiable external changes on later
connections. If OntoEnv cannot tell what changed, it asks for ``sync="full"``
rather than silently scanning everything.

The ``with`` statement is optional. It is convenient for scripts and calls
``close()`` automatically. A webserver can instead retain
``OntoEnv.connect(...)`` in application state and call ``close()`` from its
shutdown hook. See :doc:`python-api/lifecycle` for a guided explanation of
long-lived services, custom-store synchronization, and multi-worker use.

Building the docs
-----------------

The documentation lives under ``docs/`` with its own ``pyproject.toml``:

.. code-block:: bash

   cd docs
   uv sync                              # install Sphinx + theme
   uv run sphinx-build -M html . _build
   open _build/html/index.html

Helper scripts in the repo root:

.. code-block:: bash

   ./builddocs          # sync deps, build the extension, render HTML
   ./builddocs llms     # also render docs/_build/llms.txt for LLM ingestion
