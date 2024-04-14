// OfflineRetrieval error

use std::fmt;

#[derive(Debug)]
pub struct OfflineRetrievalError {
    pub file: String,
}

impl fmt::Display for OfflineRetrievalError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Failed to fetch ontology from {}", self.file)
    }
}

impl std::error::Error for OfflineRetrievalError {}
