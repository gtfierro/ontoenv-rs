//! Keep managed candidate ownership separate from external-store bookkeeping.
use crate::config::Config;
use crate::environment::Environment;
use crate::io::GraphIO;
use crate::snapshot_io::SnapshotGraphIO;
use crate::snapshots::{EnvironmentSnapshot, PruneReport};
use anyhow::Result;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

pub(crate) enum Backend {
    Managed(Box<SnapshotGraphIO>),
    Adapter(Box<Adapter>),
}

pub(crate) struct Adapter {
    io: Box<dyn GraphIO>,
    environment: Environment,
    config: Config,
}

impl Backend {
    pub(crate) fn adapter(io: Box<dyn GraphIO>, environment: Environment, config: Config) -> Self {
        Self::Adapter(Box::new(Adapter {
            io,
            environment,
            config,
        }))
    }

    pub(crate) fn environment(&self) -> &Environment {
        match self {
            Self::Managed(io) => io.environment(),
            Self::Adapter(adapter) => &adapter.environment,
        }
    }

    pub(crate) fn environment_mut(&mut self) -> &mut Environment {
        match self {
            Self::Managed(io) => &mut io.candidate_mut().environment,
            Self::Adapter(adapter) => &mut adapter.environment,
        }
    }

    pub(crate) fn config(&self) -> &Config {
        match self {
            Self::Managed(io) => io.config(),
            Self::Adapter(adapter) => &adapter.config,
        }
    }

    pub(crate) fn config_mut(&mut self) -> &mut Config {
        match self {
            Self::Managed(io) => io.config_mut(),
            Self::Adapter(adapter) => &mut adapter.config,
        }
    }

    pub(crate) fn publish(&mut self) -> Result<bool> {
        match self {
            Self::Managed(io) => {
                io.publish()?;
                Ok(true)
            }
            Self::Adapter(_) => Ok(false),
        }
    }

    pub(crate) fn is_managed(&self) -> bool {
        matches!(self, Self::Managed(_))
    }

    pub(crate) fn snapshot(&self) -> Option<Arc<EnvironmentSnapshot>> {
        match self {
            Self::Managed(io) => Some(io.snapshot()),
            Self::Adapter(_) => None,
        }
    }

    pub(crate) fn refresh(&mut self) -> Result<bool> {
        match self {
            Self::Managed(io) => io.refresh(),
            Self::Adapter(_) => {
                anyhow::bail!("This backend does not expose immutable environment refresh")
            }
        }
    }

    pub(crate) fn prune(&mut self) -> Result<PruneReport> {
        match self {
            Self::Managed(io) => io.prune(),
            Self::Adapter(_) => anyhow::bail!("Cleanup requires a managed snapshot writer"),
        }
    }
}

impl Deref for Backend {
    type Target = dyn GraphIO;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Managed(io) => io.as_ref(),
            Self::Adapter(adapter) => adapter.io.as_ref(),
        }
    }
}

impl DerefMut for Backend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Managed(io) => io.as_mut(),
            Self::Adapter(adapter) => adapter.io.as_mut(),
        }
    }
}
