Namespaces and prefix precedence
================================

A prefix gives a namespace IRI a short name: ``ex:ahu1`` can stand for
``urn:example:ahu1`` when ``ex`` is bound to ``urn:example:``. Changing a
binding changes how terms are displayed and how prefixed query names are
resolved. It does not rename the full IRIs in stored triples.

OntoEnv keeps ontology declarations and your saved prefix choices separately.
This lets you choose consistent names without editing every ontology source.

Where bindings come from
------------------------

Within one ontology, SHACL ``sh:declare`` entries take precedence over
parser declarations such as Turtle ``@prefix``. Parser declarations are
captured when the source is parsed and saved in the catalog. Namespace
lookups use that saved metadata; they do not read files or fetch URLs.

Saved user bindings take precedence over both kinds of ontology declaration.
They resolve conflicts in two ways:

* If you bind an existing prefix to another namespace, your namespace wins.
* If you give an existing namespace a new prefix, the ontology's other
  prefixes for that namespace are omitted from the merged mapping.

There is at most one saved user prefix for each namespace. Binding the same
namespace under another prefix removes its previous user binding.

Without a user binding, conflicting prefixes from different ontologies are
merged in traversal order, with later graphs winning. The order across the
whole environment is not guaranteed. Save a binding when you need a stable
choice.

Inspecting the effective mapping
--------------------------------

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv.connect("./ontology-env")
   env.get_namespaces()                       # all ontologies + saved bindings
   env.get_namespaces("urn:example:demo")      # one ontology + saved bindings
   env.get_namespaces("urn:example:demo", include_closure=True)
   env.get_bound_namespaces()                 # saved user bindings only

``get_closure()`` and ``get_union()`` views carry the prefixes of their
selected graphs, plus all saved user bindings. An unrelated ontology's
declarations are excluded from these views.

From the CLI, use ``ontoenv namespaces --json`` for the whole environment,
or ``ontoenv namespaces <ontology-IRI> --closure --json`` for a
closure.

Saving, changing, and removing a binding
----------------------------------------

In Python, these calls save changes immediately in
``.ontoenv/ontoenv.json`` for a writable persistent environment:

.. code-block:: python

   env.bind_namespace("site", "urn:example:")
   assert env.get_bound_namespaces()["site"] == "urn:example:"

   # Pick a different name for the same namespace; "site" is removed.
   env.bind_namespace("building", "urn:example:")

   # Remove the saved choice and let ontology declarations apply again.
   removed = env.unbind_namespace("building")  # True if a binding existed

The equivalent CLI commands are:

.. code-block:: console

   $ ontoenv config bind site urn:example:
   $ ontoenv config bind building urn:example:
   $ ontoenv config unbind building

Python returns ``False`` when unbinding an absent prefix; the CLI reports an
error. Removing a saved binding does not remove the source's declarations.
The effective mapping can therefore contain that prefix again afterward.

A read-only Python environment rejects these edits. A temporary environment
keeps them in memory only. Rust callers use ``bind_namespace`` or
``unbind_namespace`` followed by ``save_to_directory`` to persist changes.

Precedence when copying into rdflib
-----------------------------------

``env.copy_dataset()`` creates a mutable rdflib dataset. You can also pass
an existing dataset to add the environment's quads to it. Prefix conflicts
use this order:

1. Saved user bindings in the environment.
2. Existing bindings in the destination dataset, including rdflib defaults.
3. Ontology-declared prefixes.

A saved user binding replaces a conflicting destination prefix and becomes
the preferred prefix for its namespace. For example, saving
``schema -> urn:example:`` makes the copy use ``schema:ahu1`` even though
rdflib normally reserves ``schema`` for ``https://schema.org/``.

Ontology declarations preserve the destination's preferred bindings.
rdflib may assign a suffixed prefix such as ``ex1`` to resolve a collision.
This policy applies to ``copy_dataset()``; it does not imply that every
``copy_*`` graph method propagates the environment's bindings.

Bindings for one dataset or view
--------------------------------

Use rdflib's ``bind()`` for a choice that belongs only to one dataset:

.. code-block:: python

   dataset = env.get_dataset()
   dataset.bind("session", "urn:example:", override=True, replace=True)

This does not edit the environment configuration. A newly obtained dataset
uses the environment's bindings. Calling ``env.refresh_dataset(dataset)``
on a store-backed dataset also resets its bindings from the environment,
discarding local choices.

After ``env.bind_namespace()`` or ``env.unbind_namespace()``, obtain a new
dataset or view, or refresh an existing store-backed dataset, to see the
change. Existing copies keep their own bindings.

A ``ViewGraph`` supports ``view.bind(prefix, namespace, override=True)``
for local changes. Its binding method has no ``replace`` argument; its
``override`` flag controls replacement. The view's prefixes take precedence
over rdflib defaults when the view is serialized.

Editing source declarations
---------------------------

After changing a source's ``@prefix`` or ``sh:declare`` entries, call
``env.update()`` to refresh source metadata. Use ``env.update(force=True)``
to force re-parsing even if cache checks consider the source unchanged.
The CLI equivalent for a forced refresh is ``ontoenv update --all``.

Existing catalogs created before parser prefixes were recorded report only
their SHACL declarations until sources are re-parsed. A forced update fills
in the parser prefixes. Saved user bindings still take precedence after
that update.

For command signatures and configuration fields, see
:doc:`../reference/cli`, :doc:`../reference/python`, and
:doc:`../reference/configuration`.
