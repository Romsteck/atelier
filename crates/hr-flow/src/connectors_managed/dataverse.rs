//! `dataverse` connector — wraps `hr-dataverse` for the app's own database.
//!
//! Phase 2 will implement `find` / `insert` / `update` / `delete` / `expand`
//! against the app's Postgres pool.

use async_trait::async_trait;
use serde_json::Value;

use crate::connector::Connector;
use crate::error::{FlowError, FlowResult};

pub struct DataverseConnector {
    // Pool handle will be wired in phase 2.
}

impl DataverseConnector {
    pub fn new() -> Self { Self {} }
}

#[async_trait]
impl Connector for DataverseConnector {
    fn name(&self) -> &str { "dataverse" }

    async fn call(&self, op: &str, _params: Value) -> FlowResult<Value> {
        Err(FlowError::UnknownOperation {
            connector: "dataverse".into(),
            op: op.to_string(),
        })
    }
}
