Building from source
====================

Building the crates
-------------------

Rust 1.88 or newer is required.

.. code-block:: bash

   git clone https://github.com/gtfierro/ontoenv-rs
   cd ontoenv-rs

   cargo build -p ontoenv-cli --release
   ./target/release/ontoenv --help

   cargo test

The workspace holds four crates:

``lib``
   ``ontoenv`` — the core environment, import resolution, and dependency graph.

``cli``
   ``ontoenv-cli`` — the command-line front end.

``python``
   The PyO3 bindings published to PyPI as ``ontoenv``.

``rdf5d``
   The on-disk RDF format used for graph storage and the metadata catalog.

Building the Python bindings
----------------------------

Python 3.11 or newer.

.. code-block:: bash

   cd python
   uv run maturin develop

   uv run pytest

Building the docs
-----------------

The documentation has its own ``pyproject.toml`` under ``docs/`` so its
tooling stays separate from the project's.

.. code-block:: bash

   ./builddocs          # sync deps, build the extension, render HTML
   ./builddocs llms     # also render docs/_build/llms.txt for LLM ingestion

Or by hand:

.. code-block:: bash

   cd docs
   uv sync
   uv run sphinx-build -M html . _build
   open _build/html/index.html

How the docs are organized
--------------------------

The structure follows `Diátaxis <https://diataxis.fr>`_. When adding a page,
put it in the section matching what the reader is doing:

``tutorials/``
   Learning. A guaranteed-to-work path through a task, for someone who has
   not used OntoEnv before. No alternatives, no caveats, no edge cases.

``how-to/``
   Working. One page per goal, assuming competence. Terse; link out for
   background rather than explaining inline.

``reference/``
   Looking things up. Complete, dry, and organized by the shape of the API
   rather than by task. Tables over prose.

``explanation/``
   Understanding. Why things are the way they are. This is where design
   rationale, trade-offs, and edge cases belong — keep them out of the other
   three.

The most common mistake is letting explanation leak into a how-to, or
reference detail into a tutorial. If a paragraph starts with "note that" or
"the reason for this is", it probably belongs in ``explanation/``.
