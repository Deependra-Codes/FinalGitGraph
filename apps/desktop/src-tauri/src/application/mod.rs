use std::path::Path;

pub mod commands;
pub(crate) mod graph_queries;
pub(crate) mod index_session_planner;
pub mod indexer;
mod indexer_pipeline;
pub(crate) mod ingest_store;
pub(crate) mod limits;
pub(crate) mod query_graph_cache;

pub use ingest_store::{lsm_shadow_metrics_snapshot, LsmShadowMetricsSnapshot};

pub fn invalidate_query_graph_cache_for_db_path(db_path: &Path) {
    query_graph_cache::invalidate_query_graph_cache(db_path);
}
