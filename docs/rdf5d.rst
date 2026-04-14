RDF5D (R5TU) Storage Format
===========================

.. raw:: html

   <div class="oe-section-intro">
     <strong>RDF5D (R5TU)</strong> is a compact, immutable binary format for storing collections
     of RDF graphs. It is designed for fast, zero-copy loading from disk via memory mapping,
     enabling OntoEnv to restore an entire ontology workspace in milliseconds.
   </div>

Overview
--------

The RDF5D format (internally referred to as **R5TU**) is an HDT-inspired serialization
optimized for **5-tuples**:

.. code-block:: text

    ┌───────────────────────────────────────────────────────────┐
    │                        5-Tuple                            │
    ├──────────┬───────────┬───────────┬───────────┬────────────┤
    │    ID    │  Subject  │ Predicate │   Object  │ Graph Name │
    └────┬─────┴─────┬─────┴─────┬─────┴─────┬─────┴─────┬──────┘
         │           │           │           │           │
      "src/A"    <alice>     foaf:name    "Alice"     <graph1>

Where:
*   **id**: A string identifying the source or dataset (e.g., a file path).
*   **graphname**: The IRI of the RDF graph (e.g., from an ``owl:Ontology`` declaration).
*   **s, p, o**: The standard RDF triple components.

Design Goals
------------

*   **Fast enumeration**: Quickly list all graphs associated with a specific ``id`` or ``graphname``.
*   **Zero-copy reads**: Use ``mmap`` to access data directly from the OS page cache without parsing.
*   **Compactness**: Employs a global term dictionary and delta-encoded triple blocks to minimize disk footprint.
*   **Concurrency**: Designed for many-readers / one-writer access patterns with atomic finalization.

File Architecture
-----------------

The file structure is designed for efficient seeking. A Table of Contents (TOC) at the start
of the file points to specialized data sections.

.. mermaid::

    graph TD
        Header[Header: Magic, Version, TOC Offset]
        TOC[TOC: Section Offsets & Lengths]
        Header --> TOC
        TOC --> TERM_DICT[Global Term Dictionary]
        TOC --> ID_DICT[Source ID Dictionary]
        TOC --> GNAME_DICT[Graph Name Dictionary]
        TOC --> GDIR[Graph Directory]
        TOC --> TRIPLE_BLOCKS[Compressed Triple Data]
        TOC --> INDEXES[Lookup Indexes]

Triple Encoding
---------------

Triples are grouped by graph and sorted by (S, P, O) order. The structure follows a 
**Compressed Sparse Row (CSR)** approach to maximize sharing of subjects and predicates:

.. code-block:: text

    Triples: (s1, p1, o1), (s1, p1, o2), (s1, p2, o3), (s2, p3, o4)

    ┌─────────┐      ┌─────────┐      ┌─────────┐
    │ S-vals  │      │ P-vals  │      │ O-vals  │
    ├─────────┤      ├─────────┤      ├─────────┤
    │   s1    ├─────►│   p1    ├─────►│   o1    │
    ├─────────┤      │         │      ├─────────┤
    │   s2    ├─┐    ├─────────┤      │   o2    │
    └─────────┘ │    │   p2    ├─┐    ├─────────┤
                │    └─────────┘ │    │   o3    │
                │                │    ├─────────┤
                │    ┌─────────┐ │    │   o4    │
                └───►│   p3    ├─┘    └─────────┘
                     └─────────┘

    (S-heads and P-heads store the jump offsets between these arrays)

This structure allows for extremely efficient streaming of triples and can be
optionally compressed using **zstd** frames on a per-graph basis.

Logical Mapping
---------------

To keep the file small, strings are only stored once in the **Global Dictionaries**. 
The **Triple Blocks** store only integer **TermIDs**.

.. mermaid::

    sequenceDiagram
        participant App
        participant TripleBlock
        participant TermDict
        App->>TripleBlock: Read GID
        TripleBlock-->>App: Return (S_id, P_id, O_id)
        App->>TermDict: Resolve S_id
        TermDict-->>App: Return "<alice>"
        App->>TermDict: Resolve P_id
        TermDict-->>App: Return "foaf:name"
        App->>TermDict: Resolve O_id
        TermDict-->>App: Return "Alice"

Advanced Usage
--------------

Rust (Core Library)
~~~~~~~~~~~~~~~~~~~

The ``rdf5d`` crate supports both batch and streaming writes.

**Batch writing with compression:**

.. code-block:: rust

    use rdf5d::{write_file_with_options, Quint, Term, WriterOptions};

    let quads = vec![
        Quint {
            id: "dataset:1".into(),
            s: Term::Iri("http://example.org/Alice".into()),
            p: Term::Iri("http://xmlns.com/foaf/0.1/name".into()),
            o: Term::Literal { lex: "Alice".into(), dt: None, lang: None },
            gname: "http://example.org/graph".into(),
        },
        // ... more quints
    ];

    write_file_with_options(
        "data.r5tu",
        &quads,
        WriterOptions { zstd: true, with_crc: true }
    ).expect("Successful write");

**Streaming large datasets:**

The ``StreamingWriter`` allows you to build a file quad-by-quad without holding the
entire dataset in memory.

.. code-block:: rust

    use rdf5d::{StreamingWriter, Quint, Term, WriterOptions};

    let mut writer = StreamingWriter::new(
        "large_data.r5tu",
        WriterOptions { zstd: true, with_crc: true }
    );

    // Add millions of quads incrementally
    for i in 0..1_000_000 {
        writer.add(Quint {
            id: format!("src/{}", i % 10),
            s: Term::Iri(format!("http://ex/s{}", i)),
            p: Term::Iri("http://ex/p".into()),
            o: Term::Iri(format!("http://ex/o{}", i)),
            gname: "default".into(),
        })?;
    }

    writer.finalize()?; // Builds dictionaries and indexes at the end

Python Integration
~~~~~~~~~~~~~~~~~~

In the ``ontoenv`` Python package, RDF5D is used as the primary persistence layer. 
OntoEnv automatically manages the file structure, but you can interact with it 
transparently through the library.

.. code-block:: python

    from ontoenv import OntoEnv
    from rdflib import URIRef, Literal

    # Create an environment that persists to .ontoenv/
    env = OntoEnv(path=".")

    # When you add a graph, it is serialized into the .r5tu store
    env.add("https://brickschema.org/schema/1.4/Brick.ttl")

    # You can later retrieve graphs directly from the store
    g = env.get_graph("https://brickschema.org/schema/1.4/Brick")
    
    # Or find all sources associated with an ontology
    sources = env.list_locations()

CLI Tool (r5tu)
~~~~~~~~~~~~~~~

If the ``rdf5d`` crate is built with the ``oxigraph`` feature, the ``r5tu`` binary
can be used for powerful dataset management.

.. code-block:: bash

    # Import multiple files into a single optimized R5TU file
    r5tu build-graph \
        --input schema_v1.ttl \
        --input schema_v2.ttl \
        --output schemas.r5tu \
        --graphname http://example.org/schema

    # Convert a large N-Quads dataset to R5TU with compression
    r5tu build-dataset \
        --input large_dump.nq \
        --output archive.r5tu \
        --zstd

    # Inspect the contents and structure of a file
    r5tu stat --file archive.r5tu

    # Example output:
    # Graphs: 1,245
    # Triples: 45,892,103
    # Terms: 5,601,234
    # Compression: zstd
    # CRC: Valid
