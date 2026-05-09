//! Connector trait.
//!
//! A connector exposes a finite set of named operations. The engine
//! invokes one via `call(op, params)`; managed connectors implement this
//! against fixed APIs (postgres-dataverse, homeroute IPC, raw HTTP) while
//! per-app custom connectors plug in third-party services.
//!
//! Operations always take a JSON value (the step's resolved `params`) and
//! return a JSON value — same shape as `ActionFn`, so the executor only has
//! one dispatch model to worry about.

use async_trait::async_trait;
use serde_json::Value;

use crate::error::FlowResult;

#[async_trait]
pub trait Connector: Send + Sync {
    /// Stable connector name (e.g. `"dataverse"`, `"http"`). Used for
    /// diagnostics; the engine indexes connectors by the name passed to
    /// `register_connector`.
    fn name(&self) -> &str;

    /// Execute one operation. The implementation is responsible for
    /// validating its own input shape and for returning a value the rest
    /// of the flow can reference via `{{ steps.<id>.output.* }}`.
    async fn call(&self, op: &str, params: Value) -> FlowResult<Value>;
}
