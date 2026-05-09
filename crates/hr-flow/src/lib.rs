//! `hr-flow` — per-app flow engine for HomeRoute apps.
//!
//! Inspired by Power Automate but native Rust and embedded in each consuming
//! app. Flows are declarative TOML files (`{slug}/src/flows/*.toml`)
//! executed by a `FlowEngine` linked into the app binary. Every step
//! (connector op, primitive, custom action) is recorded as a tree node in a
//! pluggable `RunStore` for full traceability — the default JSON-on-disk
//! store puts one file per run under `runs/`.

pub mod connector;
pub mod connectors_managed;
pub mod definition;
pub mod engine;
pub mod error;
pub mod executor;
pub mod expr;
pub mod persist;

pub use connector::Connector;
pub use definition::{parse_flow_toml, FlowDef, StepDef, StepKind};
pub use engine::{FlowEngine, FlowEngineBuilder, RunError, RunResult, RunStatus};
pub use error::{FlowError, FlowResult};
pub use executor::StepRecord;
pub use persist::{JsonRunStore, RunDoc, RunErrorDoc, RunStore};

pub use connectors_managed::http::HttpConnector;

pub use hr_flow_macros::flow_action;
