//! Backend abstraction over the in-process `FlowEngine` and the daemon
//! `RemoteEngine`.
//!
//! Phase 2 introduces this trait to let consumers (Wallet, future Rust apps)
//! switch between modes via a single env var (`HR_FLOW_BACKEND=embedded|remote`)
//! without changing call sites. Phase 6 will retire `EmbeddedEngine` once
//! every app runs in callback mode.

use async_trait::async_trait;
use serde_json::Value;

use crate::engine::{FlowEngine, RunResult};
use crate::error::FlowResult;

#[async_trait]
pub trait Engine: Send + Sync {
    async fn run(&self, name: &str, input: Value) -> FlowResult<RunResult>;
    async fn run_with_trigger(
        &self,
        name: &str,
        input: Value,
        trigger: &str,
    ) -> FlowResult<RunResult>;
    async fn replay(&self, run_id: &str) -> FlowResult<RunResult>;
}

/// `EmbeddedEngine` is the historical name we use for `FlowEngine` once the
/// trait is in play. Kept as an alias so apps can write
/// `let engine: Box<dyn Engine> = Box::new(embedded_engine)`.
pub type EmbeddedEngine = FlowEngine;

#[async_trait]
impl Engine for FlowEngine {
    async fn run(&self, name: &str, input: Value) -> FlowResult<RunResult> {
        FlowEngine::run(self, name, input).await
    }

    async fn run_with_trigger(
        &self,
        name: &str,
        input: Value,
        trigger: &str,
    ) -> FlowResult<RunResult> {
        FlowEngine::run_with_trigger(self, name, input, trigger).await
    }

    async fn replay(&self, run_id: &str) -> FlowResult<RunResult> {
        FlowEngine::replay(self, run_id).await
    }
}

/// Backend selected by the `HR_FLOW_BACKEND` env var.
///
/// Defaults to `Embedded` so apps that don't opt in (yet) keep their current
/// behaviour. Set `HR_FLOW_BACKEND=remote` in `.env` to switch over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Embedded,
    Remote,
}

impl Backend {
    pub fn from_env() -> Self {
        match std::env::var("HR_FLOW_BACKEND")
            .ok()
            .as_deref()
            .map(str::to_ascii_lowercase)
        {
            Some(s) if s == "remote" => Self::Remote,
            _ => Self::Embedded,
        }
    }
}

/// Build a backend per `HR_FLOW_BACKEND`. The `embedded_factory` closure is
/// only called when the env var resolves to `embedded` (default), so apps can
/// avoid building the in-process registries when the daemon takes the relay.
pub fn engine_from_env<F>(
    slug: impl Into<String>,
    embedded_factory: F,
) -> FlowResult<Box<dyn Engine>>
where
    F: FnOnce() -> FlowResult<FlowEngine>,
{
    match Backend::from_env() {
        Backend::Embedded => Ok(Box::new(embedded_factory()?) as Box<dyn Engine>),
        Backend::Remote => {
            let remote = crate::remote_engine::RemoteEngine::from_env(slug.into())?;
            Ok(Box::new(remote) as Box<dyn Engine>)
        }
    }
}
