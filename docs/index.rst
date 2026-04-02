OntoEnv
=======

.. raw:: html

   <div class="oe-hero">
     <div class="tagline">
       A fast, lightweight environment manager for RDF ontologies —
       resolve imports, compute transitive closures, and work with
       ontology graphs from the CLI, Python, or Rust.
     </div>
     <div class="oe-badges">
       <a href="https://crates.io/crates/ontoenv"><img src="https://img.shields.io/crates/v/ontoenv.svg" alt="crates.io"></a>
       <a href="https://pypi.org/project/ontoenv/"><img src="https://img.shields.io/pypi/v/ontoenv.svg" alt="PyPI"></a>
       <a href="https://docs.rs/ontoenv"><img src="https://docs.rs/ontoenv/badge.svg" alt="docs.rs"></a>
       <a href="https://github.com/gtfierro/ontoenv-rs"><img src="https://img.shields.io/github/license/gtfierro/ontoenv-rs" alt="License"></a>
     </div>
     <code class="oe-install">pip install ontoenv</code>
     &nbsp;&nbsp;
     <code class="oe-install">cargo install ontoenv-cli</code>
   </div>

   <div class="oe-features">

     <div class="oe-card">
       <span class="oe-card-icon">&#x1F9E9;</span>
       <h3>Import resolution</h3>
       <p>Automatically follows <code>owl:imports</code> declarations to fetch and cache every transitive dependency.</p>
     </div>

     <div class="oe-card">
       <span class="oe-card-icon">&#x1F4CA;</span>
       <h3>Dependency graph</h3>
       <p>Builds a petgraph-backed directed graph so you can query closures, find roots, and detect cycles.</p>
     </div>

     <div class="oe-card">
       <span class="oe-card-icon">&#x26A1;</span>
       <h3>Fast on-disk store</h3>
       <p>Persists the environment in a compact binary RDF5D format — restores in milliseconds without re-parsing.</p>
     </div>

     <div class="oe-card">
       <span class="oe-card-icon">&#x1F40D;</span>
       <h3>Python bindings</h3>
       <p>Full PyO3 bindings expose every feature to Python, with native <code>rdflib</code> graph interop.</p>
     </div>

     <div class="oe-card">
       <span class="oe-card-icon">&#x1F527;</span>
       <h3>Flexible filtering</h3>
       <p>Glob and regex filters on file paths and ontology IRIs let you include exactly what you need.</p>
     </div>

     <div class="oe-card">
       <span class="oe-card-icon">&#x1F310;</span>
       <h3>Remote caching</h3>
       <p>Fetches remote ontologies over HTTP and caches them locally with a configurable TTL.</p>
     </div>

   </div>

Quick start
-----------

.. code-block:: bash

   # Initialize a workspace from a directory of ontology files
   ontoenv init ./ontologies

   # List everything that was discovered
   ontoenv list ontologies

   # Get the full transitive closure for one ontology
   ontoenv closure https://example.com/myOntology

.. raw:: html

   <div class="oe-quickstart"></div>

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv(
       path=".",
       recreate=True,
       search_directories=["./ontologies"],
       includes=["*.ttl"],
   )

   # Get a merged rdflib graph of an ontology and all its imports
   g = env.get_closure("https://example.com/myOntology")

Explore the docs
----------------

.. raw:: html

   <div class="oe-nav">
     <a class="oe-nav-card" href="getting-started.html">
       <span class="nav-icon">&#x1F680;</span>
       <strong>Getting Started</strong>
       <span>Installation, first workspace, filters, and the Python quickstart.</span>
     </a>
     <a class="oe-nav-card" href="python-api/index.html">
       <span class="nav-icon">&#x1F40D;</span>
       <strong>Python API</strong>
       <span>Full reference for the <code>ontoenv</code> Python package.</span>
     </a>
     <a class="oe-nav-card" href="cli/index.html">
       <span class="nav-icon">&#x1F5A5;</span>
       <strong>CLI Reference</strong>
       <span>All subcommands, flags, and configuration options.</span>
     </a>
     <a class="oe-nav-card" href="https://docs.rs/ontoenv" target="_blank">
       <span class="nav-icon">&#x1F4D6;</span>
       <strong>Rust API</strong>
       <span>Auto-generated crate docs on docs.rs.</span>
     </a>
   </div>

.. toctree::
   :hidden:
   :maxdepth: 2

   getting-started
   python-api/index
   cli/index
   Rust API (docs.rs) <https://docs.rs/ontoenv>

----

Need a plain-text snapshot for LLM ingestion? Grab `llms.txt <llms.txt>`_.
