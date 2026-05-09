//! Run replay.
//!
//! v1 = full replay: re-execute the flow with the same `input_json` as the
//! original run, producing a new `_flow_runs` row. Replay-from-step (cached
//! step outputs, à la Temporal) is deferred — the run-step rows already
//! carry enough information to enable it later.
//!
//! Phase 1 is a stub.

#![allow(dead_code)]

use crate::error::FlowResult;

pub async fn replay_full(_run_id: i64) -> FlowResult<i64> {
    Ok(0)
}
