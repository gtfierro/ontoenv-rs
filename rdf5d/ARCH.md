# R5TU v0 — An HDT-inspired, mmap-friendly on-disk format for RDF 5-tuples

**Purpose:** an efficient, immutable serialization for datasets of RDF **5-tuples**  
`(id, subject, predicate, object, graphname)` optimized for:
- fast **enumeration** of graphs by `id`, `graphname`, or their pair,
- fast **loading** of an entire `(id, graphname)` graph into memory,
- **many-readers / one-writer** with atomic finalize + `mmap` reading,
- **HDT-like** global term dictionary + compressed per-graph SPO blocks.

---

## 0) Quick Glossary

- **TermID** — integer assigned to a unique RDF term (IRI | BNODE | LITERAL).
- **id_id** — integer ID for the `id` string (e.g., source/file path).
- **gn_id** — integer ID for the `graphname` string.
- **GID** — graph instance ordinal (row index in Graph Directory).
- **uvarint** — unsigned LEB128 variable-length integer.

---

## 1) File Overview

All multi-byte fixed-size integers are **little-endian**.  
All variable-length integers are **LEB128 unsigned (uvarint)**.  
All offsets are **absolute** (from file start).

```

+----------------------+ 0x00
\| Header (fixed 32 B)  |
+----------------------+
\| TOC (array of 32 B)  |
+----------------------+
\| Sections...          |
+----------------------+
\| Footer (16 B)        |
+----------------------+

```

### 1.1 Header (32 bytes)

| Field             | Type  | Notes                                  |
|-------------------|-------|----------------------------------------|
| `magic`           | [u8;4]| `"R5TU"`                               |
| `version_u16`     | u16   | `0x0001`                               |
| `flags_u16`       | u16   | bit0=utf8, bit1=zstd, bit2=pos_perm    |
| `created_unix64`  | u64   | seconds since epoch                    |
| `toc_off_u64`     | u64   | byte offset to TOC                     |
| `toc_len_u32`     | u32   | number of TOC entries                  |
| `reserved_u32`    | u32   | 0                                      |

### 1.2 TOC entry (32 bytes each)

| Field         | Type | Notes                                  |
|---------------|------|----------------------------------------|
| `kind_u16`    | u16  | `SectionKind`                          |
| `reserved_u16`| u16  | 0                                      |
| `off_u64`     | u64  | section start                          |
| `len_u64`     | u64  | section length                         |
| `crc32_u32`   | u32  | (optional in v0)                       |
| `reserved_u32`| u32  | 0                                      |

**SectionKind:**
```

1 TERM\_DICT      | 2 ID\_DICT      | 3 GNAME\_DICT
4 GDIR           | 5 IDX\_ID2GID   | 6 IDX\_GNAME2GID
7 IDX\_PAIR2GID   | 8 TRIPLE\_BLOCKS

```

### 1.3 Footer (16 bytes)

| Field            | Type | Notes                                         |
|------------------|------|-----------------------------------------------|
| `global_crc32`   | u32  | CRC over [0 .. footer_off)                    |
| `eof_magic[12]`  | u8   | `"R5TU_ENDMARK"`                              |

> **Writer rule:** write to temp file, then atomic rename.

---

## 2) Sections (v0 encodings)

### 2.1 String Dictionaries — `ID_DICT`, `GNAME_DICT`

Simple O(1) ID→string plus an optional coarse index for string→ID.  
Reader may implement string→ID using the coarse index; can be upgraded to FST later.

**Layout:**
```

DICT:
u32  n\_entries
u64  str\_bytes\_off    --> \[UTF-8 bytes...]
u64  str\_bytes\_len
u64  offs\_off         --> \[u32 \* (n\_entries+1)] or sparse restart table
u64  offs\_len         // high bit 0x8000... => front-coded string storage
u64  idx\_off (0 if absent) --> \[IndexEntry \* n\_entries] or grouped coarse index
u64  idx\_len         // high bit 0x8000... => grouped coarse index

```

- **Plain blob mode:** concatenation of all strings.
- **Plain offsets mode:** `offs[i]` start of string i in blob; `offs[n]=blob_len`.
- **Front-coded blob mode:** enabled when `offs_len` has high bit `0x8000_0000_0000_0000`.
  - strings are grouped into fixed blocks of 16
  - block head: `len:uvarint` + raw bytes
  - subsequent strings in block: `prefix_len:uvarint` + `suffix_len:uvarint` + suffix bytes
- **Front-coded offsets mode:** `offs` becomes a sparse restart table of `u32` block starts in the blob,
  with one entry per block plus a final sentinel.
- **IndexEntry (20 bytes in current writer; reader accepts legacy 24-byte entries):**
  - `key16[16]` — lowercased first up-to-16 bytes of string, zero-padded.
  - `id_u32` — the entry’s ordinal.
- **Grouped coarse index:** enabled when `idx_len` has high bit `0x8000_0000_0000_0000`.
  - `u32 n_groups`
  - repeated group headers: `key16[16] + ids_off_u32 + count_u32`
  - followed by a packed `u32` id blob
  - all strings sharing the same `key16` share one group header

**Operations:**
- ID→string:
  - plain mode: slice `blob[offs[i]..offs[i+1]]`
  - front-coded mode: decode within the string’s 16-entry block from the nearest restart point
- string→ID: binary search `key16` then string-compare inside blob.
  - grouped index mode: binary search group headers by `key16`, then scan only that group’s ids

**Writer policy (current implementation):**
- `ID_DICT` stays on plain blob storage with the flat 20-byte coarse index.
- `GNAME_DICT` may switch to front-coded storage and/or grouped coarse index when that is smaller.

> Future: replace or augment with mmap-able FSTs for perfect lookups.

---

### 2.2 Global Term Dictionary — `TERM_DICT`

Maps unique RDF terms to `TermID`. `width_u8` controls the offset width used by the offsets table.
The high bit of `width_u8` is reserved for literal-component dictionaries.

**Layout:**
```

u8   width_flags        // low nibble: 4 or 8; high bit 0x80 => literal-component dicts present
u64  n\_terms
u64  kinds\_off          --> \[u8 \* n]  // 0=IRI, 1=BNODE, 2=LITERAL-inline, 3=LITERAL-component-ref
u64  data\_off           --> \[bytes ...] payload blob
u64  offs\_off           --> \[u32 \* (n+1)] or \[u64 \* (n+1)] per width

```

**Payload per term kind:**
- **IRI/BNODE:** raw UTF-8 bytes.
- **LITERAL:** concatenation of
  - `lex_len:uvarint` + `lex_bytes`
  - `has_dt:u8` + if 1: `dt_len:uvarint` + `dt_bytes`
  - `has_lang:u8` + if 1: `lang_len:uvarint` + `lang_bytes`
- **LITERAL-component-ref:** only used when `width_flags & 0x80 != 0`
  - `lex_id:uvarint`
  - `dt_id_plus_one:uvarint` (`0` = none)
  - `lang_id_plus_one:uvarint` (`0` = none)

When the literal-component flag is set, `data_off` begins with three compact component dictionaries
before the per-term payload region:

1. lexical-form dictionary
2. datatype dictionary
3. language dictionary

Each component dictionary is laid out as:

```
u32 n
u64 blob_len
u64 offs_len         // must equal (n + 1) * 4
u8  blob[blob_len]
u32 offs[n + 1]
```

The `offs` table indexes slices inside the immediately preceding blob. Term offsets in `offs_off`
remain relative to `data_off`, so term payloads may start after these component dictionaries.

---

### 2.3 Graph Directory — `GDIR`

One fixed row per graph (GID = row index). Sorted by `(id_id, gn_id)` at build time.

**Header (16 bytes):**
```

u64 n\_rows
u32 row\_size = 32 or 44
u32 reserved = 0

```

**Compact Row (32 bytes):**
```

u32 id\_id
u32 gn\_id
u32 triples\_off
u32 triples\_len
u32 n\_triples
u32 n\_s
u32 n\_p
u32 n\_o

```

**Legacy Row (44 bytes):**
```

u32 id\_id
u32 gn\_id
u64 triples\_off
u64 triples\_len
u64 n\_triples
u32 n\_s
u32 n\_p
u32 n\_o

```

Counts are hints only.

---

### 2.4 Postings Indexes — `IDX_ID2GID`, `IDX_GNAME2GID`

Delta-varint postings (v0); can be swapped for Elias–Fano or Roaring later.

**Layout:**
```

u64 n\_keys                         // number of id\_ids or gn\_ids
u64 key2post\_offs\_off  --> \[u64\*(n\_keys+1)]  // per-key slice into blob
u64 gids\_blob\_off      --> \[bytes...]        // concatenated postings

```

**Per posting list encoding:**
```

uvarint n
uvarint first\_gid
uvarint delta\_1
...
uvarint delta\_(n-1)     // strictly ascending

```

---

### 2.5 Pair Index — `IDX_PAIR2GID`

Sorted fixed-width mapping for `(id_id, gn_id) → gid`.

**Layout:**
```

u64 n\_pairs
u64 pairs\_off -> \[PairEntry \* n\_pairs] sorted by (id\_id, gn\_id)

```

**Compact PairEntry (12 bytes):** `u32 id_id | u32 gn_id | u32 gid`

**Legacy PairEntry (16 bytes):** `u32 id_id | u32 gn_id | u64 gid`

---

### 2.6 Per-Graph Triples — `TRIPLE_BLOCKS`

One block per GID. Each block is either raw or zstd-framed.

**Block header:**
```

u8  enc            // 0=RAW, 1=ZSTD
u32 raw\_len        // length of RAW payload; for ZSTD, compressed len (sanity)
\[ payload ... ]    // RAW or ZSTD frame containing RAW bytes

```

**RAW payload (CSR-like SPO):**
```

uvarint nS
uvarint nP           // total distinct (S,P)
uvarint nT           // triples

S\_vals\[nS]   : uvarint (TermID), delta-coded ascending
S\_heads\[nS+1]: uvarint prefix sums into P\_vals (0..nP)
P\_vals\[nP]   : uvarint (TermID), delta-coded per S-run
P\_heads\[nP+1]: uvarint prefix sums into O\_vals (0..nT)
O\_vals\[nT]   : uvarint (TermID), delta-coded per (S,P)-run

````

> Future: optional POS permutation appended if `flags.bit2==1`.

---

## 3) Writer Spec (Build Pipeline)

Input: list of quintuples `(id_str, s_term, p_term, o_term, gname_str)`.

1. **Assign IDs**
   - Deduplicate `id_str` → `id_id` (u32)
   - Deduplicate `gname_str` → `gn_id` (u32)
   - Deduplicate RDF Terms → `TermID` (u64 ok; u32 if guaranteed <4B)

2. **Group & Sort**
   - Group by pair `(id_id, gn_id)`.  
   - Inside each group, **sort by (S, P, O)** with their TermIDs.

3. **Emit `TRIPLE_BLOCKS`**
   - For each group (becomes a GID), build arrays: `S_vals`, `S_heads`, `P_vals`, `P_heads`, `O_vals` from sorted triples; encode per §2.6.
   - Optionally wrap RAW bytes in ZSTD (independent frame per graph) if `flags.bit1`.

4. **Emit `GDIR`** rows in the same order graphs are written. Row fields:
   - `id_id`, `gn_id`, `triples_off`, `triples_len`, `n_triples`, `n_s`, `n_p`, `n_o`.

5. **Emit `TERM_DICT`**
   - Sort terms by assigned TermID; store kinds and payload slices with offsets.

6. **Emit `ID_DICT` & `GNAME_DICT`**
   - Build blobs + offsets; optionally include coarse index (key16 + id).

7. **Emit postings `IDX_ID2GID` & `IDX_GNAME2GID`**
   - For each `id_id`: collect sorted list of GIDs where it appears; delta-uvarint encode.
   - For each `gn_id`: same.

8. **Emit `IDX_PAIR2GID`**
   - For each `(id_id, gn_id)` group: write a `PairEntry` sorted by `(id_id, gn_id)`.

9. **TOC & Footer**
   - Write TOC with offsets/lengths; optional per-section CRCs.
   - Compute `global_crc32` and write Footer.
   - **Atomic rename** temp → final.

**Invariants & Validation:**
- Posting lists must be strictly increasing.
- `GDIR.n_rows == number of groups`.
- `TRIPLE_BLOCKS` offsets/lengths must not overlap.
- If `flags.bit1==1` then block `enc` may be 0 or 1; if 0, it is raw.

---

## 4) Reader Spec & Minimal Rust Reference

### 4.1 Public Reader API (suggested)

```rust
pub struct R5tuFile { /* mmaps + parsed sections */ }

pub struct GraphRef {
    pub gid: u64,
    pub id: String,
    pub graphname: String,
    pub n_triples: u64,
}

impl R5tuFile {
    pub fn open(path: &Path) -> Result<Self>;
    pub fn enumerate_by_id(&self, id: &str) -> Result<Vec<GraphRef>>;
    pub fn enumerate_by_graphname(&self, gname: &str) -> Result<Vec<GraphRef>>;
    pub fn resolve_gid(&self, id: &str, gname: &str) -> Result<Option<GraphRef>>;
    pub fn triples_ids(&self, gid: u64) -> Result<impl Iterator<Item=(u64,u64,u64)>>;
    pub fn term_to_string(&self, term_id: u64) -> Result<String>;
}
```

---

This design borrows the core ideas of **HDT** (global dictionary + compressed triples), adapted to per-graph blocks and 5-tuple needs. See RDF/HDT for background; implementation is original for R5TU. Created with ChatGPT-5
