//! Defines the configuration structures for the OntoEnv environment.
//! This includes the main `Config` struct and related structs for ontology locations and environment setup.

use crate::policy::{DefaultPolicy, ResolutionPolicy};
use anyhow::Result;
use glob::{Pattern, PatternError};
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};

fn vec_pattern_ser<S>(patterns: &Vec<Pattern>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    // serialize to strings by calling the display method on the patterns
    let patterns: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
    patterns.serialize(serializer)
}

fn vec_pattern_de<'de, D>(deserializer: D) -> Result<Vec<Pattern>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // use the constructor for Pattern to validate the strings
    let patterns: Vec<String> = Vec::deserialize(deserializer)?;
    let patterns: Result<Vec<Pattern>, PatternError> =
        patterns.iter().map(|p| Pattern::new(p)).collect();
    patterns.map_err(serde::de::Error::custom)
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Config {
    pub root: PathBuf,
    #[serde(default)]
    pub locations: Vec<PathBuf>,
    // include regex patterns
    #[serde(
        serialize_with = "vec_pattern_ser",
        deserialize_with = "vec_pattern_de"
    )]
    includes: Vec<Pattern>,
    // exclude patterns
    #[serde(
        serialize_with = "vec_pattern_ser",
        deserialize_with = "vec_pattern_de"
    )]
    excludes: Vec<Pattern>,
    // require ontology names?
    pub require_ontology_names: bool,
    // strict mode (does not allow for any errors in the ontology files)
    pub strict: bool,
    // offline mode (does not fetch remote ontologies)
    pub offline: bool,
    // resolution policy
    pub resolution_policy: String,
    // if true, do not store the ontoenv store on disk
    pub temporary: bool,
    // if true, do not search for ontologies in the search directories
    #[serde(skip_deserializing, default)]
    pub no_search: bool,
}

impl Config {
    /// Creates a new `ConfigBuilder` to construct a `Config`.
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::new()
    }

    /// A convenient constructor for a default offline, non-temporary environment.
    /// Searches for ontologies in the root directory.
    pub fn default(root: PathBuf) -> Result<Self> {
        Config::builder().root(root).offline(true).build()
    }

    /// A convenient constructor for a temporary environment.
    pub fn temporary(root: PathBuf) -> Result<Self> {
        Config::builder().root(root).temporary(true).build()
    }

    /// A convenient constructor for a default configuration that uses default file matching patterns.
    pub fn new_with_default_matches(root: PathBuf) -> Result<Self> {
        Config::builder().root(root).build()
    }

    /// Determines if a file is included in the ontology environment configuration
    pub fn is_included(&self, path: &Path) -> bool {
        for exclude in self.excludes.iter() {
            if exclude.matches_path(path) {
                return false;
            }
        }
        for include in self.includes.iter() {
            if include.matches_path(path) {
                return true;
            }
        }
        // default: if no includes are defined, then include everything
        self.includes.is_empty()
    }

    pub fn save_to_file(&self, file: &Path) -> Result<()> {
        let config_str = serde_json::to_string_pretty(&self)?;
        let mut file = std::fs::File::create(file)?;
        file.write_all(config_str.as_bytes())?;
        Ok(())
    }

    pub fn from_file(file: &Path) -> Result<Self> {
        let file = std::fs::File::open(file)?;
        let reader = BufReader::new(file);
        let config: Config = serde_json::from_reader(reader)?;
        Ok(config)
    }

    /// Prints out the current Config in a clear and readable way for command line output.
    pub fn print(&self) {
        println!("Configuration:");
        println!("  Root: {}", self.root.display());
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
        println!("  Require Ontology Names: {}", self.require_ontology_names);
        println!("  Strict: {}", self.strict);
        println!("  Offline: {}", self.offline);
        println!("  Resolution Policy: {}", self.resolution_policy);
        println!("  Temporary: {}", self.temporary);
        println!("  No Search: {}", self.no_search);
    }
}

/// A builder for creating `Config` instances.
pub struct ConfigBuilder {
    root: Option<PathBuf>,
    locations: Option<Vec<PathBuf>>,
    includes: Option<Vec<String>>,
    excludes: Option<Vec<String>>,
    require_ontology_names: Option<bool>,
    strict: Option<bool>,
    offline: Option<bool>,
    resolution_policy: Option<String>,
    no_search: bool,
    temporary: Option<bool>,
}

impl ConfigBuilder {
    /// Creates a new `ConfigBuilder` with default values.
    pub fn new() -> Self {
        Self {
            root: None,
            locations: None,
            includes: None,
            excludes: None,
            require_ontology_names: None,
            strict: None,
            offline: None,
            resolution_policy: None,
            no_search: false,
            temporary: None,
        }
    }

    /// Sets the root directory for the environment. This is a required field.
    pub fn root(mut self, root: PathBuf) -> Self {
        self.root = Some(root);
        self
    }

    /// Sets the search locations for ontologies. If not set, defaults to the root directory,
    /// unless `no_search` is enabled.
    pub fn locations(mut self, locations: Vec<PathBuf>) -> Self {
        self.locations = Some(locations);
        self
    }

    /// Sets the glob patterns for including files.
    /// Defaults to `["*.ttl", "*.xml", "*.n3"]`.
    pub fn includes<I>(mut self, includes: I) -> Self
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
    {
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
        self.excludes = Some(
            excludes
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        );
        self
    }

    /// Sets whether ontology names are required to be unique. Defaults to `false`.
    pub fn require_ontology_names(mut self, require: bool) -> Self {
        self.require_ontology_names = Some(require);
        self
    }

    /// Sets strict mode. Defaults to `false`.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = Some(strict);
        self
    }

    /// Sets offline mode. Defaults to `false`.
    pub fn offline(mut self, offline: bool) -> Self {
        self.offline = Some(offline);
        self
    }

    /// Sets the resolution policy. Defaults to `"default"`.
    pub fn resolution_policy(mut self, policy: String) -> Self {
        self.resolution_policy = Some(policy);
        self
    }

    /// If set to `true`, no search for local ontologies will be performed. This will override
    /// any specified search locations. Defaults to `false`.
    pub fn no_search(mut self, no_search: bool) -> Self {
        self.no_search = no_search;
        self
    }

    /// Sets temporary mode. If `true`, the environment is not persisted to disk.
    /// Defaults to `false`.
    pub fn temporary(mut self, temporary: bool) -> Self {
        self.temporary = Some(temporary);
        self
    }

    /// Builds the `Config` object.
    ///
    /// # Errors
    ///
    /// Returns an error if the `root` is not set, or if any of the glob patterns are invalid.
    pub fn build(self) -> Result<Config> {
        let root = self
            .root
            .ok_or_else(|| anyhow::anyhow!("Config 'root' is required"))?;

        let locations = if self.no_search {
            vec![]
        } else {
            self.locations.unwrap_or_else(|| vec![root.clone()])
        };

        let includes_str = self.includes.unwrap_or_else(|| {
            vec![
                "*.ttl".to_string(),
                "*.xml".to_string(),
                "*.n3".to_string(),
            ]
        });
        let excludes_str = self.excludes.unwrap_or_default();

        let includes = includes_str
            .iter()
            .map(|p| Pattern::new(p))
            .collect::<Result<Vec<_>, _>>()?;

        let excludes = excludes_str
            .iter()
            .map(|p| Pattern::new(p))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Config {
            root,
            locations,
            includes,
            excludes,
            require_ontology_names: self.require_ontology_names.unwrap_or(false),
            strict: self.strict.unwrap_or(false),
            offline: self.offline.unwrap_or(false),
            resolution_policy: self
                .resolution_policy
                .unwrap_or_else(|| DefaultPolicy.policy_name().to_string()),
            temporary: self.temporary.unwrap_or(false),
            no_search: self.no_search,
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
