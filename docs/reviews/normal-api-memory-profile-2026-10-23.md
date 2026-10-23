# Initial import memory profile

The initial import's largest live allocation is the managed update's **staged
OxRDF graphs**, which build six indexes before RDF5D serialization begins.
At the measured heap peak, staged graphs and the serializer's data coexist.
Reducing parser concurrency or allocator arenas helps RSS but does not bring
this workload under the provisional 256 MiB writer target.

Recommendation: replace eager indexed staging with a compact triple representation,
materializing an indexed graph only when a caller needs a staged graph read.
Profile that change before accepting a lower release memory budget. Production
allocation/staging behavior has not been changed by this profiling work.

## Workload and method

This repeats the prior mixed Brick/QUDT/BACnet/REC/SHACL plus generated building
corpus: 100 graphs, 19.76 MiB Turtle and approximately 13.8 MiB RDF5D. There are
**334,059 quads across named graphs**, or **333,008 unique triples in their union**.
These are different counts because some vocabularies share triples; no source
graphs were omitted. The retained source directory has building 0's revision 2
from the earlier workload, with the same shape and counts.

The Rust worker calls the normal `OntoEnv::connect` and `update` APIs, with offline,
non-strict imports, and creates no reader or query view. The Python worker does
the same through the previously rebuilt release wheel. Production library code
is unchanged between the profiled revision `102fdfa` and current `7265b71`; the
latter changes a Python path assertion only.

Tools: rustc 1.99.1, Linux x86_64/glibc, Valgrind 3.22.0 Massif. Native and Python
RSS measurements run without Valgrind. There are three fresh sequential native
processes per mode and three fresh Python processes. Timings use warm OS caches
and are diagnostic ranges, not a controlled latency comparison.

Massif measures live allocations by stack, with depth 40 and a 0.5% reporting
threshold. Its allocator and execution overhead change timing and RSS, so only
its allocation tree is used for live heap attribution. The complete successful
profile is retained; an earlier exploratory run stopped after import on a
quad-versus-union-count assertion and is excluded from the conclusions below.

## Live heap peak

Massif measured **456.1 MiB useful live heap**, plus **10.8 MiB allocation
overhead**, during publication:

| Allocation owner at that peak | MiB | Share of useful heap |
| --- | ---: | ---: |
| Staged OxRDF graphs from `ParsedOntology::to_graph` | 342.7 | 75.1% |
| RDF5D publication through `write_object` | 99.0 | 21.7% |
| Other / allocations below the reporting threshold | 14.4 | 3.2% |

Within staged graphs, **327.2 MiB is attributed to B-tree index nodes**. OxRDF
0.3.3's `Graph` wraps a `Dataset`, which maintains six index permutations.
`SnapshotGraphIO::put` eagerly calls `parsed.to_graph()` and retains those graphs
in its private overlay. During publication, `publish_environment` and
`publish_state` clone their `Arc` handles; they share the staged contents rather
than making another full graph copy. The original indexed graphs nevertheless
remain live while the serializer builds its own structures.

The RDF5D share includes term interning, its term map/vector and string
dictionary/output buffers. A parser result wave is visible earlier in the
profile, but it is not the dominant owner at the overall peak. No independently
opened reader or lazy query index exists during this import-only experiment.

Relevant repository paths:

- `lib/src/io.rs`: `ParsedOntology::to_graph`
- `lib/src/snapshot_io.rs`: `PendingGraph`, `put`, `publish_environment`
- `lib/src/snapshots.rs`: `SnapshotUpdate::commit_with_publisher`, `write_object`
- `rdf5d/src/writer.rs`: `intern_term_owned`, `finalize_with_stats`, `write_term_dict`

## RSS and allocator retention

| Fresh process mode | Initial import peak RSS | RSS after import | RSS after diagnostic trim with environment still open |
| --- | ---: | ---: | ---: |
| Rust, default 8 parser workers | 534–544 MiB | 486–497 MiB | 40–49 MiB |
| Rust, `ONTOENV_PARALLELISM=1` | 491–498 MiB | 443–450 MiB | 25–35 MiB |
| Rust, `MALLOC_ARENA_MAX=1` | 466–467 MiB | 418–421 MiB | 36–38 MiB |
| Python, default parser workers | 570–572 MiB | 523–525 MiB | 71–79 MiB |

The Python reproduction agrees with the earlier normal API initial-load peak.
Its worker creates no retained old views, so the initial peak is independent of
the retained-version experiments from the earlier review.

A separate native run's glibc statistics immediately after import reported about
**482 MiB of allocator-owned system space but only 0.85 MiB in use**. File-backed
RDF5D mmap bytes are outside those heap totals. This shows why resident memory
can remain high after the temporary staging/serialization data has been freed.
Dropping the environment alone did not return most allocator-reserved pages;
the diagnostic trim did.

`malloc_trim` is called only by these diagnostics. It does not reduce the peak
already incurred during import and is not a portable production solution or a
reason to advertise a 71 MiB memory requirement. The allocation tree establishes
a real transient staging cost; the trim experiment explains much of the later
RSS retention. These single-import results do not establish the behavior of
arbitrary repeated updates or every allocator/platform.

Native update times were approximately 0.95–1.27 s by default, 1.23–1.45 s with
one parser worker, and 1.31–1.46 s with one arena. Python runs took 1.09–1.19 s.
The controls suggest allocator/parser settings alone are insufficient; they are
not proposed as universal tuning defaults, especially for network-heavy imports.

## Next implementation to measure

1. Retain parsed triples in compact shared staging storage. Preserve set/dedup
   semantics and blank-node identity; indexed staging should be lazy or avoided
   for the ordinary import-to-publication path.
2. Feed that representation into the existing complete-store serializer without
   constructing an indexed `oxrdf::Graph` for every staged replacement.
3. Keep nested rollback, empty replacements, staged graph reads, aliases and
   strict import handling correct. Existing ownership and rollback tests remain
   acceptance checks.
4. Rerun this corpus with native and Python entry points and the live-heap
   profiler. Do not project a new RSS number simply by subtracting the current
   graph indexes; the replacement representation will have its own cost.

This targets the measured 75% owner first. Per-graph disk copy-on-write and
snapshot index sharing address different costs and are not justified as the
first fix by this profile.

## Reproduction and artifacts

```sh
cargo build -p ontoenv --release --example managed_import_profile
target/release/examples/managed_import_profile NEW_ROOT SOURCE_DIRECTORY --allocator-stats --trim-live

# Repeat in separate fresh processes for the two controls:
ONTOENV_PARALLELISM=1 target/release/examples/managed_import_profile NEW_ROOT SOURCE_DIRECTORY --allocator-stats --trim-live
MALLOC_ARENA_MAX=1 target/release/examples/managed_import_profile NEW_ROOT SOURCE_DIRECTORY --allocator-stats --trim-live

valgrind --tool=massif --time-unit=B --detailed-freq=1 --max-snapshots=200 \
  --depth=40 --threshold=0.5 --massif-out-file=massif.out \
  target/release/examples/managed_import_profile NEW_ROOT SOURCE_DIRECTORY
ms_print massif.out

python -I python/profile_managed_import.py NEW_ROOT SOURCE_DIRECTORY
```

`NEW_ROOT` must not exist. The workers assert this fixture's graph and triple
counts. Use the corpus generated by `python/bench_managed_ontology.py`, or the
retained local `/tmp/ontoenv-final-review-run3/sources` directory.

- `normal-api-memory-profile-2026-10-23.json`: runs, phase samples, allocator
  totals, peak stack attribution, commands, source hashes and tool versions.
- `normal-api-memory-massif-2026-10-23.out.gz`: compressed complete raw Massif
  output; decompress before passing it to `ms_print`.
- Local native repeated outputs: `/tmp/ontoenv-memory-repeated/`.
- Local Python repeated outputs: `/tmp/ontoenv-memory-python/`.

The release workers built successfully; the Rust example passed clippy with
warnings denied and formatting checks. The worker invocations validated all
graph/quad/union counts. No production memory policy was changed.
