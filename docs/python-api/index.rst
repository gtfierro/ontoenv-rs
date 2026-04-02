Python API Reference
====================

.. raw:: html

   <div class="oe-section-intro">
     The <strong>ontoenv</strong> Python package exposes the full Rust core through
     <a href="https://pyo3.rs">PyO3</a> bindings, with native
     <a href="https://rdflib.readthedocs.io">rdflib</a> graph interop.
     Pre-built wheels are published on PyPI — no Rust toolchain required.
   </div>

Install
-------

.. code-block:: bash

   pip install ontoenv   # Python 3.9+

Key methods
-----------

.. raw:: html

   <div class="oe-method-grid">

     <div class="oe-method-card">
       <span class="method-sig">OntoEnv(search_directories, includes, offline, …)</span>
       <p>Create or open an environment. Accepts <code>search_directories</code> (paths to crawl),
          <code>offline</code> (skip network), <code>temporary</code> (keep everything in memory),
          glob/regex filters, and a custom <code>graph_store</code>.</p>
     </div>

     <div class="oe-method-card">
       <span class="method-sig">env.update(all=False)</span>
       <p>Re-run discovery with the configured directories. Pass <code>all=True</code> to force
          re-fetching of all remote ontologies regardless of cache age.</p>
     </div>

     <div class="oe-method-card">
       <span class="method-sig">env.add(location, fetch_imports=True)</span>
       <p>Register an ontology from a file path, URL, or an in-memory
          <code>rdflib.Graph</code> that contains an <code>owl:Ontology</code> declaration.
          Set <code>fetch_imports=False</code> to store only the root graph.</p>
     </div>

     <div class="oe-method-card">
       <span class="method-sig">env.get_closure(name, destination_graph=None, recursion_depth=-1)</span>
       <p>Return a merged <code>(Graph, int)</code> pair — the ontology named <code>name</code>
          plus all its transitive imports, and the count of imported graphs.
          Pass a <code>destination_graph</code> to merge into an existing graph in place.</p>
     </div>

     <div class="oe-method-card">
       <span class="method-sig">env.get_graph(name)</span>
       <p>Return the stored <code>rdflib.Graph</code> for a single ontology IRI — useful when
          you only need one graph rather than a full closure.</p>
     </div>

     <div class="oe-method-card">
       <span class="method-sig">env.import_dependencies(graph, fetch_missing=False)</span>
       <p>Mutate an <code>rdflib.Graph</code> in place, inserting triples from all ontologies
          declared in its <code>owl:imports</code> statements. Set <code>fetch_missing=True</code>
          to download any imports not yet in the environment.</p>
     </div>

   </div>

Example
-------

.. code-block:: python

   from pathlib import Path
   from ontoenv import OntoEnv

   env = OntoEnv(
       search_directories=["./ontologies"],
       includes=["*.ttl"],
       strict=False,
   )

   # Add a remote ontology and follow its imports
   env.add("https://brickschema.org/schema/1.4.4/Brick.ttl")

   # Retrieve just the Brick graph (no imports merged)
   brick = env.get_graph("https://brickschema.org/schema/1.4/Brick")

   # Retrieve Brick with all transitive imports merged
   g, n_imports = env.get_closure("https://brickschema.org/schema/1.4/Brick")
   print(f"Merged {n_imports} imports — {len(g)} triples total")

.. raw:: html

   <div class="oe-tip">
     <span class="oe-tip-icon">&#x1F4A1;</span>
     <p>
       <strong>Custom storage:</strong> Pass a <code>graph_store=</code> object to route all
       graph reads and writes through your own backend. See
       <a href="graph-store.html">Graph Store Interface</a> for the protocol.
     </p>
   </div>

.. toctree::
   :maxdepth: 1

   ontoenv
   graph-store
