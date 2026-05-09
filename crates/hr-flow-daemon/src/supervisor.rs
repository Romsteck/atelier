//! Run dispatch + lifecycle.
//!
//! Architecture invariant (cf. plan-hr-flowd.md §A): one run never blocks
//! another. `dispatch_run` :
//!   1. Acquires the per-slug semaphore (429 if saturated)
//!   2. Acquires the optional global semaphore
//!   3. Builds a `FlowEngine` for the (slug, flow_name) tuple
//!   4. Spawns the engine's run inside `AssertUnwindSafe(_).catch_unwind()`,
//!      wrapped with `tokio::time::timeout`
//!   5. Awaits the JoinHandle, persists RunHandle in the active-runs DashMap
//!      so /cancel and the supervisor stats can see it
//!
//! On panic: returns `FlowError::Internal("panic: ...")`, daemon stays alive.
//! On timeout: returns `FlowError::Internal("run_timeout_ms")`, the engine's
//!   future is dropped (best effort cancel — connectors may still finish).

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures::FutureExt;
use hr_flow::{FlowError, FlowResult, RunResult};
use reqwest::Client;
use serde_json::Value;
use std::panic::AssertUnwindSafe;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

use crate::engine_factory::{build_engine_for_flow, EngineFactoryInput};
use crate::error::{DaemonError, DaemonResult};
use crate::state::DaemonState;

#[derive(Debug, Clone)]
pub struct RunHandle {
    /// Internal id assigned at spawn (before the engine generates its own
    /// run_id). Used as the DashMap key so /cancel doesn't need to wait for
    /// the engine to emit a UUID.
    pub dispatch_id: String,
    pub slug: String,
    pub flow_name: String,
    pub started_at: DateTime<Utc>,
    pub cancel: CancellationToken,
}

#[instrument(skip(state, input), fields(dispatch_id))]
pub async fn dispatch_run(
    state: Arc<DaemonState>,
    slug: String,
    flow_name: String,
    input: Value,
    trigger: &str,
    http: Client,
    callback_timeout_ms: u64,
    run_timeout_ms: u64,
) -> DaemonResult<RunResult> {
    let dispatch_id = Uuid::new_v4().to_string();
    tracing::Span::current().record("dispatch_id", dispatch_id.as_str());

    // Per-slug backpressure: try_acquire so saturation is observable as 429
    // rather than hidden behind unbounded queueing.
    let slug_sem = state.semaphore_for(&slug);
    let _slug_permit = slug_sem
        .clone()
        .try_acquire_owned()
        .map_err(|_| DaemonError::Overloaded { slug: slug.clone() })?;

    // Defense-in-depth global cap (acquired with await — if global is full
    // we'd rather queue briefly than 429 here, since per-slug already serves
    // as the user-visible limit).
    let _global_permit = match &state.global_semaphore {
        Some(g) => Some(g.clone().acquire_owned().await.map_err(|e| {
            DaemonError::Internal(format!("global semaphore closed: {e}"))
        })?),
        None => None,
    };

    let registry = state.registry.load_full();
    let flow = registry
        .flows
        .get(&(slug.clone(), flow_name.clone()))
        .cloned()
        .ok_or_else(|| {
            DaemonError::NotFound(format!("flow `{}` for slug `{}`", flow_name, slug))
        })?;

    let engine = build_engine_for_flow(EngineFactoryInput {
        slug: &slug,
        flow: &flow,
        registry: &registry,
        apps_runtime_root: &state.apps_runtime_root,
        http,
        callback_timeout_ms,
    })?;

    let cancel = CancellationToken::new();
    let handle = RunHandle {
        dispatch_id: dispatch_id.clone(),
        slug: slug.clone(),
        flow_name: flow_name.clone(),
        started_at: Utc::now(),
        cancel: cancel.clone(),
    };
    state.runs.insert(dispatch_id.clone(), handle);

    let trigger_owned = trigger.to_string();
    let flow_name_owned = flow_name.clone();
    let timeout = Duration::from_millis(run_timeout_ms.min(state.step_timeout_max_ms.saturating_mul(20)).max(1_000));

    info!(slug = %slug, flow = %flow_name_owned, %trigger_owned, "dispatch: spawning run");

    let join: JoinHandle<Result<FlowResult<RunResult>, Box<dyn std::any::Any + Send>>> =
        tokio::spawn(async move {
            // Catch panics so a buggy connector cannot kill the worker.
            let fut = AssertUnwindSafe(async move {
                tokio::time::timeout(timeout, engine.run_with_trigger(&flow_name_owned, input, &trigger_owned))
                    .await
                    .map_err(|_| FlowError::Internal(format!("run_timeout: {}ms", timeout.as_millis())))?
            })
            .catch_unwind();
            fut.await
        });

    // Await join + map errors. We deliberately do NOT use cancel.cancel() to
    // abort here — Phase 1 cancel is best-effort via JoinHandle::abort triggered
    // by the /cancel route, not the dispatch path.
    let outcome = join.await;
    state.runs.remove(&dispatch_id);

    match outcome {
        Ok(Ok(Ok(result))) => {
            info!(
                slug = %slug,
                flow = %flow_name,
                run_id = %result.run_id,
                status = ?result.status,
                duration_ms = result.duration_ms,
                "dispatch: run completed"
            );
            Ok(result)
        }
        Ok(Ok(Err(flow_err))) => {
            warn!(slug = %slug, flow = %flow_name, ?flow_err, "dispatch: flow error");
            Err(DaemonError::Flow(flow_err))
        }
        Ok(Err(panic_payload)) => {
            let msg = panic_message(panic_payload);
            error!(slug = %slug, flow = %flow_name, %msg, "dispatch: run panicked");
            Err(DaemonError::Internal(format!("run_panicked: {msg}")))
        }
        Err(join_err) => {
            error!(slug = %slug, flow = %flow_name, ?join_err, "dispatch: tokio join error");
            Err(DaemonError::Internal(format!("join_error: {join_err}")))
        }
    }
}

/// Best-effort cancel by dispatch_id.
pub fn cancel_dispatch(state: &DaemonState, dispatch_id: &str) -> bool {
    if let Some(entry) = state.runs.get(dispatch_id) {
        entry.cancel.cancel();
        true
    } else {
        false
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_string()
    }
}
