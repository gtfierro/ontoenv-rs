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

How the docs should sound
-------------------------

Write for a reader who has a real ontology and wants to know what OntoEnv will
do with it. Start with the useful fact, command, or decision. Introduce the
underlying model once the reader has something concrete to attach it to.

Across all four sections:

- Prefer short paragraphs with one claim each.
- Use concrete verbs. Say that a command scans files, downloads an ontology,
  writes configuration, or deletes an environment.
- Show the result of an important command, not just the command itself.
- Introduce each command with the operation it performs. When a block contains
  several commands, say whether they are alternatives or a sequence, and
  describe the state changed by each step.
- State defaults, persistence, network access, mutation, and failure behavior
  when they could change a reader's decision.
- Keep examples internally consistent and small enough to understand in one
  sitting.
- Do not call a task simple or easy. Show the shortest reliable path instead.
- Distinguish guarantees from advice. Use "does" for behavior and "we
  recommend" for a judgment.
- Avoid slogans, superlatives, and unqualified comparisons. Replace claims
  such as "fast", "powerful", or "covers most uses" with a mechanism, a
  measurement, or nothing.
- Do not advertise OntoEnv to the reader. Describe the problem it addresses,
  its behavior, and its limits; let the reader decide whether it fits.

The four sections use the same plain language for different ends:

``tutorials/``
   Move through a working example. After each important step, show enough
   output for the reader to confirm that they are on track.

``how-to/``
   Put the canonical command or code first. Then cover verification and the
   failure cases a competent user is likely to meet.

``reference/``
   Use a predictable order: signature, behavior, arguments, return value,
   side effects, errors, example, and related operations. Omit headings that
   genuinely do not apply.

``explanation/``
   Name the design question, the constraints, the choice OntoEnv makes, and
   the consequences of that choice. Be direct about costs and limitations.
