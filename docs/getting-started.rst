Getting Started
===============

This section gives you a minimal checklist to build the documentation locally and wire it into your workflows.

Install doc tooling with ``uv``
----------------------------------------

The docs have their own ``docs/pyproject.toml``. Create the env and install requirements with ``uv``:

.. code-block:: bash

   cd docs
   uv sync            # creates docs/.venv and installs Sphinx + Furo

If you prefer to reuse an existing environment, you can instead run ``uv pip install -r docs/requirements.txt``.

Build the docs locally
----------------------

.. code-block:: bash

   cd docs
   uv run sphinx-build -M html . _build
   open _build/html/index.html

Or just run the helper from the repository root (it syncs deps, builds the extension, and renders HTML):

.. code-block:: bash

   ./builddocs

Need a single text file for LLM ingestion? Use the alternate target:

.. code-block:: bash

   ./builddocs llms   # writes docs/_build/llms.txt

Python package in editable mode
-------------------------------

To build the extension module so ``autodoc`` can import ``ontoenv``:

.. code-block:: bash

   cd python
   uv run maturin develop

CLI binary
----------

You can install the Rust CLI from crates.io or build it from the workspace:

.. code-block:: bash

   cargo install ontoenv-cli           # pulls the published binary
   # or, from the workspace:
   cargo build -p ontoenv-cli --release

Publishing to GitHub Pages
--------------------------

- Build with ``uv run sphinx-build -M html . _build`` (or from CI).  
- Publish the contents of ``docs/_build/html`` to your Pages branch (e.g., ``gh-pages``) or configure Pages to read from ``docs`` if you use ``sphinx-build -M dirhtml``.  
- Remember to set the Pages source to ``/docs`` or the dedicated branch when you push to GitHub.
