OntoEnv
=======

.. raw:: html

   <div class="oe-hero">
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

An ``owl:imports`` statement gives you an IRI. It does not tell you which file
to open. The ontology may live in a local checkout, at a URL, behind an alias,
or nowhere you can currently reach.

OntoEnv is a library and command-line tool that maps ontology IRIs to the files
or URLs where their RDF graphs can be found. It follows ``owl:imports`` to
build a dependency graph, then saves the graphs and their mappings in an
*environment*. Reopening that environment reads the saved catalog instead of
parsing every source again.

Basic workflow
--------------

Scan a directory once. Then ask for an ontology by IRI:

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

Two distinctions explain most of the API:

- ``connect`` opens saved state. ``update`` reads ontology sources.
- ``get_*`` returns a read-only view. ``copy_*`` allocates a mutable
  ``rdflib`` graph.

These names distinguish catalog access from source access, and views from
allocated copies.

Where to go next
----------------

.. raw:: html

   <div class="oe-cards">
     <a class="oe-card" href="tutorials/index.html">
       <h3>Tutorials</h3>
       <p>Build a small environment and inspect an import closure, from the
       CLI or Python.</p>
       <span class="oe-card-cta">Work through an example &rarr;</span>
     </a>
     <a class="oe-card" href="how-to/index.html">
       <h3>How-to guides</h3>
       <p>Specific tasks: constrain discovery, work offline, run a service,
       attach storage, or recover after an interrupted write.</p>
       <span class="oe-card-cta">Find a procedure &rarr;</span>
     </a>
     <a class="oe-card" href="reference/index.html">
       <h3>Reference</h3>
       <p>CLI commands, Python methods, configuration keys, and storage
       interfaces.</p>
       <span class="oe-card-cta">Consult the reference &rarr;</span>
     </a>
     <a class="oe-card" href="explanation/index.html">
       <h3>Explanation</h3>
       <p>Design constraints and trade-offs: closure transformations,
       lifecycle choices, views versus copies, and performance limits.</p>
       <span class="oe-card-cta">Read the design notes &rarr;</span>
     </a>
   </div>

What OntoEnv saves
------------------

- RDF graphs from local files and remote URLs.
- Canonical ontology IRIs, aliases, source locations, and namespace prefixes.
- Direct and transitive ``owl:imports`` relationships, including missing
  imports and cycles.
- A catalog that can be reopened without parsing every stored graph.

The same environment is available through the CLI, Python bindings with
``rdflib`` interoperability, and the Rust crate.

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
