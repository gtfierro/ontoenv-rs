Your first environment
======================

This tutorial builds an environment from two local files, resolves their
``owl:imports`` relationship, and exports the result as one graph. The final
step fetches a remote ontology; it is optional. Everything before that step
runs locally.

At a glance
-----------

The local workflow creates an environment, inspects the ontology IRIs it
recorded, and exports one ontology with its imports:

.. code-block:: console

   $ ontoenv init ./ontologies              # scan files and create .ontoenv/
   $ ontoenv list ontologies                # print the recorded ontology IRIs
   $ ontoenv closure <IRI> out.ttl          # write the IRI and its imports to out.ttl

Install the CLI
---------------

Install the command-line tool through Cargo or PyPI:

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

``init`` walked ``./ontologies``, parsed each matching RDF file, and recorded
the ontology IRI that each file declares. It also created ``.ontoenv/``, which
holds the graphs and the catalog used to find them later.

The environment now contains two graphs and the IRI declared by each one.

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

The environment now maps ``https://example.org/sensors`` — the IRI imported by
``site.ttl`` — to ``ontologies/sensors.ttl``. That mapping comes from the
``owl:Ontology`` declaration in the file, not from its filename. ``ontoenv
dump`` shows the complete mapping.

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

Request ``site`` and every ontology reachable through its imports, merged into
a single file:

.. code-block:: console

   $ ontoenv closure https://example.org/site closure.ttl

Open ``closure.ttl``. It contains the triples from *both* files. Two details
are worth noticing:

- The ``owl:imports`` statement is gone. It was resolved, so keeping it would
  invite a consumer to try resolving it again.
- There is a single ``owl:Ontology`` declaration, for
  ``https://example.org/site``. The declarations from the imported graphs were
  collapsed onto that root.

This is OntoEnv's closure representation: a flattened graph intended for a
consumer that should not resolve imports again. For the unmodified merge,
use ``ontoenv union``; :doc:`../explanation/views-and-copies` describes the
difference.

At this point the local example is complete: one command resolved the import
edge and produced a graph that can be handed to another RDF tool.

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

The first command downloads Brick, follows its imports, and stores the graphs.
The second prints every ontology IRI now recorded in the environment, so the
output includes Brick and its dependencies alongside the two local graphs.
Remote
ontologies are cached on disk, so a second run does not re-download them —
:doc:`../how-to/work-offline` covers how long the cache is trusted and how to
work with no network at all.

Check for problems
------------------

.. code-block:: console

   $ ontoenv doctor
   No issues found.

``doctor`` checks for duplicate ontology IRIs, files with no ``owl:Ontology``
declaration, and prefixes bound to conflicting namespaces.

.. code-block:: console

   $ ontoenv list missing

This prints every ``owl:imports`` IRI that does not resolve to a graph in the
environment. Common causes are a misspelled IRI, an unavailable URL, or a
source that has not been added.

Clean up
--------

.. code-block:: console

   $ ontoenv reset

``reset`` asks for confirmation. If confirmed, it removes ``.ontoenv/`` and
everything OntoEnv put in it. Your ontology files are untouched.

What you built
--------------

- The environment maps ontology IRIs to the places those ontologies live.
- ``init`` builds one from a directory; ``add`` registers individual files or
  URLs and follows their imports.
- ``closure`` exports an ontology with its transitive imports, transformed so
  a downstream consumer does not need to resolve the same imports again.
- ``why``, ``doctor``, and ``list missing`` tell you what the import graph
  looks like and where it is broken.

Next steps
----------

- :doc:`python` — the same workflow from Python, with SPARQL at the end.
- :doc:`../how-to/choose-what-gets-loaded` — glob and regex filters for when
  scanning a whole directory pulls in too much.
- :doc:`../reference/cli` — every command and flag.
