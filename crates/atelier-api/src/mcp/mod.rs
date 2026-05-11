//! MCP (Model Context Protocol) handlers ported from homeroute's
//! `hr-orchestrator::mcp` after the Atelier cutover. The internal modules
//! mirror the old layout one-for-one — adaptations limited to:
//! - sibling-module paths (`use super::scaffold;`)
//! - optional `EdgeClient` (Atelier on CloudMaster cannot reach Medion's hr-edge
//!   socket; route mutations are best-effort)
//! - removal of unused `log_store` field

pub mod apps_ops;
pub mod dv_ops;
pub mod scaffold;
