//! Carriage-return progress reporter used by `OntoEnv::update_all` and friends.
//!
//! Output is off by default for library, test, and Python consumers; the CLI
//! binary calls [`set_progress_output_enabled`] at startup to opt in. A Cargo
//! feature would leak through workspace feature unification into every crate
//! that depends on ontoenv, so the switch is a runtime flag instead.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

static PROGRESS_OUTPUT_ENABLED: AtomicBool = AtomicBool::new(false);

/// Enable or disable the progress reporter globally for this process.
pub fn set_progress_output_enabled(enabled: bool) {
    PROGRESS_OUTPUT_ENABLED.store(enabled, Ordering::Relaxed);
}

fn plural<'a>(n: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if n == 1 {
        singular
    } else {
        plural
    }
}

#[derive(Default)]
pub(crate) struct ProgressReporter {
    enabled: bool,
    last_len: usize,
    discovered: usize,
    processed: usize,
    loaded: usize,
    reused: usize,
}

impl ProgressReporter {
    pub(crate) fn new() -> Self {
        // Gate on the CLI-level opt-in AND a real TTY, so integration tests and
        // piped output never see progress lines.
        let enabled =
            PROGRESS_OUTPUT_ENABLED.load(Ordering::Relaxed) && std::io::stderr().is_terminal();
        Self {
            enabled,
            ..Self::default()
        }
    }

    pub(crate) fn silent() -> Self {
        Self::default()
    }

    pub(crate) fn add_discovered(&mut self, n: usize) {
        self.discovered = self.discovered.saturating_add(n);
    }

    pub(crate) fn announce_discovered(&mut self, n: usize) {
        self.add_discovered(n);
        if !self.enabled {
            return;
        }
        self.render(&format!(
            "Discovered {} {} to process...",
            n,
            plural(n, "source", "sources")
        ));
    }

    pub(crate) fn tick_loaded(&mut self) {
        self.loaded += 1;
    }

    pub(crate) fn tick_reused(&mut self) {
        self.reused += 1;
    }

    pub(crate) fn tick_processed(&mut self) {
        self.processed += 1;
    }

    pub(crate) fn loading<D: std::fmt::Display>(&mut self, target: D, queued_sources: usize) {
        if !self.enabled {
            return;
        }
        self.render(&format!(
            "Processed {}/{} sources, loaded {}, reused {}, queue {}, loading {}",
            self.processed,
            self.discovered.max(self.processed),
            self.loaded,
            self.reused,
            queued_sources.saturating_sub(1),
            target
        ));
    }

    pub(crate) fn expanding<D: std::fmt::Display>(&mut self, target: D, imports: usize) {
        if !self.enabled {
            return;
        }
        self.render(&format!(
            "Processed {}/{} sources, loaded {}, reused {}, expanding {} to {} {}",
            self.processed,
            self.discovered.max(self.processed),
            self.loaded,
            self.reused,
            target,
            imports,
            plural(imports, "import", "imports")
        ));
    }

    fn render(&mut self, line: &str) {
        let pad = self.last_len.saturating_sub(line.len());
        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        let _ = write!(handle, "\r{line}{:pad$}", "");
        let _ = handle.flush();
        self.last_len = line.len();
    }

    fn finish(&mut self) {
        if !self.enabled || self.last_len == 0 {
            return;
        }
        let stderr = std::io::stderr();
        let mut handle = stderr.lock();
        let _ = write!(handle, "\r{:width$}\r", "", width = self.last_len);
        let _ = handle.flush();
        self.last_len = 0;
    }
}

impl Drop for ProgressReporter {
    fn drop(&mut self) {
        self.finish();
    }
}
