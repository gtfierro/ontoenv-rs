# rdf5d Optimization Plan

This document turns the current benchmark and code review findings into a development checklist for improving rdf5d performance and space usage.

## Baseline

Measured on 2026-03-10 with:

```sh
cargo bench --bench rdf5d_bench --features zstd
```

Current synthetic benchmark snapshot:

- `write/10000`: 16.7-17.0 ms, about 590 K triples/s
- `write_zstd/10000`: 17.8-18.1 ms, about 553-563 K triples/s
- `write_streaming/10000`: 17.0-17.2 ms, about 581-587 K triples/s
- `open/10000`: 10.4-10.5 ms
- `read_triples/10000`: 101.7-103.2 us, about 97-98 M triples/s
- `graph_lookup/enumerate_by_id/100`: 13.9-14.1 us
- `graph_lookup/enumerate_by_graphname/100`: 49.4-50.4 us
- `roundtrip/10000`: 59.0-59.8 ms, about 502-508 K triples/s

Observed constraints from the current implementation:

- Both batch and "streaming" writers retain all groups and sort at finalize time.
- `open()` reads the whole file and recomputes CRCs eagerly.
- Triple iteration eagerly decodes each graph block into multiple `Vec<u64>` buffers.
- String dictionaries use a relatively large 24-byte coarse index entry.
- Term dictionary offsets are always 64-bit in the current implementation.

## Progress Notes

### 2026-03-10

- Implemented shared reader open/validation logic so owned and mmap-backed readers no longer maintain separate validation code paths.
- Added `IntegrityMode` and `OpenOptions` so callers can choose between strict, structural, and trusted open behavior explicitly.
- Added `R5tuFile::open_with_options` and `R5tuFile::open_mmap_with_options`.
- Added `prefer_mmap` handling in open options and an `is_mmap_backed()` helper for test coverage.
- Removed one avoidable writer-side reparse of freshly built triple blocks by returning counts directly from `build_raw_spo`.
- Replaced eager full materialization of `O` values in triple blocks with a lazy iterator that decodes object values incrementally from the encoded payload.
- Implemented width-aware `TERM_DICT` offsets:
  - default writer path now emits `u32` offsets when the term payload fits
  - reader now honors `width=4` and `width=8`
  - `ARCH.md` updated to document the active width field semantics
- Compacted the string-dictionary coarse index from 24-byte entries to 20-byte entries in newly written files while keeping reader compatibility with legacy 24-byte entries.
- Added compact metadata layouts for newly written files when values fit:
  - `GDIR` rows now use 32 bytes instead of 44 bytes
  - `IDX_PAIR2GID` entries now use 12 bytes instead of 16 bytes
  - reader remains compatible with legacy 44-byte rows and 16-byte pair entries
- Applied low-risk writer hot-path cleanup:
  - switched `id` and `gname` interning from `BTreeMap` to `HashMap`
  - pre-sized batch writer vectors/maps from input size
  - added `StreamingWriter::with_capacity` for callers that know approximate input size
- Added tests covering:
  - strict CRC mismatch rejection
  - structural open ignoring footer CRC mismatch
  - mmap-preferred open path
- Verified with:
  - `cargo test`
  - `cargo test --features mmap,zstd,oxigraph`
- Added benchmark cases for:
  - structural open
  - trusted open
  - mmap structural open
- Focused open-path benchmark notes from `cargo bench --bench rdf5d_bench --features mmap,zstd`:
  - strict `open/10000`: about `10.51-10.60 ms`
  - `open/structural/10000`: about `14.82-15.11 us`
  - `open/trusted/10000`: about `14.35-14.60 us`
  - `open/mmap_structural/10000`: about `3.88-3.94 us`
- Focused read-iteration benchmark notes from `cargo bench --bench rdf5d_bench --features mmap,zstd read_triples` after the lazy object decode change:
  - `read_triples/100`: about `1.34-1.36 us`
  - `read_triples/1000`: about `12.87-13.07 us`
  - `read_triples/10000`: about `139.48-141.35 us`
- Focused first-triple benchmark notes from `cargo bench --bench rdf5d_bench --features mmap,zstd first_triple`:
  - `first_triple/100`: about `865.60-877.07 ns`
  - `first_triple/1000`: about `8.06-8.19 us`
  - `first_triple/10000`: about `80.22-81.28 us`
- Focused graph-lookup notes from `cargo bench --bench rdf5d_bench --features mmap,zstd graph_lookup` after compacting the string index:
  - `enumerate_by_id/20`: about `2.38-2.41 us`
  - `enumerate_by_graphname/20`: about `3.90-4.02 us`
  - `enumerate_by_id/100`: about `13.18-13.42 us`
  - `enumerate_by_graphname/100`: about `54.26-54.95 us`
- Compact string-index entries reduce coarse-index bytes by 4 bytes per dictionary entry, a 16.7% reduction for that section without changing lookup semantics.
- Focused graph-lookup notes from `cargo bench --bench rdf5d_bench --features mmap,zstd graph_lookup` after compacting `GDIR` and pair entries:
  - `enumerate_by_id/20`: about `2.68-2.71 us`
  - `enumerate_by_graphname/20`: about `3.97-4.03 us`
  - `enumerate_by_id/100`: about `13.92-14.01 us`
  - `enumerate_by_graphname/100`: about `49.95-50.35 us`
- Focused metadata-path notes after adding dedicated benchmarks:
  - `resolve_gid/20`: about `7.44-8.32 us`
  - `resolve_gid/100`: about `113.84-128.55 us`
  - `enumerate_all/20`: about `2.24-2.41 us`
  - `enumerate_all/100`: about `13.27-15.04 us`
  - `enumerate_all/500`: about `88.61-102.12 us`
- Focused write-path notes from targeted 10k-triple benchmarks after the 4.2 intern/preallocation cleanup:
  - `write/10000`: about `15.60-16.34 ms`
  - `write_streaming/10000`: about `19.77-20.75 ms`
  - `write_zstd/10000`: about `17.26-17.52 ms`
- Compact `GDIR` rows reduce that section by 12 bytes per row, a 27.3% reduction versus the legacy 44-byte row.
- Compact pair-index entries reduce that section by 4 bytes per pair, a 25% reduction versus the legacy 16-byte entry.
- The synthetic benchmark shows the intended reader-side improvement clearly for non-strict open paths, but the full suite was noisy for unrelated write benchmarks and should not yet be used as a stable throughput comparison for write-path work.

## Goals

- Improve open latency, especially for trusted local files and mmap-backed reads.
- Reduce peak memory usage during read and write paths.
- Reduce file size overhead from dictionaries and metadata.
- Preserve deterministic output and current correctness guarantees unless explicitly relaxed by configuration.
- Add representative benchmarks so optimization work is guided by real workloads, not just the current synthetic single-graph microbenchmarks.

## Phase 0: Benchmarking And Visibility

- [ ] Expand the benchmark suite beyond the current synthetic cases.
- [ ] Add benchmarks for:
  - many small graphs
  - one very large graph
  - repeated literals with shared datatype/lang
  - high-cardinality graph names and ids
  - mmap open path
  - trusted open path with integrity checks disabled
- [ ] Add a corpus benchmark target using a small checked-in or generated representative dataset.
- [ ] Add a size-breakdown tool or benchmark output that reports section sizes per file:
  - `TERM_DICT`
  - `ID_DICT`
  - `GNAME_DICT`
  - `GDIR`
  - postings indexes
  - pair index
  - triple blocks
- [ ] Record pre-change benchmark baselines in this file or a sibling benchmark artifact.

Definition of done:

- We can attribute both runtime and bytes-on-disk to specific sections and workloads.

## Phase 1: Low-Risk Reader Wins

### 1.1 Mmap-first open path

- [x] Add benchmarks comparing `R5tuFile::open` and `R5tuFile::open_mmap`.
- [x] Introduce a reader API that prefers mmap when the `mmap` feature is enabled.
- [x] Avoid duplicated open/validation logic between owned and mmap-backed paths.
- [x] Keep one shared validation path so correctness changes do not fork.

Success criteria:

- Open latency drops materially for larger files.
- No behavior regression in existing tests.

Notes:

- The benchmark harness now includes `structural`, `trusted`, and `mmap_structural` open cases.
- Stable synthetic open-path numbers are now recorded in the progress notes above.
- The main follow-up question is policy, not mechanism: whether the default public API should eventually prefer mmap automatically.

### 1.2 Configurable integrity verification

- [x] Add reader options for integrity mode:
  - strict: section CRCs + global CRC
  - default: structural validation only, CRC optional
  - trusted: minimal validation for local trusted files
- [x] Make integrity policy explicit in the API instead of baking eager CRC into `open()`.
- [ ] Benchmark open times across integrity modes.
- [x] Preserve a strict mode for tests and high-integrity use cases.

Success criteria:

- Trusted or default open avoids paying full-file CRC cost when not needed.

Notes:

- `R5tuFile::open()` remains strict for backward compatibility.
- `R5tuFile::open_with_options()` now exposes the integrity policy explicitly.
- Structural mode currently skips CRC verification but still performs structural validation and required-section parsing.
- Trusted mode currently applies the lightest validation path while still validating section bounds and required sections.
- On the synthetic 10k-triple benchmark, structural/trusted open is roughly three orders of magnitude faster than strict open because it avoids CRC work and full owned reads.

### 1.3 Lazy triple decoding

- [x] Replace eager full-block decode with a streaming iterator over the encoded payload.
- [ ] For zstd blocks, evaluate two options:
  - [x] fully decompress but lazily decode varints
  - stream-decompress and decode in one pass
- [ ] Keep the current eager path available temporarily for A/B testing if helpful.
- [ ] Benchmark:
  - [x] time to first triple
  - [x] total iteration throughput
  - [x] peak allocation count and bytes

Success criteria:

- Lower peak memory per graph read.
- Equal or better full iteration throughput.
- Better latency to first triple.

Notes:

- The current implementation keeps `S` and `P` structure eagerly decoded but no longer materializes `O` values into a `Vec<u64>`.
- Raw blocks borrow the original payload; zstd blocks still decompress into an owned buffer, but object values are decoded lazily from that buffer.
- This is a meaningful peak-memory reduction for large graphs because `O` is typically the largest decoded vector.
- Allocation-focused coverage now exists via `tests/decode_alloc.rs`, which asserts that creating an iterator for a 10k-object raw block stays under a small allocation budget instead of materializing an `O` vector.
- The remaining open question in this phase is whether zstd should eventually support true streaming decompression rather than decode-then-iterate.

## Phase 2: Dictionary Space Reduction

### 2.1 String dictionary redesign

- [ ] Measure actual contribution of `ID_DICT` and `GNAME_DICT` coarse indexes to total file size.
- [x] Prototype alternatives to the current 24-byte `key16` index:
  - [x] compact fixed-width `key16 + id` entries
  - [ ] sorted string table plus sparse restart points
  - [ ] front-coded blocks
  - [ ] mmap-friendly FST for `string -> id`
- [x] Benchmark lookup latency for:
  - [x] `enumerate_by_id`
  - [x] `enumerate_by_graphname`
  - [ ] pair resolution
- [x] Prefer a design that shrinks bytes without making graph-name lookup worse.

Decision gate:

- If graph-name lookup remains slower than id lookup after compaction, revisit lookup-specific indexing rather than compressing blindly.

Notes:

- Newly written files now use 20-byte index entries for `ID_DICT` and `GNAME_DICT`.
- The reader accepts both the new 20-byte entries and legacy 24-byte entries.
- On the current synthetic lookup benchmark, this compaction did not regress lookup performance; most cases improved modestly, while `enumerate_by_graphname/100` stayed within noise.
- This is a compatible compaction step, not the final string-dictionary design. FST/front-coding work remains open.

### 2.2 Term dictionary width and layout

- [x] Implement width-aware term offset storage:
  - `u32` offsets for smaller files
  - `u64` offsets only when required
- [x] Make the reserved `width` field in `TERM_DICT` meaningful.
- [x] Add compatibility tests for both widths.
- [ ] Measure byte savings on representative corpora.

Expected impact:

- Large win when term count is high and total term payload still fits under 4 GiB.

Notes:

- The default writer now emits width `4` when the term payload fits in `u32`, which should halve the size of the term-offset table for the common case.
- Coverage now includes:
  - an integration test that default files emit width `4`
  - a reader unit test that decodes a manually encoded width `8` term dictionary
- Corpus-level byte-savings measurement is still pending.

### 2.3 Literal component interning

- [ ] Split literal storage into reusable components where beneficial:
  - lexical form dictionary
  - datatype dictionary
  - language dictionary
- [ ] Prototype a term encoding that references datatype/lang IDs instead of embedding repeated strings inline.
- [ ] Compare against current inline literal encoding on corpora with repeated XSD datatypes and language tags.
- [ ] Ensure decoding remains mmap-friendly.

Decision gate:

- Only adopt if space savings are clear and decode cost does not materially regress.

## Phase 3: Index And Metadata Compaction

### 3.1 GDIR and pair index width reduction

- [ ] Audit realistic bounds for:
  - `gid`
  - `id_id`
  - `gn_id`
  - section-relative offsets
- [x] Introduce width-aware row formats for `GDIR` and `IDX_PAIR2GID`.
- [ ] Prefer section-relative offsets where that simplifies compaction.
- [x] Benchmark impact on:
  - [x] file size
  - [x] `resolve_gid`
  - [x] `enumerate_all`

Notes:

- Newly written files use compact 32-byte `GDIR` rows when offsets, lengths, and counts fit in `u32`.
- Newly written files use compact 12-byte pair-index entries when `gid` fits in `u32`.
- Reader compatibility is preserved by decoding both compact and legacy layouts based on `row_size` and pair-section stride.
- On the current synthetic `graph_lookup` benchmark, the metadata compaction did not introduce a broad lookup regression; most cases improved or stayed close to prior measurements, while `enumerate_by_id/20` regressed modestly in one run and should be treated as noise until rechecked on a more stable benchmark setup.
- Practical bounds assumption for the current compact layout:
  - compact rows/entries are used only when file offsets, block lengths, triple counts, and `gid` fit in `u32`
  - when any of those exceed `u32`, the writer falls back to the legacy wider layout
- The section-relative-offset idea is still open. The current implementation keeps absolute offsets and only compacts integer width.

### 3.2 Postings representation upgrade

- [ ] Measure postings size and decode cost on realistic many-graph corpora.
- [ ] Prototype alternatives:
  - Elias-Fano
  - roaring-style containers if fanout patterns justify them
  - keep delta-varint for very small lists
- [ ] Consider hybrid encoding by posting-list cardinality.
- [ ] Benchmark lookup and intersection workloads if future query APIs need them.

Decision gate:

- Do not replace delta-varints globally unless real datasets show clear wins.

## Phase 4: Writer Architecture

### 4.1 True streaming/external build

- [ ] Redesign `StreamingWriter` so it does not retain all triples in memory.
- [ ] Choose an external build strategy:
  - chunked in-memory sorts + merge
  - append per-graph temporary runs then merge
  - two-pass build if required for dictionaries
- [ ] Define memory budget targets for large ingest.
- [ ] Preserve deterministic output ordering.
- [ ] Benchmark:
  - peak RSS
  - build throughput
  - temp disk usage

Success criteria:

- `StreamingWriter` becomes meaningfully lower-memory than batch build.

### 4.2 Writer hot-path cleanup

- [ ] Remove avoidable cloning during interning where possible.
- [x] Revisit `BTreeMap` vs `HashMap` choices for intern tables and group assembly.
- [x] Pre-size buffers from input statistics when available.
- [x] Eliminate duplicate work such as reparsing raw counts after `build_raw_spo` if those counts can be returned directly.
- [x] Benchmark plain and zstd write paths after each change.

Expected impact:

- Moderate throughput win with relatively low format risk.

Notes:

- `build_raw_spo` now returns both the encoded payload and the already-known `(n_s, n_p, n_t)` counts, so the writer no longer reparses the varint header for each graph block during file assembly.
- Batch writer interning for `id` and `gname` no longer pays ordered-map costs; deterministic output is preserved because ID assignment order still comes from first-seen insertion into the backing vectors.
- The targeted 10k-triple benchmarks show a clear improvement for `write` and `write_zstd`, while `write_streaming` stayed roughly flat. That makes sense because `StreamingWriter` still retains all groups and is still dominated by the same finalize/sort pipeline.

## Phase 5: Format-Level Experiments

These are higher-risk and should only start after the lower-risk items above are measured.

- [ ] Evaluate alternate triple block encodings beyond current CSR-like SPO:
  - section-local term ID remapping
  - hybrid block-local dictionaries
  - optional additional permutations only if query workload needs them
- [ ] Measure whether block-local remapping improves both raw size and zstd compression ratio.
- [ ] Prototype per-block encoding selection based on graph characteristics.
- [ ] Keep migration/versioning implications explicit in the design notes.

Decision gate:

- Require clear wins on both corpus size and representative read/write workloads before changing the on-disk version.

## Cross-Cutting Requirements

- [x] Keep all current tests passing.
- [x] Add new tests for any width-aware or versioned encoding changes.
- [x] Add benchmark assertions or at least stable result capture for regressions.
- [x] Update `ARCH.md` whenever an adopted plan changes the format or reserved fields become active.
- [ ] Prefer feature flags or versioned readers for risky changes rather than silent format drift.

## Suggested Execution Order

- [ ] Phase 0: benchmark expansion and section-size visibility
- [x] Phase 1.1: mmap-first open path
- [x] Phase 1.2: configurable integrity verification
- [x] Phase 1.3: lazy triple decoding
- [x] Phase 2.2: width-aware term offsets
- [ ] Phase 2.1: string dictionary redesign
- [ ] Phase 3.1: compact `GDIR` and pair index
- [ ] Phase 4.2: writer hot-path cleanup
- [ ] Phase 4.1: true streaming writer
- [ ] Phase 2.3 and Phase 3.2 after new corpus benchmarks exist
- [ ] Phase 5 only after earlier phases plateau

## Notes

- The current synthetic benchmark suite understates some likely real-world costs because it uses small generated dictionaries and a simple graph layout.
- Previous corpus measurements in `benchmark_results.csv` suggest rdf5d is already competitive on size and load time against Turtle/RDFLib for larger files, so the biggest practical wins are likely in metadata overhead, open latency, and memory behavior rather than raw triple iteration throughput.
