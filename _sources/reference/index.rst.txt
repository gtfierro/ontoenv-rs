Reference
=========

.. raw:: html

   <div class="oe-section-intro">
     Look-up material. Complete, terse, and organized by what a thing is
     rather than by what you are trying to do.
   </div>

:doc:`cli`
   Every ``ontoenv`` subcommand and global flag.

:doc:`python`
   The ``OntoEnv`` class, grouped by purpose: lifecycle, ingestion, reading,
   aliases, configuration.

:doc:`configuration`
   Every setting, its default, and how to set it from the CLI or Python.

:doc:`rdflib-store`
   ``ViewGraph`` and ``OntoEnvStore`` — the read-only ``rdflib`` surfaces.

:doc:`graph-store`
   The protocol a custom ``graph_store=`` object must implement.

:doc:`api`
   Auto-generated signatures and docstrings for the whole Python module.

`Rust API <https://docs.rs/ontoenv>`__
   Crate documentation on docs.rs.

.. toctree::
   :hidden:
   :maxdepth: 1

   cli
   python
   configuration
   rdflib-store
   graph-store
   api
