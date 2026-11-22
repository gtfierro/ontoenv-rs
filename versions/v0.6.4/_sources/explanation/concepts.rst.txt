Concepts
========

Ontologies are identified by IRI, not by location
-------------------------------------------------

An RDF ontology declares its own name:

.. code-block:: turtle

   <https://example.org/site> a owl:Ontology .

That IRI is the ontology's identity. It is not a URL you are promised to be
able to fetch, and it need not resemble the path of the file containing it.
Two copies of the same ontology in different directories declare the same IRI;
a file renamed on disk still declares the same IRI.

Imports are written in the same currency:

.. code-block:: turtle

   <https://example.org/site> owl:imports <https://example.org/sensors> .

This says *what* is needed, not *where* it is. Something has to close that gap,
and that something is what OntoEnv is.

An environment is a name-to-location index
------------------------------------------

An environment is a directory — ``.ontoenv/`` by default — holding two things:

**The graphs themselves**, in a compact binary format (rdf5d) that can be
memory-mapped and read without parsing RDF text.

**A catalog** describing them: for each ontology, its canonical IRI, where it
came from, any aliases, its namespace prefixes, and its ``owl:imports``
targets.

The catalog is the interesting half. Because it records the import
relationships explicitly, OntoEnv can answer "what does this ontology need?"
by walking a graph in memory, rather than by re-parsing files. It is also
small — it holds facts *about* ontologies, not their triples — which is why
reopening an environment takes about the same time whether it holds a thousand
triples or a million.

Discovery: how ontologies get in
--------------------------------

Three routes:

- ``init <dir>`` / ``search_directories=`` — walk a directory, parse every
  file matching the include filters, record the IRI each one declares.
- ``add <path-or-url>`` — register one ontology, then follow its
  ``owl:imports`` and register those too, recursively.
- ``update()`` — revisit sources already known to the environment and re-read
  the ones that changed.

Remote ontologies are fetched over HTTP and cached on disk with a TTL, so the
second run of a program does not re-download anything.

Closures
--------

The transitive closure of an ontology is that ontology plus everything it
imports, plus everything *those* import, and so on. It is the set of graphs
you need in order to interpret the first one.

OntoEnv does not hand you a raw concatenation of that set. A closure is
*flattened*:

- **Resolved ``owl:imports`` statements are removed.** They have already been
  followed. Leaving them in invites a downstream consumer to follow them
  again — probably over the network, probably to a different version.
- **Ontology declarations are collapsed onto the root.** A merged graph with
  twelve ``owl:Ontology`` subjects is ambiguous about what it *is*. The result
  declares one ontology: the root you asked for.
- **SHACL prefix declarations are consolidated** onto the root, so
  ``sh:prefixes`` still resolves in the merged graph.
- **Duplicate triples appear once.**

The result is a single self-contained graph. That is usually what you want
when handing a closure to a reasoner, a validator, or a colleague.

When you want the other thing — exactly the graphs you named, untouched —
that is a **union**: ``ontoenv union`` / ``env.get_union(...)``. Unions do not
strip imports, do not collapse declarations, and do not de-duplicate across
graphs.

Aliases and canonical IRIs
--------------------------

Real ontologies get published at more than one IRI: with and without a version
suffix, over ``http`` and ``https``, at a vanity domain and a permanent one.
An alias maps an extra IRI onto an ontology already in the environment.

An alias may only point at a canonical IRI, never at another alias — so
resolution is always a single hop and chains cannot form. Aliases resolve
transparently everywhere an IRI is accepted.

Resolution policy
-----------------

When two files declare the same ontology IRI, something has to choose. The
resolution policy decides:

``default``
   Prefer the first-registered definition.

``latest``
   Prefer the most recently updated source.

``version``
   Prefer the highest ``owl:versionIRI`` / version property.

``ontoenv doctor`` reports duplicate IRIs so you can decide whether the
duplication is intentional at all.

Strict mode
-----------

By default an unresolvable import is a warning: OntoEnv assembles what it can
and tells you what is missing. In strict mode it is an error.

Non-strict is the right default for exploration — a closure missing one
obscure vocabulary is usually still useful. Strict is the right setting for
CI, where silently incomplete output is worse than a failure.

Persistent and temporary environments
-------------------------------------

A persistent environment writes to ``.ontoenv/`` and reopens quickly because
of its saved catalog. A temporary one (``--temporary`` /
``OntoEnv(temporary=True)``) keeps graphs and catalog in memory, leaves
nothing behind, and starts from scratch every time. The API is otherwise
identical.

To experiment with an existing environment without changing it, create an
explicit snapshot instead: ``env.temporary_snapshot()`` in Python (or
``env.new_temporary()`` in Rust). A snapshot copies the current catalog and
graph content into a separate in-memory environment; later changes are
independent in both directions. A ``root`` supplied to
``OntoEnv(temporary=True)`` is only a configuration base for source paths; it
does not select or load a saved environment.

Persistent environments allow one writer at a time. Any number of readers can
open the same environment read-only.

.. seealso::

   :doc:`views-and-copies` for what a closure gives you in Python, and
   :doc:`lifecycle` for the ways to open an environment.
