use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use anyhow::Result;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Serialize, Deserialize, Debug)]
pub struct OntologyConfig {
    pub uri: String,
    pub file: PathBuf,
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EnvironmentConfig {
    pub ontologies: Vec<OntologyConfig>,
}

impl EnvironmentConfig {
    pub fn from_file(file_path: &Path) -> Result<Self> {
        let file = File::open(file_path)?;
        let reader = BufReader::new(file);
        let config: EnvironmentConfig = serde_json::from_reader(reader)?;
        Ok(config)
    }
}
