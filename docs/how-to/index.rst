How-to guides
=============

.. raw:: html

   <div class="oe-section-intro">
     Short, goal-directed recipes. Each page answers one question and assumes
     you already know the basics from the <a href="../tutorials/index.html">tutorials</a>.
   </div>

.. rubric:: Controlling what is in the environment

:doc:`choose-what-gets-loaded`
   Glob and regex filters, so scanning a directory does not pull in test
   fixtures, drafts, or half the web.

:doc:`work-offline`
   Run with no network, and control how long cached remote ontologies are
   trusted.

:doc:`rename-and-alias`
   Store a third-party ontology under your own IRI, or route several IRIs to
   one canonical graph.

.. rubric:: Using an environment from code

:doc:`use-in-a-service`
   Connect once at startup, share the environment across requests, and handle
   multiple worker processes.

:doc:`query-with-sparql`
   Run SPARQL against a closure, a single graph, or the whole environment as
   an ``rdflib`` dataset.

:doc:`use-a-custom-graph-store`
   Route graph reads and writes through your own storage instead of
   OntoEnv's, and keep the two in sync.

.. rubric:: When something is wrong

:doc:`recover-an-environment`
   Fix a ``CatalogRecoveryError`` after an interrupted write.

:doc:`diagnose-imports`
   Track down a missing import, a duplicate ontology IRI, or an ontology you
   did not expect to be there.

.. toctree::
   :hidden:
   :maxdepth: 1

   choose-what-gets-loaded
   work-offline
   rename-and-alias
   use-in-a-service
   query-with-sparql
   use-a-custom-graph-store
   recover-an-environment
   diagnose-imports
