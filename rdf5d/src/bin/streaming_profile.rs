use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use rdf5d::{
    Quint, SpillPolicy, StreamingWriteStats, StreamingWriter, StreamingWriterOptions, Term,
    WriterOptions, write_file_with_options,
};

const ENCODED_QUINT_BYTES: u64 = 32;

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum Mode {
    Batch,
    Streaming,
}

#[derive(Debug, Parser)]
#[command(name = "streaming_profile")]
#[command(about = "Profile rdf5d batch and streaming writer memory behavior")]
struct Args {
    #[arg(long, value_enum, default_value_t = Mode::Streaming)]
    mode: Mode,
    #[arg(long, default_value_t = 20)]
    graphs: usize,
    #[arg(long = "triples-per-graph", default_value_t = 10_000)]
    triples_per_graph: usize,
    #[arg(long = "chunk-quads", default_value_t = 4_096)]
    chunk_quads: usize,
    #[arg(long, default_value_t = false)]
    zstd: bool,
    #[arg(long = "with-crc", default_value_t = true)]
    with_crc: bool,
    #[arg(long = "sample-ms", default_value_t = 2)]
    sample_ms: u64,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ProcMemory {
    rss_bytes: u64,
    hwm_bytes: u64,
}

struct PeakRssMonitor {
    stop: Arc<AtomicBool>,
    peak_rss: Arc<AtomicU64>,
    peak_hwm: Arc<AtomicU64>,
    handle: Option<thread::JoinHandle<()>>,
}

impl PeakRssMonitor {
    fn spawn(sample_period: Duration) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak_rss = Arc::new(AtomicU64::new(0));
        let peak_hwm = Arc::new(AtomicU64::new(0));

        let stop_thread = Arc::clone(&stop);
        let peak_rss_thread = Arc::clone(&peak_rss);
        let peak_hwm_thread = Arc::clone(&peak_hwm);
        let handle = thread::spawn(move || {
            while !stop_thread.load(Ordering::Relaxed) {
                let mem = read_proc_self_memory();
                update_peak(&peak_rss_thread, mem.rss_bytes);
                update_peak(&peak_hwm_thread, mem.hwm_bytes);
                thread::sleep(sample_period);
            }
            let mem = read_proc_self_memory();
            update_peak(&peak_rss_thread, mem.rss_bytes);
            update_peak(&peak_hwm_thread, mem.hwm_bytes);
        });

        Self {
            stop,
            peak_rss,
            peak_hwm,
            handle: Some(handle),
        }
    }

    fn stop(mut self) -> ProcMemory {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        ProcMemory {
            rss_bytes: self.peak_rss.load(Ordering::Relaxed),
            hwm_bytes: self.peak_hwm.load(Ordering::Relaxed),
        }
    }
}

fn update_peak(slot: &AtomicU64, value: u64) {
    let mut current = slot.load(Ordering::Relaxed);
    while value > current {
        match slot.compare_exchange(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn read_proc_self_memory() -> ProcMemory {
    match fs::read("/proc/self/status") {
        Ok(bytes) => parse_proc_status(&bytes),
        Err(_) => ProcMemory::default(),
    }
}

fn parse_proc_status(bytes: &[u8]) -> ProcMemory {
    let text = String::from_utf8_lossy(bytes);
    let mut rss_bytes = 0;
    let mut hwm_bytes = 0;

    for line in text.lines() {
        if let Some(value) = parse_kib_value(line, "VmRSS:") {
            rss_bytes = value;
        } else if let Some(value) = parse_kib_value(line, "VmHWM:") {
            hwm_bytes = value;
        }
    }

    ProcMemory {
        rss_bytes,
        hwm_bytes,
    }
}

fn parse_kib_value(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?.trim_start();
    let number = rest.split_whitespace().next()?.parse::<u64>().ok()?;
    Some(number.saturating_mul(1024))
}

fn writer_options(args: &Args) -> WriterOptions {
    WriterOptions {
        zstd: args.zstd,
        with_crc: args.with_crc,
    }
}

fn generate_quint(graph_idx: usize, triple_idx: usize) -> Quint {
    let id = format!("dataset/{graph_idx}");
    let gname = format!("http://example.org/graph/{graph_idx}");
    let s = if triple_idx.is_multiple_of(5) {
        Term::BNode(format!("b{graph_idx}_{triple_idx}"))
    } else {
        Term::Iri(format!("http://example.org/s/{graph_idx}/{triple_idx}"))
    };
    let p = Term::Iri(format!("http://example.org/p/{}", triple_idx % 20));
    let o = match triple_idx % 4 {
        0 => Term::Iri(format!("http://example.org/o/{triple_idx}")),
        1 => Term::Literal {
            lex: format!("value {triple_idx}"),
            dt: None,
            lang: None,
        },
        2 => Term::Literal {
            lex: format!("typed {triple_idx}"),
            dt: Some("http://www.w3.org/2001/XMLSchema#string".into()),
            lang: None,
        },
        _ => Term::Literal {
            lex: format!("hello {triple_idx}"),
            dt: None,
            lang: Some("en".into()),
        },
    };
    Quint { id, s, p, o, gname }
}

fn build_batch(
    path: &Path,
    args: &Args,
) -> Result<(usize, Option<StreamingWriteStats>), Box<dyn Error>> {
    let total_quads = args.graphs.saturating_mul(args.triples_per_graph);
    let mut quads = Vec::with_capacity(total_quads);
    for graph_idx in 0..args.graphs {
        for triple_idx in 0..args.triples_per_graph {
            quads.push(generate_quint(graph_idx, triple_idx));
        }
    }
    write_file_with_options(path, &quads, writer_options(args))?;
    Ok((total_quads, None))
}

fn build_streaming(
    path: &Path,
    args: &Args,
) -> Result<(usize, Option<StreamingWriteStats>), Box<dyn Error>> {
    let total_quads = args.graphs.saturating_mul(args.triples_per_graph);
    let mut writer = StreamingWriter::with_options_and_hint(
        path,
        StreamingWriterOptions {
            writer: writer_options(args),
            spill_policy: SpillPolicy::MaxPendingQuads(args.chunk_quads),
        },
        total_quads,
    );
    for graph_idx in 0..args.graphs {
        for triple_idx in 0..args.triples_per_graph {
            writer.add(generate_quint(graph_idx, triple_idx))?;
        }
    }
    let stats = writer.finalize_with_stats()?;
    Ok((total_quads, Some(stats)))
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let output_path = args
        .output
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("rdf5d_streaming_profile.r5tu"));

    let monitor = PeakRssMonitor::spawn(Duration::from_millis(args.sample_ms.max(1)));
    let started = Instant::now();
    let (total_quads, stats) = match args.mode {
        Mode::Batch => build_batch(&output_path, &args)?,
        Mode::Streaming => build_streaming(&output_path, &args)?,
    };
    let elapsed = started.elapsed();
    let mem = monitor.stop();
    let file_bytes = fs::metadata(&output_path)?.len();

    let stats = stats.unwrap_or_default();
    let pending_quads_bytes_upper_bound = (stats.max_pending_quads as u64) * ENCODED_QUINT_BYTES;

    println!("{{");
    println!(
        "  \"mode\": \"{}\",",
        match args.mode {
            Mode::Batch => "batch",
            Mode::Streaming => "streaming",
        }
    );
    println!("  \"graphs\": {},", args.graphs);
    println!("  \"triples_per_graph\": {},", args.triples_per_graph);
    println!("  \"total_quads\": {},", total_quads);
    println!("  \"chunk_quads\": {},", args.chunk_quads);
    println!("  \"elapsed_ms\": {:.3},", elapsed.as_secs_f64() * 1000.0);
    println!("  \"peak_rss_bytes\": {},", mem.rss_bytes);
    println!("  \"peak_hwm_bytes\": {},", mem.hwm_bytes);
    println!("  \"file_bytes\": {},", file_bytes);
    println!("  \"streaming_run_count\": {},", stats.run_count);
    println!(
        "  \"streaming_temp_bytes_written\": {},",
        stats.temp_bytes_written
    );
    println!(
        "  \"streaming_pending_quads_bytes_upper_bound\": {}",
        pending_quads_bytes_upper_bound
    );
    println!("}}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ProcMemory, parse_proc_status};

    #[test]
    fn parses_proc_status_rss_and_hwm() {
        let status = b"Name:\ttest\nVmRSS:\t   1234 kB\nVmHWM:\t5678 kB\n";
        let mem = parse_proc_status(status);
        assert_eq!(
            mem,
            ProcMemory {
                rss_bytes: 1234 * 1024,
                hwm_bytes: 5678 * 1024,
            }
        );
    }

    #[test]
    fn missing_proc_status_fields_default_to_zero() {
        let mem = parse_proc_status(b"Name:\ttest\nVmSize:\t42 kB\n");
        assert_eq!(mem, ProcMemory::default());
    }
}
