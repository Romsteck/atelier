//! Managed connectors: `dataverse`, `homeroute`, `http`.
//!
//! Phase 1 ships only the type stubs and module layout — the actual op
//! handlers (find/insert/update on dataverse, GET/POST on http, …) land
//! in phase 2 once the executor can invoke them.

pub mod dataverse;
pub mod homeroute;
pub mod http;
