//! `hr-flow-daemon` — multi-stack flow daemon for HomeRoute.
//!
//! Standalone HTTP service running on Medion (port 4002, loopback only) that
//! orchestrates flow runs for all HomeRoute apps regardless of their stack.
//! Custom actions and connectors are invoked via HTTP callback into the
//! target app — no cross-compilation, no port of the engine to TS.
//!
//! Architecture invariants (cf. plan-hr-flowd.md, decision §A):
//! - non-blocking : every run dispatches into its own `tokio::spawn` task
//! - lock-free registry : `Arc<ArcSwap<Registry>>` for hot-reload (no STW)
//! - panic isolation : each callback wrapped in `catch_unwind`
//! - per-slug backpressure : `Semaphore` keyed by slug
//! - single source of truth : runs persist on disk via `JsonRunStore`,
//!   the Atelier API viewer reads the same files (no HTTP hop for reads).

pub mod auth;
pub mod callback;
pub mod engine_factory;
pub mod error;
pub mod registry;
pub mod routes;
pub mod state;
pub mod supervisor;

pub use error::{DaemonError, DaemonResult};
pub use state::DaemonState;
