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

The Python API exposes the same configuration surface as the CLI:

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.create(
       ".",
       search_directories=["."],    # scan the current project
       includes=["*.ttl", "*.xml"],
       include_ontologies=[r"^https://example\.com/"],
       exclude_ontologies=[r"experimental"],
       offline=True,
       use_cached_ontologies=False,
       remote_cache_ttl_secs=86400,
   )

   env.update(all=True)

   # Retrieve a read-only ViewGraph of an ontology and all its transitive imports
   g, imported = env.get_closure("https://example.com/myOntology")
   print(f"Read {len(imported)} graphs, {len(g)} triples total")

   # Use copy_closure when you need a mutable materialized graph
   mutable_g, imported = env.copy_closure("https://example.com/myOntology")

Pass ``use_cached_ontologies=True`` to start with an empty container that only fills when you
explicitly call ``add`` or ``update``.

Persistent lifecycle
--------------------

Use the explicit lifecycle entry points when the environment should survive the
current process:

.. code-block:: python

   # Create a new catalog. Fails if an environment already exists.
   with OntoEnv.create("./environment") as env:
       env.add("./ontologies/site.ttl")

   # Warm open: load catalog metadata without reading ontology graphs.
   with OntoEnv.open("./environment", read_only=True) as env:
       print(env.get_ontology_names())

   # Deliberately scan an existing custom graph store exactly once.
   with OntoEnv.adopt("./adopted-environment", graph_store) as env:
       print(env.get_ontology_names())

``adopt`` does not fetch network imports. Out-of-band store mutations must be
reconciled explicitly with ``refresh_from_store()`` (when revisions are
available), ``refresh_from_store(graphs=[...])``, or
``refresh_from_store(full=True)``.

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
