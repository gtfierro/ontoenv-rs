//! Defines the configuration structures for the OntoEnv environment.
//! This includes the main `Config` struct and related structs for ontology locations and environment setup.

use crate::options::CacheMode;
use crate::policy::{DefaultPolicy, ResolutionPolicy};
use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_INCLUDE_PATTERNS: &[&str] = &["*.ttl", "*.xml", "*.n3"];
const DEFAULT_REMOTE_CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24);

fn default_remote_cache_ttl_secs() -> u64 {
    DEFAULT_REMOTE_CACHE_TTL.as_secs()
}

fn cache_mode_ser<S>(mode: &CacheMode, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_bool(mode.is_enabled())
}

fn cache_mode_de<'de, D>(deserializer: D) -> Result<CacheMode, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = bool::deserialize(deserializer)?;
    Ok(CacheMode::from(value))
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Config {
    pub root: PathBuf,
    #[serde(default)]
    pub locations: Vec<PathBuf>,
    /// External graph store identifier (e.g., Python module path), if used.
    #[serde(default)]
    pub external_graph_store: Option<String>,
    // include glob patterns (globset syntax; ** supported)
    #[serde(default)]
    includes: Vec<String>,
    // exclude patterns
    #[serde(default)]
    excludes: Vec<String>,
    /// Regex patterns applied to ontology IRIs to include
    #[serde(default)]
    include_ontologies: Vec<String>,
    /// Regex patterns applied to ontology IRIs to exclude
    #[serde(default)]
    exclude_ontologies: Vec<String>,
    // require ontology names?
    pub require_ontology_names: bool,
    // strict mode (does not allow for any errors in the ontology files)
    pub strict: bool,
    // offline mode (does not fetch remote ontologies)
    pub offline: bool,
    // resolution policy
    pub resolution_policy: String,
    #[serde(
        default,
        serialize_with = "cache_mode_ser",
        deserialize_with = "cache_mode_de"
    )]
    pub use_cached_ontologies: CacheMode,
    /// Maximum age for cached remote ontologies before they are re-fetched, in seconds.
    #[serde(default = "default_remote_cache_ttl_secs")]
    pub remote_cache_ttl_secs: u64,
    // if true, do not store the ontoenv store on disk
    pub temporary: bool,
}

impl Config {
    /// Creates a new `ConfigBuilder` to construct a `Config`.
    pub fn builder() -> ConfigBuilder {
        // Keep construction centralized so defaults stay consistent.
        ConfigBuilder::new()
    }

    pub(crate) fn build_globsets(&self) -> Result<(GlobSet, GlobSet)> {
        fn contains_meta(pat: &str) -> bool {
            pat.chars()
                .any(|c| matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '!'))
        }

        fn expand_patterns(patterns: &[String]) -> Result<GlobSet> {
            let mut builder = GlobSetBuilder::new();
            for pat in patterns {
                let trimmed = pat.trim_end_matches('/');
                builder.add(Glob::new(trimmed)?);

                // If the pattern looks like a bare directory (no glob meta),
                // also match anything underneath it to support inputs like "lib/tests".
                if !contains_meta(trimmed) {
                    builder.add(Glob::new(&format!("{}/**", trimmed))?);
                }
            }
            Ok(builder.build()?)
        }

        let includes = expand_patterns(&self.includes)?;
        let excludes = expand_patterns(&self.excludes)?;
        Ok((includes, excludes))
    }

    pub(crate) fn includes_is_empty(&self) -> bool {
        self.includes.is_empty()
    }

    pub(crate) fn build_ontology_regexes(&self) -> Result<(Vec<Regex>, Vec<Regex>)> {
        let inc = self
            .include_ontologies
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<_>, _>>()?;
        let exc = self
            .exclude_ontologies
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((inc, exc))
    }

    /// A convenient constructor for a default offline, non-temporary environment.
    /// Searches for ontologies in the root directory.
    pub fn default(root: PathBuf) -> Result<Self> {
        // Provide a one-liner for the most common offline local use case.
        Config::builder().root(root).offline(true).build()
    }

    /// A convenient constructor for a temporary environment.
    pub fn temporary(root: PathBuf) -> Result<Self> {
        // Avoid persisting state for short-lived or test workflows.
        Config::builder().root(root).temporary(true).build()
    }

    /// A convenient constructor for a default configuration that uses default file matching patterns.
    pub fn new_with_default_matches(root: PathBuf) -> Result<Self> {
        // Defaults to include patterns suitable for RDF files.
        Config::builder().root(root).build()
    }

    /// Determines if a file is included in the ontology environment configuration
    pub fn is_included(&self, path: &Path) -> bool {
        // Apply include/exclude glob logic with a safe fallback on invalid patterns.
        // Match relative to the config root when possible so globs like "lib/**/*.ttl"
        // behave intuitively even when walking absolute paths.
        let rel = path.strip_prefix(&self.root).unwrap_or(path).to_path_buf();

        let (include_set, exclude_set) = match self.build_globsets() {
            Ok(sets) => sets,
            Err(err) => {
                // Fall back to permissive behavior if patterns are invalid
                log::warn!("Invalid include/exclude pattern: {err}");
                return true;
            }
        };

        if exclude_set.is_match(&rel) {
            return false;
        }

        if self.includes.is_empty() {
            // no includes means include everything unless excluded
            return true;
        }

        include_set.is_match(&rel)
    }

    pub fn save_to_file(&self, file: &Path) -> Result<()> {
        // Persist as JSON so config is editable and stable across versions.
        let config_str = serde_json::to_string_pretty(&self)?;
        let mut file = std::fs::File::create(file)?;
        file.write_all(config_str.as_bytes())?;
        Ok(())
    }

    pub fn from_file(file: &Path) -> Result<Self> {
        // Load config from disk without side effects (no path normalization here).
        let file = std::fs::File::open(file)?;
        let reader = BufReader::new(file);
        let config: Config = serde_json::from_reader(reader)?;
        Ok(config)
    }

    /// Prints out the current Config in a clear and readable way for command line output.
    pub fn print(&self) {
        // Prefer a human-readable dump for CLI output and debugging.
        println!("Configuration:");
        println!("  Root: {}", self.root.display());
        if let Some(store) = &self.external_graph_store {
            println!("  External Graph Store: {store}");
        }
        if !self.locations.is_empty() {
            println!("  Locations:");
            for loc in &self.locations {
                println!("    - {}", loc.display());
            }
        }
        println!("  Include Patterns:");
        for pat in &self.includes {
            println!("    - {pat}");
        }
        if !self.excludes.is_empty() {
            println!("  Exclude Patterns:");
            for pat in &self.excludes {
                println!("    - {pat}");
            }
        }
        if !self.include_ontologies.is_empty() {
            println!("  Include Ontology Regexes:");
            for pat in &self.include_ontologies {
                println!("    - {pat}");
            }
        }
        if !self.exclude_ontologies.is_empty() {
            println!("  Exclude Ontology Regexes:");
            for pat in &self.exclude_ontologies {
                println!("    - {pat}");
            }
        }
        println!("  Require Ontology Names: {}", self.require_ontology_names);
        println!("  Strict: {}", self.strict);
        println!("  Offline: {}", self.offline);
        println!(
            "  Use Cached Ontologies: {}",
            self.use_cached_ontologies.is_enabled()
        );
        println!("  Remote Cache TTL (secs): {}", self.remote_cache_ttl_secs);
        println!("  Resolution Policy: {}", self.resolution_policy);
        println!("  Temporary: {}", self.temporary);
    }
}

/// A builder for creating `Config` instances.
pub struct ConfigBuilder {
    root: Option<PathBuf>,
    locations: Option<Vec<PathBuf>>,
    external_graph_store: Option<Option<String>>,
    includes: Option<Vec<String>>,
    excludes: Option<Vec<String>>,
    include_ontologies: Option<Vec<String>>,
    exclude_ontologies: Option<Vec<String>>,
    require_ontology_names: Option<bool>,
    strict: Option<bool>,
    offline: Option<bool>,
    resolution_policy: Option<String>,
    temporary: Option<bool>,
    use_cached_ontologies: Option<CacheMode>,
    remote_cache_ttl_secs: Option<u64>,
}

impl ConfigBuilder {
    /// Creates a new `ConfigBuilder` with default values.
    pub fn new() -> Self {
        // Start from None so we can detect which fields were explicitly set.
        Self {
            root: None,
            locations: None,
            external_graph_store: None,
            includes: None,
            excludes: None,
            include_ontologies: None,
            exclude_ontologies: None,
            require_ontology_names: None,
            strict: None,
            offline: None,
            resolution_policy: None,
            temporary: None,
            use_cached_ontologies: None,
            remote_cache_ttl_secs: None,
        }
    }

    /// Sets the root directory for the environment. This is a required field.
    pub fn root(mut self, root: PathBuf) -> Self {
        // Store the root so relative paths can be resolved consistently.
        self.root = Some(root);
        self
    }

    /// Sets the search locations for ontologies. If not set, no directories will be scanned.
    pub fn locations(mut self, locations: Vec<PathBuf>) -> Self {
        // Allow explicit locations to override the default root scan behavior.
        self.locations = Some(locations);
        self
    }

    /// Sets the external graph store identifier (if using a non-default backend).
    pub fn external_graph_store<S: Into<String>>(mut self, store: Option<S>) -> Self {
        // Store an optional backend identifier for cross-language IO integrations.
        self.external_graph_store = Some(store.map(|s| s.into()));
        self
    }

    /// Sets the glob patterns for including files.
    /// Defaults to `["*.ttl", "*.xml", "*.n3"]`.
    pub fn includes<I>(mut self, includes: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        // Convert iterable into owned strings for serialization.
        self.includes = Some(
            includes
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        );
        self
    }

    /// Sets the glob patterns for excluding files. Defaults to an empty list.
    pub fn excludes<I>(mut self, excludes: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        // Convert iterable into owned strings for serialization.
        self.excludes = Some(
            excludes
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        );
        self
    }

    /// Sets regex patterns for ontology IRIs to include. If set, only matching ontologies are kept.
    pub fn include_ontologies<I>(mut self, patterns: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        // Store regex patterns as strings to keep config JSON compact.
        self.include_ontologies = Some(
            patterns
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        );
        self
    }

    /// Sets regex patterns for ontology IRIs to exclude.
    pub fn exclude_ontologies<I>(mut self, patterns: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
        // Store regex patterns as strings to keep config JSON compact.
        self.exclude_ontologies = Some(
            patterns
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        );
        self
    }

    /// Sets whether ontology names are required to be unique. Defaults to `false`.
    pub fn require_ontology_names(mut self, require: bool) -> Self {
        // Toggle stricter validation for environments that rely on canonical names.
        self.require_ontology_names = Some(require);
        self
    }

    /// Sets strict mode. Defaults to `false`.
    pub fn strict(mut self, strict: bool) -> Self {
        // Enable strict parsing/validation for CI or production use.
        self.strict = Some(strict);
        self
    }

    /// Sets offline mode. Defaults to `false`.
    pub fn offline(mut self, offline: bool) -> Self {
        // Disable network retrieval for air-gapped or deterministic runs.
        self.offline = Some(offline);
        self
    }

    /// Sets whether to reuse cached ontologies when possible. Defaults to disabled.
    pub fn use_cached_ontologies(mut self, mode: CacheMode) -> Self {
        // Control cache behavior explicitly instead of boolean flags.
        self.use_cached_ontologies = Some(mode);
        self
    }

    /// Sets the remote ontology cache TTL (seconds). Defaults to 24h.
    pub fn remote_cache_ttl_secs(mut self, ttl_secs: u64) -> Self {
        // Keep cache freshness configurable for fast iteration vs. stability.
        self.remote_cache_ttl_secs = Some(ttl_secs);
        self
    }

    /// Sets the resolution policy. Defaults to `"default"`.
    pub fn resolution_policy(mut self, policy: String) -> Self {
        // Store policy name for serialization and later policy lookup.
        self.resolution_policy = Some(policy);
        self
    }

    /// Sets temporary mode. If `true`, the environment is not persisted to disk.
    /// Defaults to `false`.
    pub fn temporary(mut self, temporary: bool) -> Self {
        // Support ephemeral environments for testing and exploration.
        self.temporary = Some(temporary);
        self
    }

    /// Builds the `Config` object.
    ///
    /// # Errors
    ///
    /// Returns an error if the `root` is not set, or if any of the glob patterns are invalid.
    pub fn build(self) -> Result<Config> {
        // Consolidate defaults and validate required fields before constructing.
        let root = self
            .root
            .ok_or_else(|| anyhow::anyhow!("Config 'root' is required"))?;

        let locations = self.locations.unwrap_or_default();

        let includes_str = self.includes.unwrap_or_else(|| {
            DEFAULT_INCLUDE_PATTERNS
                .iter()
                .map(|s| s.to_string())
                .collect()
        });
        let excludes_str = self.excludes.unwrap_or_default();
        let include_ontologies = self.include_ontologies.unwrap_or_default();
        let exclude_ontologies = self.exclude_ontologies.unwrap_or_default();

        Ok(Config {
            root,
            locations,
            external_graph_store: self.external_graph_store.unwrap_or(None),
            includes: includes_str,
            excludes: excludes_str,
            include_ontologies,
            exclude_ontologies,
            require_ontology_names: self.require_ontology_names.unwrap_or(false),
            strict: self.strict.unwrap_or(false),
            offline: self.offline.unwrap_or(false),
            resolution_policy: self
                .resolution_policy
                .unwrap_or_else(|| DefaultPolicy.policy_name().to_string()),
            use_cached_ontologies: self.use_cached_ontologies.unwrap_or_default(),
            remote_cache_ttl_secs: self
                .remote_cache_ttl_secs
                .unwrap_or_else(default_remote_cache_ttl_secs),
            temporary: self.temporary.unwrap_or(false),
        })
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HowCreated {
    New,
    SameConfig,
    RecreatedDifferentConfig,
    RecreatedFlag,
}

impl std::fmt::Display for HowCreated {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            HowCreated::New => write!(f, "New Environment"),
            HowCreated::SameConfig => write!(f, "Same Config. Reusing existing environment."),
            HowCreated::RecreatedDifferentConfig => {
                write!(f, "Recreated environment due to different config")
            }
            HowCreated::RecreatedFlag => write!(f, "Recreated environment due to 'recreate' flag"),
        }
    }
}
