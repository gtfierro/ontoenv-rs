Diagnose import problems
========================

Start with ``doctor``
---------------------

.. code-block:: console

   $ ontoenv doctor

This checks for the three problems that cause most confusion:

- two files declaring the **same ontology IRI**
- files with **no ``owl:Ontology`` declaration**, which are skipped
- the same prefix bound to **conflicting namespaces** in different files

"An import is not resolving"
----------------------------

.. code-block:: console

   $ ontoenv list missing

Every IRI listed here is an ``owl:imports`` target that nothing in the
environment provides. From Python:

.. code-block:: python

   for iri in env.missing_imports():
       print(iri)

Common causes, in rough order of likelihood:

1. **The file is there but was filtered out.** Check your includes and
   excludes with ``ontoenv config list``. See
   :doc:`choose-what-gets-loaded`.
2. **The file has no ontology declaration**, so OntoEnv never learned which
   IRI it provides. ``ontoenv doctor`` reports these.
3. **The declared IRI differs from the imported IRI** — a version suffix, or
   ``http`` versus ``https``. Compare ``ontoenv list ontologies`` against the
   import target. Fix it with an alias (:doc:`rename-and-alias`).
4. **You are offline** and the ontology is remote. Check with
   ``ontoenv status``.

To make a missing import a hard error instead of a warning:

.. code-block:: console

   $ ontoenv update --strict

.. code-block:: python

   env.set_strict(True)

In Python, an unresolved import passed to ``copy_graph`` raises
:class:`ontoenv.UnresolvedImportError`, which subclasses ``LookupError``. An
IRI that was never declared or attempted anywhere raises a plain
``ValueError``, so you can catch the two cases separately:

.. code-block:: python

   from ontoenv import UnresolvedImportError

   try:
       g = env.copy_graph(iri)
   except UnresolvedImportError as e:
       log.warning("known import could not be resolved: %s", e)
   except ValueError:
       log.error("no such ontology: %s", iri)

"Why is this ontology in my environment?"
-----------------------------------------

.. code-block:: console

   $ ontoenv why https://brickschema.org/schema/Brick

``why`` prints every import path that reaches that IRI, each running from the
most distant importer down to the target. This is how you find the one file
that dragged in a whole subtree.

From Python, for direct importers only:

.. code-block:: python

   env.get_importers("https://brickschema.org/schema/Brick")

"What is actually in this closure?"
-----------------------------------

.. code-block:: python

   names = env.list_closure("https://example.org/site")
   print(names)

   # Or with the merged view:
   view, names = env.get_closure("https://example.org/site")

To limit how deep import resolution goes:

.. code-block:: python

   view, names = env.get_closure("https://example.org/site", recursion_depth=2)

Visualize the dependency graph
------------------------------

.. code-block:: console

   # Whole environment (requires Graphviz)
   $ ontoenv dep-graph

   # Limited to one root and its subgraph
   $ ontoenv dep-graph https://example.org/site --output site_deps.pdf

Inspect the raw state
---------------------

.. code-block:: console

   $ ontoenv status            # summary: location, count, active settings
   $ ontoenv status --json     # same, machine-readable
   $ ontoenv dump              # every ontology and its metadata
   $ ontoenv dump brick        # filtered by name

For prefix conflicts specifically:

.. code-block:: console

   $ ontoenv namespaces
   $ ontoenv namespaces https://example.org/site --closure

Turn up the logging
-------------------

.. code-block:: console

   $ ontoenv -v update       # info level
   $ ontoenv --debug update  # debug level
