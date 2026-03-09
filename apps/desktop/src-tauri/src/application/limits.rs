pub(crate) const DEFAULT_WINDOW_LIMIT: usize = 100;
pub(crate) const MAX_WINDOW_LIMIT: usize = 5_000;
pub(crate) const DEFAULT_GRAPH_MAX_NODES: usize = 2_000;
pub(crate) const MAX_GRAPH_MAX_NODES: usize = 2_000_000;

pub(crate) fn normalize_window_limit(requested_limit: Option<usize>) -> usize {
    requested_limit
        .unwrap_or(DEFAULT_WINDOW_LIMIT)
        .clamp(1, MAX_WINDOW_LIMIT)
}

pub(crate) fn normalize_graph_max_nodes(requested_max_nodes: usize) -> usize {
    if requested_max_nodes == 0 {
        DEFAULT_GRAPH_MAX_NODES
    } else {
        requested_max_nodes
    }
    .clamp(1, MAX_GRAPH_MAX_NODES)
}
