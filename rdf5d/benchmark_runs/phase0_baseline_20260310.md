# Phase 0 Baseline (2026-03-10)

Commands:

```sh
cargo bench --bench rdf5d_bench --features mmap,zstd workload_matrix
CARGO_TARGET_DIR=/tmp/rdf5d-target cargo run --release --bin workload_profile --features mmap,zstd -- --iterations 5
```

## Criterion workload matrix

Representative latency ranges from `workload_matrix`:

| Workload | Write | Open strict | Open trusted | Open mmap | Read all | Resolve all |
|---|---:|---:|---:|---:|---:|---:|
| many_small_graphs | 1.595-1.841 ms | 808.33-825.61 us | 2.0595-2.0967 us | 3.7650-3.8354 us | 77.965-78.542 us | 2.6711-2.6890 ms |
| one_large_graph | 32.748-33.065 ms | 17.598-17.828 ms | 24.916-25.256 us | 4.5678-4.6274 us | 279.36-281.97 us | 136.98-139.13 ns |
| repeated_literals | 2.8293-2.9345 ms | 840.15-853.23 us | 2.4830-2.5073 us | 4.2237-4.2793 us | 63.550-64.388 us | 40.336-40.512 us |
| high_cardinality_names | 2.8636-3.0182 ms | 1.7657-1.7881 ms | 4.1177-4.1455 us | 5.4801-5.5776 us | 85.295-86.709 us | 3.3775-3.4029 ms |

## Workload profile

Median timings and top-level section sizes from `workload_profile`:

| Workload | File bytes | Write ms | Open strict ms | Open trusted ms | Open mmap ms | Read all ms | Resolve all ms |
|---|---:|---:|---:|---:|---:|---:|---:|
| many_small_graphs | 56,114 | 1.232 | 0.725 | 0.003 | 0.006 | 0.076 | 2.888 |
| one_large_graph | 1,252,779 | 38.590 | 19.655 | 0.029 | 0.007 | 0.257 | 0.000 |
| repeated_literals | 59,320 | 2.619 | 0.877 | 0.002 | 0.005 | 0.053 | 0.039 |
| high_cardinality_names | 123,584 | 2.610 | 2.049 | 0.003 | 0.006 | 0.067 | 3.302 |

### Section bytes by workload

| Workload | TERM_DICT | ID_DICT | GNAME_DICT | GDIR | IDX_ID2GID | IDX_GNAME2GID | IDX_PAIR2GID | TRIPLE_BLOCKS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| many_small_graphs | 26,764 | 6,946 | 1,884 | 6,416 | 2,104 | 2,104 | 2,416 | 7,176 |
| one_large_graph | 1,073,360 | 89 | 106 | 48 | 42 | 42 | 28 | 178,760 |
| repeated_literals | 40,864 | 1,121 | 336 | 816 | 282 | 282 | 316 | 14,999 |
| high_cardinality_names | 85,199 | 13,136 | 3,779 | 6,416 | 2,104 | 2,104 | 2,416 | 8,126 |

## Takeaways

- `TERM_DICT` is the dominant section for one-large-graph, repeated-literal, and high-cardinality-name workloads.
- High-cardinality ids still leave `ID_DICT` materially larger than `GNAME_DICT`, even after the selective 2.1 redesign.
- Strict open time scales with file size and CRC work; trusted and mmap opens remain effectively flat across workloads.
- Many-small-graph and high-cardinality-name workloads spend a visible amount of time in `resolve_all`, which keeps 3.2 and any future dictionary/FST work relevant after broader corpus validation.
