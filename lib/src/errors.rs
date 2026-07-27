//! Defines custom error types used within the OntoEnv library, such as errors related to offline mode.

// OfflineRetrieval error

use std::fmt;

#[derive(Debug)]
pub struct OfflineRetrievalError {
    pub file: String,
}

impl fmt::Display for OfflineRetrievalError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "OFFLINE enabled: Failed to fetch ontology from {}",
            self.file
        )
    }
}

impl std::error::Error for OfflineRetrievalError {}

#[derive(Debug)]
pub struct ExternalStoreChangedError {
    pub message: String,
}

impl fmt::Display for ExternalStoreChangedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ExternalStoreChangedError: {}", self.message)
    }
}

impl std::error::Error for ExternalStoreChangedError {}

#[derive(Debug)]
/// Indicates that an interrupted mutation marker prevents a normal catalog open.
///
/// Call `OntoEnv::recover` (or the corresponding language-binding API) to
/// rebuild the catalog from the authoritative graph store.
pub struct CatalogRecoveryError {
    pub message: String,
}

impl fmt::Display for CatalogRecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OntoEnv recovery required: {}", self.message)
    }
}

impl std::error::Error for CatalogRecoveryError {}

#[derive(Debug)]
pub struct StoreCapabilityError {
    pub message: String,
}

impl fmt::Display for StoreCapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Store capability unavailable: {}", self.message)
    }
}

impl std::error::Error for StoreCapabilityError {}
