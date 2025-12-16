Getting Started
===============

This section shows how to spin up an ``ontoenv`` workspace, filter what gets loaded, and keep cached ontologies fresh. The CLI, the Python package, and the Rust API now share the same vocabulary for configuration (``includes``, regex filters, cache modes, etc.), so you can jump between them without surprises.

Install the CLI or Python bindings
----------------------------------

.. code-block:: bash

   # CLI from crates.io
   cargo install ontoenv-cli

   # or from this workspace (after cloning the repo)
   cargo build -p ontoenv-cli --release

   # Python bindings (build the native module once)
   cd python
   uv run maturin develop

Initialize an ontology workspace
--------------------------------

``ontoenv`` stores its metadata under ``.ontoenv``. Discovery now happens only when you explicitly pass directories to ``ontoenv init``:

.. code-block:: console

   # Discover ontologies under the current directory
   ontoenv init .

   # Create an empty environment (no discovery yet)
   ontoenv init

   # Seed from multiple paths
   ontoenv init ./ontologies ./models

Running ``init`` again with ``--overwrite`` rebuilds the environment in place. Every command walks up from your current directory to find ``.ontoenv`` unless ``ONTOENV_DIR`` is set.

Add and refresh ontologies
--------------------------

Once the workspace exists you can add specific files or URLs, or re-run discovery:

.. code-block:: console

   # Add a single ontology without exploring its imports
   ontoenv add ./ontologies/site.ttl --no-imports

   # Refresh anything whose remote cache entry is stale
   ontoenv update
   ontoenv update --all              # force every ontology to re-download

Use ``ontoenv list ontologies`` or ``ontoenv dump`` to inspect what is currently cached.

Control discovery scope
-----------------------

Two layers of filters govern what gets pulled in:

* ``-i/--includes`` and ``-e/--excludes`` accept gitignore-style globs. Bare directories (``lib/tests``) automatically expand to ``lib/tests/**`` so you can target entire trees.
* ``--include-ontology`` / ``--exclude-ontology`` accept regex patterns that run against ontology IRIs after parsing. Includes act as a whitelist and excludes prune whatever slips through.

Example:

.. code-block:: console

   ontoenv init ontologies \
     --includes '*.ttl' \
     --exclude-ontology 'experimental' \
     --include-ontology '^https://example\\.com/'

Those settings are saved into ``.ontoenv/config.json`` so future commands inherit them. The ``ontoenv config`` helper currently supports ``locations``, ``includes``, and ``excludes`` via ``add``/``remove``; regex filters must be supplied on the command line or edited directly in the config file.

Cache strategy and TTL
----------------------

Remote ontologies live in a cache on disk. You can tune two independent knobs:

* ``use_cached_ontologies`` (exposed as ``use_cached_ontologies`` in Python or ``CacheMode`` in Rust) controls whether ``init`` eagerly scans the configured locations (disabled/default) or skips discovery until you explicitly add/update (enabled).
* ``--remote-cache-ttl-secs`` sets the maximum age of each cached remote ontology before ``update`` re-fetches it (default 86,400 seconds).

.. code-block:: console

   # Keep cached copies for a week before refreshing
   ontoenv update --remote-cache-ttl-secs 604800

   # Persist default settings in the config
   ontoenv config set remote_cache_ttl_secs 604800
   ontoenv config add locations ./ontologies

Python quickstart
-----------------

The Python API mirrors the same configuration surface:

.. code-block:: python

   from ontoenv import OntoEnv

   env = OntoEnv(
       path=".",                    # use ./ .ontoenv if it exists
       recreate=True,               # create/overwrite metadata
       search_directories=["."],    # scan the current project
       includes=["*.ttl", "*.xml"],
       include_ontologies=[r"^https://example\.com/"],
       exclude_ontologies=[r"experimental"],
       offline=True,
       use_cached_ontologies=False,
       remote_cache_ttl_secs=86400,
   )

   env.update(all=True)

Pass ``use_cached_ontologies=True`` to start with an empty container that only fills when you explicitly call ``add``/``update``.

Working on this documentation
-----------------------------

Documentation lives under ``docs/`` with its own ``pyproject.toml``. To build or preview the site:

.. code-block:: bash

   cd docs
   uv sync                 # installs Sphinx + theme into docs/.venv
   uv run sphinx-build -M html . _build
   open _build/html/index.html

The repo also ships helper scripts:

.. code-block:: bash

   ./builddocs          # sync deps, build the extension module, render HTML
   ./builddocs llms     # render docs/_build/llms.txt for LLM ingestion

If ``autodoc`` needs the Python bindings, rebuild them in editable mode:

.. code-block:: bash

   cd python
   uv run maturin develop

Publishing to GitHub Pages still follows the same steps: build HTML, push ``docs/_build/html`` (or configure Pages to use ``/docs``), and update the Pages source accordingly.
