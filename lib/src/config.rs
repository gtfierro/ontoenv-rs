//! Defines the configuration structures for the OntoEnv environment.
//! This includes the main `Config` struct and related structs for ontology locations and environment setup.

use crate::ontology::OntologyLocation;
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

#[derive(Serialize, Deserialize, Debug)]
pub struct EnvironmentConfig {
    pub ontologies: Vec<OntologyConfig>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct OntologyConfig {
    #[serde(flatten)]
    pub location: OntologyLocation,
    pub version: Option<String>,
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
}

#[derive(Debug, Default)]
pub struct ConfigBuilder {
    root: Option<PathBuf>,
    locations: Option<Vec<PathBuf>>,
    includes: Option<Vec<String>>,
    excludes: Option<Vec<String>>,
    require_ontology_names: Option<bool>,
    strict: Option<bool>,
    offline: Option<bool>,
    resolution_policy: Option<String>,
    temporary: Option<bool>,
    no_search: bool,
}

impl ConfigBuilder {
    pub fn locations(mut self, locations: Vec<PathBuf>) -> Self {
        self.locations = Some(locations);
        self
    }

    pub fn includes(mut self, includes: Vec<String>) -> Self {
        self.includes = Some(includes);
        self
    }

    pub fn excludes(mut self, excludes: Vec<String>) -> Self {
        self.excludes = Some(excludes);
        self
    }

    pub fn require_ontology_names(mut self, require: bool) -> Self {
        self.require_ontology_names = Some(require);
        self
    }

    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = Some(strict);
        self
    }

    pub fn offline(mut self, offline: bool) -> Self {
        self.offline = Some(offline);
        self
    }

    pub fn resolution_policy(mut self, policy: String) -> Self {
        self.resolution_policy = Some(policy);
        self
    }

    pub fn temporary(mut self, temporary: bool) -> Self {
        self.temporary = Some(temporary);
        self
    }

    pub fn no_search(mut self, no_search: bool) -> Self {
        self.no_search = no_search;
        self
    }

    pub fn build(self) -> Result<Config> {
        let root = self
            .root
            .ok_or_else(|| anyhow::anyhow!("Config 'root' is required"))?;

        let locations = self.locations.unwrap_or_else(|| {
            if self.no_search {
                vec![]
            } else {
                vec![root.clone()]
            }
        });

        let include_strs = self.includes.unwrap_or_else(|| {
            vec![
                "*.ttl".to_string(),
                "*.xml".to_string(),
                "*.n3".to_string(),
            ]
        });

        let mut includes = Vec::new();
        for s in include_strs {
            includes.push(Pattern::new(&s)?);
        }

        let exclude_strs = self.excludes.unwrap_or_default();
        let mut excludes = Vec::new();
        for s in exclude_strs {
            excludes.push(Pattern::new(&s)?);
        }

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
        })
    }
}

impl Config {
    pub fn builder(root: PathBuf) -> ConfigBuilder {
        ConfigBuilder {
            root: Some(root),
            ..Default::default()
        }
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
