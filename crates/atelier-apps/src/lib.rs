//! atelier-apps — Application management for Atelier.

pub mod context;
pub mod metrics;
pub mod port_registry;
pub mod registry;
pub mod supervisor;
pub mod types;

pub use context::ContextGenerator;
pub use port_registry::PortRegistry;
pub use registry::AppRegistry;
pub use supervisor::{AppSupervisor, ProcessStatus};
pub use types::*;
