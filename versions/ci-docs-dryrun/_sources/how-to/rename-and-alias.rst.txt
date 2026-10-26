Rename and alias ontologies
===========================

Two different problems, two different tools:

- **Rename** — you want an ontology stored under a *different* IRI than the
  one it declares. The old IRI stops working.
- **Alias** — you want an *additional* IRI to resolve to an existing ontology.
  Both IRIs keep working.

Store an ontology under your own IRI
------------------------------------

Use ``--rename`` / ``rename=`` when loading a third-party ontology that you
want addressed by a local or canonical IRI, without editing the source file.

.. code-block:: console

   $ ontoenv add ./vendor/upstream.ttl \
       --rename https://my-org.com/local/upstream

   # Same, without following owl:imports
   $ ontoenv add ./vendor/upstream.ttl \
       --rename https://my-org.com/local/upstream \
       --no-imports

.. code-block:: python

   name = env.add(
       "./vendor/upstream.ttl",
       rename="https://my-org.com/local/upstream",
   )
   # name == 'https://my-org.com/local/upstream'

   # add_no_imports takes the same argument
   env.add_no_imports("./vendor/upstream.ttl", rename="https://my-org.com/local/upstream")

.. _renaming-on-add:

What the rename rewrites
------------------------

Every occurrence of the original IRI in the stored graph is rewritten, in both
subject and object position, with one deliberate exception:

.. list-table::
   :header-rows: 1
   :widths: 50 50

   * - Before
     - After
   * - ``<original> a owl:Ontology``
     - ``<new> a owl:Ontology``
   * - ``<original> owl:imports <X>``
     - ``<new> owl:imports <X>``
   * - ``<original> sh:prefixes <original>``
     - ``<new> sh:prefixes <new>``
   * - ``<X> sh:prefixes <original>``
     - ``<X> sh:prefixes <new>``
   * - ``<original> owl:versionIRI <original>``
     - ``<new> owl:versionIRI <original>``

The last row is the exception: the subject is rewritten but the version IRI
*value* is preserved, because it identifies which upstream version you loaded.

.. warning::

   After a rename the original IRI is no longer addressable. Other ontologies
   that ``owl:imports`` the original IRI will not resolve to the renamed copy
   — re-add or update them, or add an alias.

Rename an ontology already in the environment
---------------------------------------------

.. code-block:: python

   new_iri = env.rename_graph_iri(
       "https://example.org/old",
       "https://example.org/new",
   )

This applies the same rewrite rules to the stored graph and rebuilds the
import dependency graph so existing imports point at the new name.

Route several IRIs to one graph
-------------------------------

An alias is a second name for an ontology already in the environment. Use it
when the same ontology is published under more than one IRI, or when a
consumer imports a URL that redirects to your canonical version.

.. code-block:: python

   env.add_alias(
       "https://example.org/legacy/site",
       "https://example.org/site",
   )

   env.resolve_alias("https://example.org/legacy/site")
   # 'https://example.org/site'

   env.get_aliases_for("https://example.org/site")
   # ['https://example.org/legacy/site']

   env.is_canonical_iri("https://example.org/legacy/site")
   # False

   env.remove_alias("https://example.org/legacy/site")

Aliases resolve transparently: ``get_graph``, ``get_closure``, ``uri in env``,
and ``env[uri]`` all accept an alias and return the canonical graph.

An alias may only point at a canonical IRI, never at another alias. That rule
keeps resolution to a single hop and makes alias chains impossible.
