OntoEnv
=======

.. raw:: html

   <div class="oe-hero">
     <div class="tagline">
       An environment manager for RDF ontologies. Point it at some files or
       URLs and it resolves every <code>owl:imports</code> for you — from the
       command line, Python, or Rust.
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

The problem OntoEnv solves
--------------------------

An ontology is rarely self-contained. It declares ``owl:imports``, those
imports declare their own imports, and before long a single file needs a dozen
others — some local, some on the web, some moved or renamed since they were
published. Assembling that set by hand is tedious and easy to get wrong.

OntoEnv keeps an *environment*: a directory of ontologies it has discovered,
plus everything it learned about them — canonical names, source locations,
namespace prefixes, and the import graph connecting them. Ask for one
ontology and you can get its complete imports closure as a single graph, in
milliseconds, without re-parsing anything.

The minimum you need to know
----------------------------

Point OntoEnv at a directory, then ask for a closure. That is the whole idea.

.. code-block:: console

   $ pip install ontoenv
   $ ontoenv init ./ontologies                              # build the environment
   $ ontoenv closure https://example.org/site closure.ttl   # export IRI + its imports

The same thing from Python:

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect("./ontology-env", search_directories=["./ontologies"])
   env.update()

   view, imported = env.get_closure("https://example.org/site")
   print(f"{len(imported)} graphs, {len(view)} triples")

Two rules explain most of the rest of the API: ``connect`` opens an
environment but never reads your files — ``update`` does that, explicitly. And
``get_*`` returns a fast read-only view, while ``copy_*`` returns a mutable
``rdflib`` graph.

Where to go next
----------------

.. raw:: html

   <div class="oe-cards">
     <a class="oe-card" href="tutorials/index.html">
       <h3>Tutorials</h3>
       <p>Start here. Two short, hands-on walkthroughs that take you from an
       empty directory to a working environment — one for the CLI, one for
       Python.</p>
       <span class="oe-card-cta">Learn the basics &rarr;</span>
     </a>
     <a class="oe-card" href="how-to/index.html">
       <h3>How-to guides</h3>
       <p>Recipes for specific jobs: filtering what gets loaded, working
       offline, running inside a long-lived service, plugging in your own
       storage, recovering a broken environment.</p>
       <span class="oe-card-cta">Solve a problem &rarr;</span>
     </a>
     <a class="oe-card" href="reference/index.html">
       <h3>Reference</h3>
       <p>Every CLI command and flag, every Python method, every
       configuration key. Look things up here.</p>
       <span class="oe-card-cta">Look something up &rarr;</span>
     </a>
     <a class="oe-card" href="explanation/index.html">
       <h3>Explanation</h3>
       <p>How OntoEnv thinks: what a closure actually contains, why there are
       five ways to open an environment, when views beat copies, and what the
       performance numbers mean.</p>
       <span class="oe-card-cta">Understand the design &rarr;</span>
     </a>
   </div>

What you get
------------

- **Import resolution** — follows ``owl:imports`` to fetch and cache every
  transitive dependency, local or remote.
- **A queryable dependency graph** — ask for a closure, find every importer
  of an ontology, or detect cycles.
- **Fast restarts** — the environment is stored in a compact binary format
  and reopens in milliseconds without re-parsing RDF.
- **Read-only views** — query a 200k-triple closure through SPARQL without
  materializing it in memory.
- **Three front ends** — a CLI, Python bindings with native ``rdflib``
  interop, and the underlying Rust crate.

Upgrading
---------

Coming from 0.5? :doc:`migration-0.6` lists the API changes you need to make.
The full release history is in the :doc:`changelog`.

.. toctree::
   :hidden:
   :maxdepth: 2

   tutorials/index
   how-to/index
   reference/index
   explanation/index

.. toctree::
   :hidden:
   :caption: Project

   migration-0.6
   development
   Rust API (docs.rs) <https://docs.rs/ontoenv>
   changelog

----

Need a plain-text snapshot for LLM ingestion? Grab `llms.txt <llms.txt>`_.
