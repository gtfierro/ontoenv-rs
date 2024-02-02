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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Config {
    pub root: PathBuf,
    pub search_directories: Vec<PathBuf>,
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
}

impl Config {
    // new constructor where includes and excludes accept iterators of &str
    pub fn new<I, J>(
        root: PathBuf,
        search_directories: Vec<PathBuf>,
        includes: I,
        excludes: J,
        require_ontology_names: bool,
    ) -> Result<Self>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
        J: IntoIterator,
        J::Item: AsRef<str>,
    {
        let mut config = Config {
            root,
            search_directories,
            includes: vec![],
            excludes: vec![],
            require_ontology_names,
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
            config.includes.push(Pattern::new("*")?);
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

    pub fn new_with_default_matches(
        root: PathBuf,
        search_directories: Vec<PathBuf>,
        require_ontology_names: bool,
    ) -> Result<Self> {
        let includes = vec!["*/**/*.ttl", "*/**/*.xml", "*/**/*.n3"];
        self::Config::new::<Vec<&str>, Vec<&str>>(
            root,
            search_directories,
            includes,
            vec![],
            require_ontology_names,
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
        let config = serde_json::from_reader(reader)?;
        Ok(config)
    }
}
