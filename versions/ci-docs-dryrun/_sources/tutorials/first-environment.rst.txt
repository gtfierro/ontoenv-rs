Your first environment
======================

In this tutorial you will build an OntoEnv environment from scratch, watch it
resolve an ``owl:imports`` chain, and export the result as a single file.
Everything runs locally except one optional step at the end.

You will need about 10 minutes.

.. raw:: html

   <div class="oe-minimum">
     <h3>The minimum you need to know</h3>
     <p>Three commands cover most day-to-day use:</p>
   </div>

.. code-block:: console

   $ ontoenv init ./ontologies              # build an environment from a directory
   $ ontoenv list ontologies                # see what it found
   $ ontoenv closure <IRI> out.ttl          # export an ontology + all its imports

Everything below is that, slowed down.

Install the CLI
---------------

The command-line tool ships two ways. Pick whichever is easier for you:

.. code-block:: bash

   # With a Rust toolchain:
   cargo install --locked ontoenv-cli

   # Or as part of the Python package (no Rust needed):
   pip install ontoenv

Check that it worked:

.. code-block:: console

   $ ontoenv version
   ontoenv 0.6.0 @ 90f61b73604620bb5582b18e1d2d9dcd004b2fea

Create some ontologies
----------------------

Make a directory with two small ontology files. The first describes a
building; the second describes sensors and is imported by the first.

.. code-block:: bash

   mkdir -p tutorial/ontologies
   cd tutorial

Save this as ``ontologies/sensors.ttl``:

.. code-block:: turtle

   @prefix owl:  <http://www.w3.org/2002/07/owl#> .
   @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
   @prefix sen:  <https://example.org/sensors#> .

   <https://example.org/sensors> a owl:Ontology .

   sen:Sensor       a owl:Class ; rdfs:label "Sensor" .
   sen:Thermometer  a owl:Class ; rdfs:subClassOf sen:Sensor .

And this as ``ontologies/site.ttl``:

.. code-block:: turtle

   @prefix owl:  <http://www.w3.org/2002/07/owl#> .
   @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
   @prefix sen:  <https://example.org/sensors#> .
   @prefix site: <https://example.org/site#> .

   <https://example.org/site> a owl:Ontology ;
       owl:imports <https://example.org/sensors> .

   site:Room a owl:Class ; rdfs:label "Room" .
   site:hasSensor a owl:ObjectProperty ;
       rdfs:domain site:Room ;
       rdfs:range sen:Sensor .

Note what ``site.ttl`` does *not* contain: any mention of where
``https://example.org/sensors`` lives. That is the problem OntoEnv exists to
solve. The import names an IRI, and something has to work out which file that
IRI corresponds to.

Initialize the environment
--------------------------

Run ``init`` and tell it which directory to scan:

.. code-block:: console

   $ ontoenv init ./ontologies
   Initialized environment with 2 unique ontologies (2 records).

Two things just happened. OntoEnv walked ``./ontologies``, parsed every RDF
file it found, and recorded the ontology IRI each file declares. It also
created a ``.ontoenv/`` directory to hold what it learned.

Every later command finds that directory by walking up from wherever you are,
so you can work from any subdirectory of ``tutorial/``.

See what was discovered
-----------------------

.. code-block:: console

   $ ontoenv list ontologies
   https://example.org/sensors
   https://example.org/site

   $ ontoenv list locations
   file:///home/you/tutorial/ontologies/sensors.ttl
   file:///home/you/tutorial/ontologies/site.ttl

OntoEnv now knows that the IRI ``https://example.org/sensors`` — the one
``site.ttl`` imports — comes from the file ``ontologies/sensors.ttl``. It made
that connection by reading the ``owl:Ontology`` declaration inside each file,
not by guessing from filenames. To see the pairing directly, use ``ontoenv
dump``.

For a summary of the environment itself:

.. code-block:: console

   $ ontoenv status
   Environment Path: /home/you/tutorial/.ontoenv
   Number of Ontologies: 2
   Last Updated: 2026-07-27 12:39:57 -06:00
   Store Size: 6.00 KiB

Trace an import
---------------

Ask which ontologies depend on ``sensors``:

.. code-block:: console

   $ ontoenv why https://example.org/sensors
   Why https://example.org/sensors:
   https://example.org/site -> https://example.org/sensors

``why`` prints every import path that reaches the given IRI, running from the
most distant importer down to the target. With one import there is only one
path; in a real project this is how you find out why some unexpected ontology
ended up in your environment.

Export a closure
----------------

Now for the payoff. Ask for ``site`` plus everything it transitively imports,
merged into a single file:

.. code-block:: console

   $ ontoenv closure https://example.org/site closure.ttl

Open ``closure.ttl``. It contains the triples from *both* files. Two details
are worth noticing:

- The ``owl:imports`` statement is gone. It was resolved, so keeping it would
  invite a consumer to try resolving it again.
- There is a single ``owl:Ontology`` declaration, for
  ``https://example.org/site``. The declarations from the imported graphs were
  collapsed onto that root.

That is what OntoEnv means by a closure: not a raw concatenation, but a
flattened graph that stands on its own. If you want the raw merge instead,
:doc:`../explanation/views-and-copies` explains the difference and
``ontoenv union`` gives you it.

To get just one graph, without its imports:

.. code-block:: console

   $ ontoenv get https://example.org/site

With no output file, ``get`` writes to standard output.

Add an ontology from the web
----------------------------

So far everything has been local. Add a published ontology and OntoEnv will
fetch it, then follow its imports and fetch those too:

.. code-block:: console

   $ ontoenv add https://brickschema.org/schema/1.4.4/Brick.ttl
   $ ontoenv list ontologies

You now have Brick and its dependencies alongside your own files. Remote
ontologies are cached on disk, so a second run does not re-download them —
:doc:`../how-to/work-offline` covers how long the cache is trusted and how to
work with no network at all.

Check for problems
------------------

.. code-block:: console

   $ ontoenv doctor
   No issues found.

``doctor`` looks for the mistakes that actually bite: two files declaring the
same ontology IRI, files with no ``owl:Ontology`` declaration at all, and
prefixes bound to conflicting namespaces.

.. code-block:: console

   $ ontoenv list missing

This lists imports nothing in the environment can resolve — usually a typo, a
dead URL, or a file you have not added yet.

Clean up
--------

.. code-block:: console

   $ ontoenv reset

This removes ``.ontoenv/`` and everything OntoEnv put in it. Your ontology
files are untouched.

What you learned
----------------

- An environment maps ontology IRIs to the places those ontologies live.
- ``init`` builds one from a directory; ``add`` registers individual files or
  URLs and follows their imports.
- ``closure`` exports an ontology merged with its transitive imports, cleaned
  up so the result stands alone.
- ``why``, ``doctor``, and ``list missing`` tell you what the import graph
  looks like and where it is broken.

Next steps
----------

- :doc:`python` — the same workflow from Python, with SPARQL at the end.
- :doc:`../how-to/choose-what-gets-loaded` — glob and regex filters for when
  scanning a whole directory pulls in too much.
- :doc:`../reference/cli` — every command and flag.
