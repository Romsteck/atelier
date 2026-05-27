//! MCP (Model Context Protocol) handlers ported from homeroute's
//! `hr-orchestrator::mcp` after the Atelier cutover. The internal modules
//! mirror the old layout one-for-one — adaptations limited to:
//! - sibling-module paths (`use super::scaffold;`)
//! - optional `EdgeClient` (`None` if hr-edge's IPC socket is unreachable;
//!   route mutations are best-effort in that case)
//! - removal of unused `log_store` field

pub mod apps_ops;
pub mod dv_ops;
pub mod scaffold;
