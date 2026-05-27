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
       <a href="https://github.com/gtfierro/ontoenv-rs"><img src="https://img.shields.io/badge/GitHub-ontoenv--rs-181717?logo=github" alt="GitHub"></a>
       <a href="https://github.com/gtfierro/ontoenv-rs"><img src="https://img.shields.io/github/license/gtfierro/ontoenv-rs" alt="License"></a>
     </div>
     <code class="oe-install">pip install ontoenv</code>
     &nbsp;&nbsp;
     <code class="oe-install">cargo install ontoenv-cli</code>
   </div>

- **Import resolution** — Follows ``owl:imports`` declarations to fetch and cache every transitive dependency.
- **Dependency graph** — Builds a petgraph-backed directed graph for querying closures, finding roots, and detecting cycles.
- **Fast on-disk store** — Persists the environment in a compact binary RDF5D format; restores in milliseconds without re-parsing.
- **Python bindings** — Full PyO3 bindings expose every feature to Python, with native ``rdflib`` graph interop.
- **Flexible filtering** — Glob and regex filters on file paths and ontology IRIs let you include exactly what you need.
- **Remote caching** — Fetches remote ontologies over HTTP and caches them locally with a configurable TTL.

Quick start
-----------

.. code-block:: bash

   # Initialize a workspace from a directory of ontology files
   ontoenv init ./ontologies

   # List everything that was discovered
   ontoenv list ontologies

   # Get the full transitive closure for one ontology
   ontoenv closure https://example.com/myOntology

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv(
       path=".",
       recreate=True,
       search_directories=["./ontologies"],
       includes=["*.ttl"],
   )

   # Copy an ontology and all its imports into a mutable rdflib graph
   g, closure_names = env.copy_closure("https://example.com/myOntology")

Explore the docs
----------------

- `Getting Started <getting-started.html>`__ — Installation, first workspace, filters, and the Python quickstart.
- `Python API <python-api/index.html>`__ — Full reference for the ``ontoenv`` Python package.
- `CLI Reference <cli/index.html>`__ — All subcommands, flags, and configuration options.
- `Rust API <https://docs.rs/ontoenv>`__ — Auto-generated crate docs on docs.rs.
- `Changelog <changelog.html>`__ — Release history and what changed in each version.

.. toctree::
   :hidden:
   :maxdepth: 2

   getting-started
   python-api/index
   cli/index
   Rust API (docs.rs) <https://docs.rs/ontoenv>
   changelog

----

Need a plain-text snapshot for LLM ingestion? Grab `llms.txt <llms.txt>`_.
