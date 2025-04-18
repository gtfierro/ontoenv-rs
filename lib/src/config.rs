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

impl Config {
    // new constructor where includes and excludes accept iterators of &str
    pub fn new<I, J, K>(
        root: PathBuf,
        locations: Option<K>,
        includes: I,
        excludes: J,
        require_ontology_names: bool,
        strict: bool,
        offline: bool,
        resolution_policy: String,
        no_search: bool,
        temporary: bool,
    ) -> Result<Self>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
        J: IntoIterator,
        J::Item: AsRef<str>,
        K: IntoIterator<Item = PathBuf>,
    {
        // if search directories are empty, add the root. Otherwise, use the provided search directories
        // if no_search is true, then do not default to the root directory
        let locations = locations
            .map(|dirs| dirs.into_iter().collect())
            .unwrap_or_else(|| {
                if no_search {
                    vec![]
                } else {
                    vec![root.clone()]
                }
            });

        let mut config = Config {
            root,
            locations,
            includes: vec![],
            excludes: vec![],
            require_ontology_names,
            strict,
            offline,
            resolution_policy,
            temporary,
        };
        let includes: Vec<String> = includes
            .into_iter()
            .map(|s| s.as_ref().to_owned())
            .collect();
        let excludes: Vec<String> = excludes
            .into_iter()
            .map(|s| s.as_ref().to_owned())
            .collect();
        if includes.is_empty() {
            config.includes.push(Pattern::new("*.ttl")?);
        }
        for include in includes {
            let pat = Pattern::new(&include)?;
            config.includes.push(pat);
        }
        for exclude in excludes {
            let pat = Pattern::new(&exclude)?;
            config.excludes.push(pat);
        }
        Ok(config)
    }

    pub fn default_offline<K>(root: PathBuf, locations: Option<K>, temporary: bool) -> Result<Self>
    where
        K: IntoIterator<Item = PathBuf>,
    {
        Self::new_with_default_matches(root, locations, false, false, true, temporary)
    }

    pub fn new_with_default_matches<K>(
        root: PathBuf,
        locations: Option<K>,
        require_ontology_names: bool,
        strict: bool,
        offline: bool,
        temporary: bool,
    ) -> Result<Self>
    where
        K: IntoIterator<Item = PathBuf>,
    {
        let includes = vec!["*.ttl", "*.xml", "*.n3"];
        Self::new::<Vec<&str>, Vec<&str>, Vec<PathBuf>>(
            root,
            locations.map(|dirs| dirs.into_iter().collect()),
            includes,
            vec![],
            require_ontology_names,
            strict,
            offline,
            DefaultPolicy.policy_name().to_string(),
            false,
            temporary,
        )
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
        let mut config: Config = serde_json::from_reader(reader)?;

        if config.locations.is_empty() {
            config.locations = vec![config.root.clone()];
        }
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
            println!("    - {}", pat);
        }
        if !self.excludes.is_empty() {
            println!("  Exclude Patterns:");
            for pat in &self.excludes {
                println!("    - {}", pat);
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
