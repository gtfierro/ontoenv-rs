//! Shared option types that replace boolean flag parameters in the Rust API.

/// Controls how an add operation handles existing data.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum Overwrite {
    /// Replace any existing ontology with the incoming data.
    Allow,
    /// Preserve the existing ontology and add only if it is new.
    Preserve,
}

impl Overwrite {
    pub fn as_bool(self) -> bool {
        matches!(self, Overwrite::Allow)
    }
}

impl From<bool> for Overwrite {
    fn from(value: bool) -> Self {
        if value {
            Overwrite::Allow
        } else {
            Overwrite::Preserve
        }
    }
}

impl From<Overwrite> for bool {
    fn from(value: Overwrite) -> Self {
        value.as_bool()
    }
}

/// Indicates whether the caller wants to force a refresh or reuse cached data.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum RefreshStrategy {
    /// Always refetch the ontology, even if a cached copy exists.
    Force,
    /// Reuse cached data when available and fresh.
    UseCache,
}

impl RefreshStrategy {
    pub fn is_force(self) -> bool {
        matches!(self, RefreshStrategy::Force)
    }
}

impl From<bool> for RefreshStrategy {
    fn from(value: bool) -> Self {
        if value {
            RefreshStrategy::Force
        } else {
            RefreshStrategy::UseCache
        }
    }
}

/// Represents the cache usage policy captured in the configuration.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
#[derive(Default)]
pub enum CacheMode {
    Enabled,
    #[default]
    Disabled,
}

impl CacheMode {
    pub fn is_enabled(self) -> bool {
        matches!(self, CacheMode::Enabled)
    }
}


impl From<bool> for CacheMode {
    fn from(value: bool) -> Self {
        if value {
            CacheMode::Enabled
        } else {
            CacheMode::Disabled
        }
    }
}

impl From<CacheMode> for bool {
    fn from(value: CacheMode) -> Self {
        value.is_enabled()
    }
}
