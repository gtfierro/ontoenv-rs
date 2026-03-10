use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use rdf5d::{
    reader::R5tuFile,
    writer::{Quint, Term, WriterOptions, write_file_with_options},
};

struct CountingAlloc;

static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ALLOC_BYTES: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        ALLOC_BYTES.fetch_add(new_size, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

fn reset_alloc_stats() {
    ALLOC_COUNT.store(0, Ordering::Relaxed);
    ALLOC_BYTES.store(0, Ordering::Relaxed);
}

fn allocated_bytes() -> usize {
    ALLOC_BYTES.load(Ordering::Relaxed)
}

#[test]
fn raw_decode_does_not_materialize_all_objects() {
    let mut quints = Vec::with_capacity(10_000);
    for i in 0..10_000 {
        quints.push(Quint {
            id: "dataset".into(),
            s: Term::Iri("http://ex/s".into()),
            p: Term::Iri("http://ex/p".into()),
            o: Term::Iri(format!("http://ex/o/{i}")),
            gname: "g".into(),
        });
    }

    let mut path = std::env::temp_dir();
    path.push("decode_alloc.r5tu");
    write_file_with_options(
        &path,
        &quints,
        WriterOptions {
            zstd: false,
            with_crc: true,
        },
    )
    .unwrap();

    let file = R5tuFile::open(&path).unwrap();
    reset_alloc_stats();

    let mut iter = file.triples_ids(0).unwrap();
    let first = iter.next().unwrap();
    let bytes = allocated_bytes();

    assert_eq!(file.term_to_string(first.0).unwrap(), "http://ex/s");
    assert!(
        bytes < 4096,
        "expected lazy decode allocations to stay small, got {bytes} bytes"
    );

    let _ = std::fs::remove_file(&path);
}
