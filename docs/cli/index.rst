CLI Usage
=========

The ``ontoenv`` CLI wraps the Rust core with commands for discovering and materializing ontology imports.

Install
-------

.. code-block:: bash

   cargo install ontoenv-cli
   # or from the workspace:
   cargo build -p ontoenv-cli --release
   ./target/release/ontoenv --help

Common tasks
------------

- ``ontoenv init`` — create a new ontology environment in the current directory.
- ``ontoenv add <path-or-iri>`` — register an ontology; optionally fetch its imports.
- ``ontoenv update`` — refresh cached ontologies.
- ``ontoenv dump`` — export the current environment for inspection.

Reference
---------

Start by embedding the CLI help output as the reference page. Replace this block once you settle on a stable command set:

.. code-block:: text

   $ ontoenv --help
   (paste the generated help text here)

