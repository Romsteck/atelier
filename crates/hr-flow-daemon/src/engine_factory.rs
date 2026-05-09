//! Build a `FlowEngine` for a given (slug, flow_name) by wiring:
//! - the managed connectors the daemon owns (`http`)
//! - a `RemoteConnector` for every other connector name referenced by the
//!   flow's steps (forwards each `op` to the app's callback URL)
//! - a remote-action closure for every `kind = "action"` step
//! - a `JsonRunStore` rooted at `${runtime_root}/{slug}/runs/`
//!
//! Connectors `dataverse` / `homeroute` shipped in `hr-flow::connectors_managed`
//! are stubs in v1 (always error). The daemon does NOT register them — a flow
//! using them is treated as referring to an app-side custom connector and is
//! routed via callback. This matches reality (Wallet's `dataverse` is a custom
//! connector that wraps the gateway, not the engine's built-in stub).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use hr_flow::{
    definition::StepKind, FlowDef, FlowEngine, FlowEngineBuilder, FlowError, FlowResult,
    HttpConnector, JsonRunStore,
};
use reqwest::Client;
use tracing::{debug, warn};

use crate::callback::{remote_action, RemoteConnector};
use crate::registry::{AppEntry, Registry};

/// Names of connectors the daemon owns and registers natively.
const MANAGED_LOCALLY: &[&str] = &["http"];

pub struct EngineFactoryInput<'a> {
    pub slug: &'a str,
    pub flow: &'a FlowDef,
    pub registry: &'a Registry,
    pub apps_runtime_root: &'a std::path::Path,
    pub http: Client,
    pub callback_timeout_ms: u64,
}

pub fn build_engine_for_flow(args: EngineFactoryInput<'_>) -> FlowResult<FlowEngine> {
    let app = args
        .registry
        .apps
        .get(args.slug)
        .ok_or_else(|| FlowError::Internal(format!("unknown slug `{}`", args.slug)))?;

    let runs_dir: PathBuf = args.apps_runtime_root.join(args.slug).join("runs");
    let store = Arc::new(JsonRunStore::new(&runs_dir).map_err(|e| {
        FlowError::Persistence(format!("create runs dir {}: {e}", runs_dir.display()))
    })?);

    let mut builder = FlowEngineBuilder::new();
    builder.with_store(store);
    builder.register_connector("http", Arc::new(HttpConnector::new()));

    // Discover connector + action names referenced by this flow.
    let (connectors, actions) = discover_step_handlers(args.flow);

    for connector_name in connectors {
        if MANAGED_LOCALLY.contains(&connector_name.as_str()) {
            continue;
        }
        match RemoteConnector::new(
            &connector_name,
            app,
            args.http.clone(),
            args.callback_timeout_ms,
        ) {
            Ok(c) => {
                debug!(slug = args.slug, connector = %connector_name, "engine: remote connector wired");
                builder.register_connector(&connector_name, c);
            }
            Err(e) => {
                warn!(slug = args.slug, connector = %connector_name, ?e, "engine: cannot wire remote connector (callback url/token missing)");
                return Err(e);
            }
        }
    }

    for action_name in actions {
        let handler = remote_action(
            action_name.clone(),
            app,
            args.http.clone(),
            args.callback_timeout_ms,
        )?;
        debug!(slug = args.slug, action = %action_name, "engine: remote action wired");
        builder.register_action(action_name.clone(), move |v| {
            let h = handler.clone();
            async move { h(v).await }
        });
    }

    builder.register_flow(args.flow.clone());
    builder.build()
}

/// Walk the flow's steps and collect all distinct connector names and action
/// names that the engine will need handlers for.
fn discover_step_handlers(flow: &FlowDef) -> (HashSet<String>, HashSet<String>) {
    let mut connectors = HashSet::new();
    let mut actions = HashSet::new();
    for step in &flow.steps {
        match &step.kind {
            StepKind::Connector { connector, .. } => {
                connectors.insert(connector.clone());
            }
            StepKind::Action { action } => {
                actions.insert(action.clone());
            }
            _ => {}
        }
    }
    (connectors, actions)
}

/// Lookup helper used by replay routes that don't have the flow_name in the
/// query string but only the run_id.
pub fn flow_name_from_run_doc(doc: &hr_flow::RunDoc) -> &str {
    &doc.flow_name
}

#[allow(unused)]
pub use std::convert::Infallible as _Infallible; // keep doc-link tidy

#[allow(unused)]
fn _docs_app_entry_unused(_: &AppEntry) {}
