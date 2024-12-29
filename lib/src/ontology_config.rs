use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug)]
pub struct OntologyConfig {
    pub location: OntologyLocation,
    pub version: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum OntologyLocation {
    File { file: PathBuf },
    Uri { uri: String },
}
