Choose what gets loaded
=======================

Pointing OntoEnv at a directory picks up every RDF file underneath it,
including test fixtures, drafts, and vendored copies. Two independent filter
layers narrow that down.

.. list-table::
   :header-rows: 1
   :widths: 20 20 30 30

   * - Layer
     - Matches on
     - CLI flags
     - Python arguments
   * - File filters
     - File paths, gitignore-style globs
     - ``-i/--includes``, ``-e/--excludes``
     - ``includes=``, ``excludes=``
   * - Ontology filters
     - Ontology IRIs, regular expressions
     - ``--include-ontology``, ``--exclude-ontology``
     - ``include_ontologies=``, ``exclude_ontologies=``

File filters run first and decide what gets parsed. Ontology filters run after
parsing, once the declared IRI is known.

Filter by file path
-------------------

Globs support ``*``, ``?``, and ``**``. A bare directory expands to
``dir/**`` automatically.

.. code-block:: console

   # Only Turtle files
   $ ontoenv init ./ontologies --includes '*.ttl'

   # Everything except the test fixtures
   $ ontoenv init . --excludes 'lib/tests' 'target'

.. code-block:: python

   env = OntoEnv.connect(
       "./ontology-env",
       search_directories=["."],
       includes=["*.ttl", "*.xml"],
       excludes=["lib/tests", "target"],
   )
   env.update()

The default include list is ``['*.ttl', '*.xml', '*.n3']``.

Filter by ontology IRI
----------------------

Use these when the file layout does not tell you what you need to know — for
example when one directory holds both your ontologies and vendored ones.

Includes act as a whitelist: if any include pattern is set, an ontology must
match one of them. Excludes run last and prune whatever slipped through.

.. code-block:: console

   $ ontoenv init . \
       --include-ontology '^https://example\.com/' \
       --exclude-ontology 'experimental'

.. code-block:: python

   env = OntoEnv.connect(
       "./ontology-env",
       search_directories=["."],
       include_ontologies=[r"^https://example\.com/"],
       exclude_ontologies=[r"experimental"],
   )

These are regular expressions, not globs, and they are matched against the
full ontology IRI.

Reject files without an ontology declaration
--------------------------------------------

By default a file with no ``owl:Ontology`` declaration is skipped with a
warning. To make it an error:

.. code-block:: console

   $ ontoenv init ./ontologies --require-ontology-names

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", require_ontology_names=True)

Change filters on an existing environment
-----------------------------------------

Filters are saved in ``.ontoenv/config.json`` and re-applied by every later
command. Path-based lists can be edited through ``ontoenv config``:

.. code-block:: console

   $ ontoenv config list
   $ ontoenv config add locations ./more-ontologies
   $ ontoenv config remove locations ./old-path
   $ ontoenv config add includes '*.n3'

``locations``, ``includes``, and ``excludes`` are list-valued, so they take
``config add`` / ``config remove`` rather than ``config set``. The
ontology-IRI regex lists (``include_ontologies``, ``exclude_ontologies``) have
no ``config`` support — edit ``.ontoenv/config.json`` directly, or pass the
flags on the next command.

From Python, passing a value to ``connect`` overrides the saved one; omitting
it keeps the saved one. Passing an empty list is an explicit override that
clears the setting:

.. code-block:: python

   # Keep everything as saved
   env = OntoEnv.connect("./ontology-env")

   # Override the saved search directories
   env = OntoEnv.connect("./ontology-env", search_directories=["./vendor"])

   # Explicitly clear them
   env = OntoEnv.connect("./ontology-env", search_directories=[])

Changing filters does not rescan anything by itself. The new settings apply on
the next ``update()``:

.. code-block:: python

   env = OntoEnv.connect("./ontology-env", excludes=["vendor"])
   env.update()   # now the exclusion takes effect

.. seealso::

   :doc:`../reference/configuration` for every setting and its default.
