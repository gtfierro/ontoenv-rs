# Snapshot backing comparison

Keep the existing backing policy and lifetime pins during consolidation. Mmap
provides useful multiprocess sharing; owned encoded bytes do not remove the
larger query-index allocations. The user explicitly clarified after seeing these
measurements that the rough 512 MB estimate is advisory. Do not turn it into a
release gate or add a memory manager. Defer the separate index-memory project.

## Method

Both isolated release builds use revision `f73c67b2370033e10ebc4a81f5e862db8f14d767`.
The owned build changes only `Snapshot::open_file` to use `R5tuFile::open`.
Both preserve strict integrity validation, lifetime pins, publication and query
code. The experimental selection exists only in an archived diagnostic source
copy; no product option or backing protocol changed.

Linux x86_64, rustc 1.99.1, Python 3.14.2. Two alternating repetitions of each
backing at 1/4/9 independent readers, using these 100-graph offline stores:

| Fixture | Source bytes | Encoded bytes | Unique triples |
| --- | ---: | ---: | ---: |
| Representative | 20,718,528 | 14,365,217 (13.70 MiB) | 333,008 |
| Upper workload | 129,182,428 | 96,248,298 (91.79 MiB) | 1,636,808 |

The corpus reuses Brick/QUDT/BACnet/SHACL fixtures and the building generator
from `bench_managed_ontology.py`. Each reader opens through `OntoEnv.open`, obtains
snapshot graph/closure views, runs count and schema-path queries, and retains
views and partially consumed iterators through two publications. Each publication
uses a fresh writer process. Old views check their original revision; old
iterators finish using their original dictionary. Releasing their last owners
allows startup/prune to remove the oldest object; closing all owners releases
every mapped RDF file. All 24 runs completed those assertions.

Memory is aggregate PSS sampled by the parent at synchronized barriers. Raw
records also include anonymous/file PSS, private clean/dirty pages, swap and
per-process RSS high-water marks. Summed RSS is not shared physical memory.
Unmapped OS file cache and the separate writer are outside the reader PSS total.
The writer exits before memory sampling. These are warm-cache diagnostics,
without an enforced memory limit, controlled cold-cache test or SLO claim.

## Aggregate reader PSS

Means of two runs, in MiB. "Queried" includes graph/closure queries and a live
iterator. Three versions means three distinct encoded objects per reader;
cloned views of one version share that version's backing and indexes.

| Encoded store | Readers | Backing | Opened | Queried | Three queried versions |
| --- | ---: | --- | ---: | ---: | ---: |
| 13.70 MiB | 1 | mmap | 53.6 | 140.4 | 307.5 |
| 13.70 MiB | 1 | owned | 52.3 | 139.0 | 306.2 |
| 13.70 MiB | 4 | mmap | 114.6 | 455.7 | 1,042.3 |
| 13.70 MiB | 4 | owned | 154.4 | 495.5 | 1,164.2 |
| 13.70 MiB | 9 | mmap | 215.7 | 980.8 | 2,266.4 |
| 13.70 MiB | 9 | owned | 324.1 | 1,089.0 | 2,593.7 |
| 91.79 MiB | 1 | mmap | 130.0 | 549.3 | 1,553.9 |
| 91.79 MiB | 1 | owned | 126.3 | 546.2 | 1,552.1 |
| 91.79 MiB | 4 | mmap | 189.3 | 1,861.0 | 5,334.2 |
| 91.79 MiB | 4 | owned | 462.7 | 2,135.0 | 6,158.5 |
| 91.79 MiB | 9 | mmap | 290.5 | 4,048.8 | 11,633.6 |
| 91.79 MiB | 9 | owned | 1,022.9 | 4,780.6 | 13,834.1 |

At the upper workload, nine mmap readers save about 732 MiB for one queried
version and 2,200 MiB for three versus owned buffers. One process has little
PSS difference because a singly mapped file is charged to that process.

Release was also checked without allocator trimming. For example, upper-workload
nine-reader mmap PSS fell from about 11,634 MiB to 8,345 MiB after dropping old
owners. A diagnostic `malloc_trim` then reduced it to about 4,061 MiB with the
current version still queried. Closing all owners and trimming returned it to
about 231 MiB. Thus both live query allocations and allocator retention matter;
the trim is diagnostic, not production behavior or a proposed solution.

## Timing

Single-reader medians of two runs, milliseconds. Closure construction and its
first query are measured after the graph query; do not read them as independent
cold-first-query timings.

| Store | Backing | Open | First graph query | Build closure | First closure query | Warm closure query |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 13.70 MiB | mmap | 31.3 | 173.5 | 151.3 | 20.5 | 16.5 |
| 13.70 MiB | owned | 38.8 | 171.9 | 149.4 | 20.6 | 16.4 |
| 91.79 MiB | mmap | 200.5 | 973.7 | 800.6 | 31.1 | 24.2 |
| 91.79 MiB | owned | 249.3 | 973.6 | 795.7 | 31.2 | 24.2 |

Warm refresh was roughly 31/39 ms at the representative size and 197/251 ms at
the upper size for mmap/owned. Results support retaining mmap where already used;
they do not justify replacing the existing query engine or storage format.

## Scope and continuation

No automatic memory limit, per-graph disk COW, pin removal, durability change or
new runtime mode was implemented. Windows retains its existing owned fallback;
this Linux test does not establish Windows working-set or power-loss guarantees.

Code inspection identifies scoped operations that construct whole-store indexes
and repeated temporary query indexes. Record that as a possible later targeted
optimization. The user's current priority is finishing state/API consolidation,
not achieving a particular RAM ceiling.

## Reproduction

```sh
python3 python/profile_snapshot_readers.py build \
  --work /tmp/ontoenv-backing-builds-20261025 \
  --maturin /path/to/maturin --base-python /path/to/python-with-rdflib \
  --target-dir /tmp/ontoenv-snapshot-review-target
python3 python/profile_snapshot_readers.py run \
  --builds /tmp/ontoenv-backing-builds-20261025/builds.json \
  --work /tmp/ontoenv-backing-matrix-20261025 \
  --nodes 350,3000 --readers 1,4,9 --repeats 2
```

Output roots must be new. The build command archives the committed revision;
it does not include uncommitted product edits. Both isolated environments reuse
identical dependencies from the supplied base interpreter, and verify their own
native extension path/hash. Raw records retain every fixture hash, exact patch,
build command, native hash/path, barrier sample and timing.

- `snapshot-backing-profile-2026-10-25.json.gz`: complete raw matrix (gunzip to JSON).
- `snapshot-backing-summary-2026-10-25.json`: rounded aggregates and timing medians.
- Local complete artifacts: `/tmp/ontoenv-backing-matrix-20261025/` and
  `/tmp/ontoenv-backing-builds-20261025/`.
