//! Linux/glibc diagnostic of a normal API import, without readers or queries.
//! Usage: managed_import_profile NEW_ROOT SOURCE_DIRECTORY [--allocator-stats] [--trim-live]
//! Allocator trim is diagnostic only and never changes OntoEnv's production behavior.

use anyhow::{ensure, Context, Result};
use ontoenv::api::{ConnectSync, OntoEnv};
use ontoenv::config::Config;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[cfg(all(target_os = "linux", target_env = "gnu"))]
unsafe extern "C" {
    fn malloc_stats();
    fn malloc_trim(pad: usize) -> std::ffi::c_int;
}

fn memory(phase: &str, allocator_stats: bool) -> Result<Value> {
    let mut result = serde_json::Map::new();
    result.insert("phase".into(), json!(phase));
    #[cfg(target_os = "linux")]
    {
        for line in std::fs::read_to_string("/proc/self/status")?.lines() {
            if line.starts_with("VmRSS:") || line.starts_with("VmHWM:") {
                let fields: Vec<_> = line.split_whitespace().collect();
                result.insert(
                    fields[0].trim_end_matches(':').into(),
                    json!(fields[1].parse::<u64>()?),
                );
            }
        }
        for line in std::fs::read_to_string("/proc/self/smaps_rollup")?.lines() {
            if line.starts_with("Rss:") || line.starts_with("Pss:") {
                let fields: Vec<_> = line.split_whitespace().collect();
                result.insert(
                    fields[0].trim_end_matches(':').into(),
                    json!(fields[1].parse::<u64>()?),
                );
            }
        }
    }
    if allocator_stats {
        eprintln!("ALLOCATOR_PHASE {phase}");
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        // SAFETY: diagnostic glibc routine with no caller-owned pointers.
        unsafe {
            malloc_stats()
        };
    }
    eprintln!("MEMORY_PHASE {}", Value::Object(result.clone()));
    Ok(Value::Object(result))
}

fn object_bytes(root: &Path) -> Result<u64> {
    let directory = root.join(".ontoenv-snapshots");
    let head: Value = serde_json::from_slice(&std::fs::read(directory.join("CURRENT"))?)?;
    let manifest = head["manifest"].as_str().context("Missing manifest")?;
    let record: Value =
        serde_json::from_slice(&std::fs::read(directory.join("manifests").join(manifest))?)?;
    record["object_length"]
        .as_u64()
        .context("Missing object length")
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() >= 2,
        "Expected NEW_ROOT SOURCE_DIRECTORY [--allocator-stats]"
    );
    let root = PathBuf::from(&args[0]);
    let sources = std::fs::canonicalize(&args[1])?;
    ensure!(sources.is_dir(), "Source directory must exist");
    let allocator_stats = args.iter().any(|arg| arg == "--allocator-stats");
    let trim_live = args.iter().any(|arg| arg == "--trim-live");
    std::fs::create_dir(&root)?;
    let config = Config::builder()
        .root(root.clone())
        .locations(vec![sources])
        .temporary(false)
        .offline(true)
        .strict(false)
        .build()?;
    let mut phases = vec![memory("before_connect", allocator_stats)?];
    let mut env = OntoEnv::connect(config, ConnectSync::Auto, false)?;
    phases.push(memory("after_connect", allocator_stats)?);
    let started = Instant::now();
    env.update()?;
    let update_ms = started.elapsed().as_secs_f64() * 1000.0;
    phases.push(memory("after_update", allocator_stats)?);
    let stats = env.stats()?;
    let rdf = env.snapshot().context("Missing managed snapshot")?.rdf();
    let stored_quads = rdf.graph_names().try_fold(0usize, |count, name| {
        rdf.triple_count(rdf5d::Scope::ByName(name))
            .map(|next| count + next)
    })?;
    drop(rdf);
    let encoded_bytes = object_bytes(&root)?;
    ensure!(
        stats.num_graphs == 100,
        "Expected 100 graphs, got {}",
        stats.num_graphs
    );
    ensure!(
        stats.num_triples == 333_008,
        "Unexpected stored triple count: {}",
        stats.num_triples
    );
    ensure!(
        stored_quads == 334_059,
        "Unexpected stored quad count: {stored_quads}"
    );
    if trim_live {
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        // SAFETY: diagnostic only; glibc synchronizes its own allocator state.
        unsafe {
            malloc_trim(0)
        };
        phases.push(memory(
            "live_environment_after_diagnostic_trim",
            allocator_stats,
        )?);
    }
    drop(env);
    phases.push(memory("after_drop", allocator_stats)?);
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: glibc accepts a padding size and manages its own synchronized arenas.
    unsafe {
        malloc_trim(0)
    };
    phases.push(memory("after_diagnostic_trim", allocator_stats)?);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "update_ms": update_ms,
            "graphs": stats.num_graphs,
            "unique_union_triples": stats.num_triples,
            "stored_quads": stored_quads,
            "encoded_bytes": encoded_bytes,
            "parallelism": ontoenv::api::import_parallelism(),
            "malloc_arena_max": std::env::var("MALLOC_ARENA_MAX").ok(),
            "phases_kib": phases,
        }))?
    );
    Ok(())
}
