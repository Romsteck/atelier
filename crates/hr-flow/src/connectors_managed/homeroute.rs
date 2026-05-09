//! `homeroute` connector — IPC against `hr-orchestrator` and the wider
//! HomeRoute platform.
//!
//! Phase 2 will implement ops like `apps.list`, `dns.records`, `proxy.routes`.

use async_trait::async_trait;
use serde_json::Value;

use crate::connector::Connector;
use crate::error::{FlowError, FlowResult};

pub struct HomeRouteConnector {}

impl HomeRouteConnector {
    pub fn new() -> Self { Self {} }
}

#[async_trait]
impl Connector for HomeRouteConnector {
    fn name(&self) -> &str { "homeroute" }

    async fn call(&self, op: &str, _params: Value) -> FlowResult<Value> {
        Err(FlowError::UnknownOperation {
            connector: "homeroute".into(),
            op: op.to_string(),
        })
    }
}
