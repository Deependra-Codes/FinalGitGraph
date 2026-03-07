use std::collections::{BTreeSet, HashMap, VecDeque};

use rusqlite::Connection;

use crate::adapters::db::{
    count_commits, load_commit_shas_for_rowids, load_neighbor_rowids,
    load_neighbor_rowids_for_sources, load_pull_neighbor_rowids,
};
use crate::domain::models::{
    GraphDirection, GraphNodeVisit, GraphTraversalResponse, GraphTraversalStrategy,
    ShortestPathResponse,
};
use crate::support::error::{AppError, AppResult};

const DEFAULT_GRAPH_MAX_NODES: usize = 200;
const MAX_GRAPH_MAX_NODES: usize = 10_000;
const BFS_PULL_MIN_FRONTIER: usize = 256;
const BFS_PULL_MIN_REMAINING: usize = 1_024;
const BFS_PULL_FRONTIER_TO_REMAINING_RATIO: usize = 8;
type NodeId = usize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BfsMode {
    Push,
    Pull,
}

#[derive(Default)]
struct CompactNodeMap {
    rowid_to_id: HashMap<i64, NodeId>,
    id_to_rowid: Vec<i64>,
}

impl CompactNodeMap {
    fn intern(&mut self, rowid: i64) -> NodeId {
        if let Some(existing) = self.rowid_to_id.get(&rowid).copied() {
            return existing;
        }

        let next_id = self.id_to_rowid.len();
        self.id_to_rowid.push(rowid);
        self.rowid_to_id.insert(rowid, next_id);
        next_id
    }

    fn id_for_rowid(&self, rowid: i64) -> Option<NodeId> {
        self.rowid_to_id.get(&rowid).copied()
    }

    fn rowid_for_id(&self, node_id: NodeId) -> i64 {
        self.id_to_rowid[node_id]
    }
}

#[derive(Default)]
struct DenseSet {
    words: Vec<u64>,
    count: usize,
}

impl DenseSet {
    fn insert(&mut self, id: NodeId) -> bool {
        let word_index = id / u64::BITS as usize;
        let bit_mask = 1u64 << (id % u64::BITS as usize);

        if word_index >= self.words.len() {
            self.words.resize(word_index + 1, 0);
        }

        if self.words[word_index] & bit_mask == 0 {
            self.words[word_index] |= bit_mask;
            self.count = self.count.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn contains(&self, id: NodeId) -> bool {
        let word_index = id / u64::BITS as usize;
        let bit_mask = 1u64 << (id % u64::BITS as usize);
        self.words
            .get(word_index)
            .map(|word| *word & bit_mask != 0)
            .unwrap_or(false)
    }

    fn count(&self) -> usize {
        self.count
    }

    fn union_count(&self, other: &Self) -> usize {
        let max_len = self.words.len().max(other.words.len());
        let mut total = 0usize;
        for index in 0..max_len {
            let left = self.words.get(index).copied().unwrap_or_default();
            let right = other.words.get(index).copied().unwrap_or_default();
            total = total.saturating_add((left | right).count_ones() as usize);
        }
        total
    }
}

struct SearchSide {
    frontier: VecDeque<NodeId>,
    visited: DenseSet,
    predecessors: Vec<Option<NodeId>>,
}

impl SearchSide {
    fn new(start: NodeId) -> Self {
        let mut side = Self {
            frontier: VecDeque::from([start]),
            visited: DenseSet::default(),
            predecessors: Vec::new(),
        };
        side.visited.insert(start);
        ensure_slot(&mut side.predecessors, start);
        side
    }
}

pub fn traverse_graph(
    conn: &Connection,
    repo_id: i64,
    start_rowid: i64,
    strategy: GraphTraversalStrategy,
    direction: GraphDirection,
    max_nodes: usize,
) -> AppResult<GraphTraversalResponse> {
    let requested_max_nodes = if max_nodes == 0 {
        DEFAULT_GRAPH_MAX_NODES
    } else {
        max_nodes
    };
    let bounded_max_nodes = requested_max_nodes.clamp(1, MAX_GRAPH_MAX_NODES);

    let mut nodes = CompactNodeMap::default();
    let start_id = nodes.intern(start_rowid);
    let (visits, truncated) = match strategy {
        GraphTraversalStrategy::Bfs => traverse_bfs(
            conn,
            repo_id,
            start_id,
            direction,
            bounded_max_nodes,
            &mut nodes,
        )?,
        GraphTraversalStrategy::Dfs => traverse_dfs(
            conn,
            repo_id,
            start_id,
            direction,
            bounded_max_nodes,
            &mut nodes,
        )?,
        GraphTraversalStrategy::Topological => {
            if direction == GraphDirection::Both {
                return Err(AppError::InvalidArgument(
                    "topological traversal requires direction=parents or direction=children"
                        .to_string(),
                ));
            }
            traverse_topological(
                conn,
                repo_id,
                start_id,
                direction,
                bounded_max_nodes,
                &mut nodes,
            )?
        }
    };

    let graph_nodes = visits_to_graph_nodes(conn, repo_id, &visits, &nodes)?;
    Ok(GraphTraversalResponse {
        strategy,
        direction,
        nodes: graph_nodes,
        truncated,
    })
}

pub fn shortest_path(
    conn: &Connection,
    repo_id: i64,
    from_sha: &str,
    to_sha: &str,
    direction: GraphDirection,
    from_rowid: i64,
    to_rowid: i64,
) -> AppResult<ShortestPathResponse> {
    let mut nodes = CompactNodeMap::default();
    let from_id = nodes.intern(from_rowid);
    let to_id = nodes.intern(to_rowid);

    if from_id == to_id {
        return Ok(ShortestPathResponse {
            from_sha: from_sha.to_string(),
            to_sha: to_sha.to_string(),
            direction,
            found: true,
            path: vec![from_sha.to_string()],
            visited_nodes: 1,
        });
    }

    let reverse_direction = reverse_direction(direction);
    let mut forward = SearchSide::new(from_id);
    let mut backward = SearchSide::new(to_id);

    let mut meeting: Option<NodeId> = None;
    while !forward.frontier.is_empty() && !backward.frontier.is_empty() && meeting.is_none() {
        let expand_forward = forward.frontier.len() <= backward.frontier.len();
        if expand_forward {
            meeting = expand_frontier(
                conn,
                repo_id,
                &mut forward,
                &backward.visited,
                direction,
                &mut nodes,
            )?;
        } else {
            meeting = expand_frontier(
                conn,
                repo_id,
                &mut backward,
                &forward.visited,
                reverse_direction,
                &mut nodes,
            )?;
        }
    }

    let mut path = Vec::new();
    if let Some(meeting_id) = meeting {
        let id_path = reconstruct_bidirectional_path(
            from_id,
            to_id,
            meeting_id,
            &forward.predecessors,
            &backward.predecessors,
        )?;
        path = node_path_to_sha_path(conn, repo_id, &id_path, &nodes)?;
    }

    Ok(ShortestPathResponse {
        from_sha: from_sha.to_string(),
        to_sha: to_sha.to_string(),
        direction,
        found: meeting.is_some(),
        path,
        visited_nodes: forward.visited.union_count(&backward.visited),
    })
}

fn traverse_bfs(
    conn: &Connection,
    repo_id: i64,
    start_id: NodeId,
    direction: GraphDirection,
    max_nodes: usize,
    nodes: &mut CompactNodeMap,
) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
    traverse_bfs_adaptive(conn, repo_id, start_id, direction, max_nodes, nodes)
}

fn traverse_bfs_adaptive(
    conn: &Connection,
    repo_id: i64,
    start_id: NodeId,
    direction: GraphDirection,
    max_nodes: usize,
    nodes: &mut CompactNodeMap,
) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
    let total_nodes = count_commits(conn, repo_id)?;
    let mut visits = Vec::new();
    let mut visited = DenseSet::default();
    visited.insert(start_id);
    let mut depth = 0usize;
    let mut frontier = vec![start_id];

    while !frontier.is_empty() {
        let remaining_nodes = total_nodes.saturating_sub(visited.count());
        let mode = select_bfs_mode(frontier.len(), remaining_nodes);

        let next_frontier = match mode {
            BfsMode::Push => {
                expand_bfs_frontier_push(conn, repo_id, &frontier, direction, &mut visited, nodes)?
            }
            BfsMode::Pull => {
                expand_bfs_frontier_pull(conn, repo_id, &frontier, direction, &mut visited, nodes)?
            }
        };

        let level_len = frontier.len();
        for (index, node_id) in frontier.into_iter().enumerate() {
            visits.push((node_id, depth));
            if visits.len() >= max_nodes {
                let has_more = (index + 1 < level_len) || !next_frontier.is_empty();
                return Ok((visits, has_more));
            }
        }

        frontier = next_frontier;
        depth = depth.saturating_add(1);
    }

    Ok((visits, false))
}

#[cfg(test)]
fn traverse_bfs_push_only(
    conn: &Connection,
    repo_id: i64,
    start_id: NodeId,
    direction: GraphDirection,
    max_nodes: usize,
    nodes: &mut CompactNodeMap,
) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
    let mut visits = Vec::new();
    let mut visited = DenseSet::default();
    visited.insert(start_id);
    let mut depth = 0usize;
    let mut frontier = vec![start_id];

    while !frontier.is_empty() {
        let next_frontier =
            expand_bfs_frontier_push(conn, repo_id, &frontier, direction, &mut visited, nodes)?;

        let level_len = frontier.len();
        for (index, node_id) in frontier.into_iter().enumerate() {
            visits.push((node_id, depth));
            if visits.len() >= max_nodes {
                let has_more = (index + 1 < level_len) || !next_frontier.is_empty();
                return Ok((visits, has_more));
            }
        }

        frontier = next_frontier;
        depth = depth.saturating_add(1);
    }

    Ok((visits, false))
}

fn expand_bfs_frontier_push(
    conn: &Connection,
    repo_id: i64,
    frontier: &[NodeId],
    direction: GraphDirection,
    visited: &mut DenseSet,
    nodes: &mut CompactNodeMap,
) -> AppResult<Vec<NodeId>> {
    let source_rowids: Vec<i64> = frontier
        .iter()
        .map(|node_id| nodes.rowid_for_id(*node_id))
        .collect();
    let neighbors = load_neighbor_rowids_for_sources(conn, repo_id, &source_rowids, direction)?;

    let mut next_frontier = Vec::new();
    for source_neighbors in neighbors {
        for neighbor_rowid in source_neighbors {
            let neighbor_id = nodes.intern(neighbor_rowid);
            if visited.insert(neighbor_id) {
                next_frontier.push(neighbor_id);
            }
        }
    }

    Ok(next_frontier)
}

fn expand_bfs_frontier_pull(
    conn: &Connection,
    repo_id: i64,
    frontier: &[NodeId],
    direction: GraphDirection,
    visited: &mut DenseSet,
    nodes: &mut CompactNodeMap,
) -> AppResult<Vec<NodeId>> {
    let frontier_rowids: Vec<i64> = frontier
        .iter()
        .map(|node_id| nodes.rowid_for_id(*node_id))
        .collect();
    let discovered = load_pull_neighbor_rowids(conn, repo_id, &frontier_rowids, direction)?;

    let mut next_frontier = Vec::with_capacity(discovered.len());
    for discovered_rowid in discovered {
        let discovered_id = nodes.intern(discovered_rowid);
        if visited.insert(discovered_id) {
            next_frontier.push(discovered_id);
        }
    }

    Ok(next_frontier)
}

fn select_bfs_mode(frontier_len: usize, remaining_nodes: usize) -> BfsMode {
    if frontier_len < BFS_PULL_MIN_FRONTIER
        || remaining_nodes < BFS_PULL_MIN_REMAINING
        || remaining_nodes == 0
    {
        return BfsMode::Push;
    }

    if frontier_len.saturating_mul(BFS_PULL_FRONTIER_TO_REMAINING_RATIO) >= remaining_nodes {
        BfsMode::Pull
    } else {
        BfsMode::Push
    }
}

fn traverse_dfs(
    conn: &Connection,
    repo_id: i64,
    start_id: NodeId,
    direction: GraphDirection,
    max_nodes: usize,
    nodes: &mut CompactNodeMap,
) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
    let mut visits = Vec::new();
    let mut visited = DenseSet::default();
    visited.insert(start_id);
    let mut stack = vec![(start_id, 0usize)];

    while let Some((node_id, depth)) = stack.pop() {
        visits.push((node_id, depth));
        let source_rowid = nodes.rowid_for_id(node_id);

        let neighbors = load_neighbor_rowids(conn, repo_id, source_rowid, direction)?;
        for neighbor_rowid in neighbors.into_iter().rev() {
            let neighbor_id = nodes.intern(neighbor_rowid);
            if visited.insert(neighbor_id) {
                stack.push((neighbor_id, depth.saturating_add(1)));
            }
        }

        if visits.len() >= max_nodes {
            let has_more = !stack.is_empty()
                || has_unvisited_neighbors(
                    conn,
                    repo_id,
                    source_rowid,
                    direction,
                    &visited,
                    nodes,
                )?;
            return Ok((visits, has_more));
        }
    }

    Ok((visits, false))
}

fn traverse_topological(
    conn: &Connection,
    repo_id: i64,
    start_id: NodeId,
    direction: GraphDirection,
    max_nodes: usize,
    nodes: &mut CompactNodeMap,
) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
    let (reachable, depths, truncated_reachable) =
        collect_reachable_bfs(conn, repo_id, start_id, direction, max_nodes, nodes)?;
    let mut reachable_set = DenseSet::default();
    for node_id in &reachable {
        reachable_set.insert(*node_id);
    }

    let mut indegree: HashMap<NodeId, usize> = HashMap::with_capacity(reachable.len());
    for node_id in &reachable {
        indegree.insert(*node_id, 0usize);
    }
    let mut adjacency: HashMap<NodeId, Vec<NodeId>> = HashMap::with_capacity(reachable.len());

    let reachable_rowids: Vec<i64> = reachable
        .iter()
        .map(|node_id| nodes.rowid_for_id(*node_id))
        .collect();
    let neighbors = load_neighbor_rowids_for_sources(conn, repo_id, &reachable_rowids, direction)?;

    for (source_id, source_neighbors) in reachable.iter().zip(neighbors.into_iter()) {
        let mut filtered = Vec::new();
        for neighbor_rowid in source_neighbors {
            let neighbor_id = nodes.intern(neighbor_rowid);
            if reachable_set.contains(neighbor_id) {
                filtered.push(neighbor_id);
            }
        }

        for neighbor_id in &filtered {
            if let Some(entry) = indegree.get_mut(neighbor_id) {
                *entry = entry.saturating_add(1);
            }
        }
        adjacency.insert(*source_id, filtered);
    }

    let mut ready: BTreeSet<(usize, NodeId)> = BTreeSet::new();
    for source_id in &reachable {
        if indegree.get(source_id).copied().unwrap_or_default() == 0 {
            let depth = depths.get(source_id).copied().unwrap_or(usize::MAX);
            ready.insert((depth, *source_id));
        }
    }

    let mut ordered = Vec::new();
    while let Some((_, source_id)) = ready.pop_first() {
        ordered.push((
            source_id,
            depths.get(&source_id).copied().unwrap_or_default(),
        ));

        if let Some(source_neighbors) = adjacency.get(&source_id) {
            for neighbor_id in source_neighbors {
                if let Some(value) = indegree.get_mut(neighbor_id) {
                    if *value > 0 {
                        *value -= 1;
                    }
                    if *value == 0 {
                        ready.insert((
                            depths.get(neighbor_id).copied().unwrap_or(usize::MAX),
                            *neighbor_id,
                        ));
                    }
                }
            }
        }
    }

    let mut truncated = truncated_reachable || ordered.len() < reachable.len();
    if ordered.len() < reachable.len() {
        let mut present = DenseSet::default();
        for (node_id, _) in &ordered {
            present.insert(*node_id);
        }
        for source_id in reachable {
            if !present.contains(source_id) {
                ordered.push((
                    source_id,
                    depths.get(&source_id).copied().unwrap_or_default(),
                ));
            }
        }
        truncated = true;
    }

    Ok((ordered, truncated))
}

fn collect_reachable_bfs(
    conn: &Connection,
    repo_id: i64,
    start_id: NodeId,
    direction: GraphDirection,
    max_nodes: usize,
    nodes: &mut CompactNodeMap,
) -> AppResult<(Vec<NodeId>, HashMap<NodeId, usize>, bool)> {
    let mut order = Vec::new();
    let mut depths = HashMap::new();
    let mut visited = DenseSet::default();

    visited.insert(start_id);
    depths.insert(start_id, 0usize);

    let mut queue = VecDeque::from([(start_id, 0usize)]);
    while !queue.is_empty() {
        let breadth = queue.len();
        let mut level_nodes = Vec::with_capacity(breadth);
        for _ in 0..breadth {
            if let Some((node_id, depth)) = queue.pop_front() {
                level_nodes.push((node_id, depth));
            }
        }

        let source_rowids: Vec<i64> = level_nodes
            .iter()
            .map(|(node_id, _)| nodes.rowid_for_id(*node_id))
            .collect();
        let neighbors = load_neighbor_rowids_for_sources(conn, repo_id, &source_rowids, direction)?;

        let level_len = level_nodes.len();
        for (index, ((source_id, depth), source_neighbors)) in level_nodes
            .into_iter()
            .zip(neighbors.into_iter())
            .enumerate()
        {
            order.push(source_id);
            for neighbor_rowid in source_neighbors {
                let neighbor_id = nodes.intern(neighbor_rowid);
                if visited.insert(neighbor_id) {
                    let next_depth = depth.saturating_add(1);
                    depths.insert(neighbor_id, next_depth);
                    queue.push_back((neighbor_id, next_depth));
                }
            }

            if order.len() >= max_nodes {
                let has_more = (index + 1 < level_len) || !queue.is_empty();
                return Ok((order, depths, has_more));
            }
        }
    }

    Ok((order, depths, false))
}

fn has_unvisited_neighbors(
    conn: &Connection,
    repo_id: i64,
    source_rowid: i64,
    direction: GraphDirection,
    visited: &DenseSet,
    nodes: &CompactNodeMap,
) -> AppResult<bool> {
    let neighbors = load_neighbor_rowids(conn, repo_id, source_rowid, direction)?;
    for neighbor_rowid in neighbors {
        if let Some(neighbor_id) = nodes.id_for_rowid(neighbor_rowid) {
            if !visited.contains(neighbor_id) {
                return Ok(true);
            }
        } else {
            return Ok(true);
        }
    }
    Ok(false)
}

fn reverse_direction(direction: GraphDirection) -> GraphDirection {
    match direction {
        GraphDirection::Parents => GraphDirection::Children,
        GraphDirection::Children => GraphDirection::Parents,
        GraphDirection::Both => GraphDirection::Both,
    }
}

fn expand_frontier(
    conn: &Connection,
    repo_id: i64,
    current_side: &mut SearchSide,
    visited_other_side: &DenseSet,
    direction: GraphDirection,
    nodes: &mut CompactNodeMap,
) -> AppResult<Option<NodeId>> {
    let breadth = current_side.frontier.len();
    let mut level_nodes = Vec::with_capacity(breadth);
    for _ in 0..breadth {
        if let Some(node_id) = current_side.frontier.pop_front() {
            level_nodes.push(node_id);
        }
    }

    let source_rowids: Vec<i64> = level_nodes
        .iter()
        .map(|node_id| nodes.rowid_for_id(*node_id))
        .collect();
    let neighbors = load_neighbor_rowids_for_sources(conn, repo_id, &source_rowids, direction)?;

    for (source_id, source_neighbors) in level_nodes.into_iter().zip(neighbors.into_iter()) {
        for neighbor_rowid in source_neighbors {
            let neighbor_id = nodes.intern(neighbor_rowid);
            if current_side.visited.insert(neighbor_id) {
                ensure_slot(&mut current_side.predecessors, neighbor_id);
                current_side.predecessors[neighbor_id] = Some(source_id);

                if visited_other_side.contains(neighbor_id) {
                    return Ok(Some(neighbor_id));
                }

                current_side.frontier.push_back(neighbor_id);
            }
        }
    }

    Ok(None)
}

fn reconstruct_bidirectional_path(
    from_id: NodeId,
    to_id: NodeId,
    meeting_id: NodeId,
    forward_prev: &[Option<NodeId>],
    backward_prev: &[Option<NodeId>],
) -> AppResult<Vec<NodeId>> {
    let mut left = vec![meeting_id];
    let mut cursor = meeting_id;
    while cursor != from_id {
        let Some(prev) = forward_prev.get(cursor).copied().flatten() else {
            return Err(AppError::Parse(
                "path reconstruction failed from source side".to_string(),
            ));
        };
        cursor = prev;
        left.push(cursor);
    }
    left.reverse();

    let mut right = Vec::new();
    let mut cursor = meeting_id;
    while cursor != to_id {
        let Some(next) = backward_prev.get(cursor).copied().flatten() else {
            return Err(AppError::Parse(
                "path reconstruction failed from target side".to_string(),
            ));
        };
        cursor = next;
        right.push(cursor);
    }

    left.extend(right);
    Ok(left)
}

fn visits_to_graph_nodes(
    conn: &Connection,
    repo_id: i64,
    visits: &[(NodeId, usize)],
    nodes: &CompactNodeMap,
) -> AppResult<Vec<GraphNodeVisit>> {
    let rowids: Vec<i64> = visits
        .iter()
        .map(|(node_id, _depth)| nodes.rowid_for_id(*node_id))
        .collect();
    let shas = load_commit_shas_for_rowids(conn, repo_id, &rowids)?;
    if shas.len() != visits.len() {
        return Err(AppError::Parse(
            "missing commit sha for visited rowid during graph traversal".to_string(),
        ));
    }

    let mut result = Vec::with_capacity(visits.len());
    for ((_, depth), sha) in visits.iter().zip(shas.into_iter()) {
        result.push(GraphNodeVisit { sha, depth: *depth });
    }

    Ok(result)
}

fn node_path_to_sha_path(
    conn: &Connection,
    repo_id: i64,
    path: &[NodeId],
    nodes: &CompactNodeMap,
) -> AppResult<Vec<String>> {
    let rowids: Vec<i64> = path
        .iter()
        .map(|node_id| nodes.rowid_for_id(*node_id))
        .collect();
    let shas = load_commit_shas_for_rowids(conn, repo_id, &rowids)?;
    if shas.len() != path.len() {
        return Err(AppError::Parse(
            "missing commit sha for shortest path rowid".to_string(),
        ));
    }

    Ok(shas)
}

fn ensure_slot<T>(slots: &mut Vec<Option<T>>, index: usize) {
    if index >= slots.len() {
        slots.resize_with(index + 1, || None);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    use rusqlite::{params, Connection};

    use super::{
        select_bfs_mode, traverse_bfs_adaptive, traverse_bfs_push_only, BfsMode, CompactNodeMap,
    };
    use crate::adapters::db::run_migrations;
    use crate::domain::models::GraphDirection;

    struct PullOverlapFixture {
        repo_id: i64,
        root_rowid: i64,
        expected_nodes: usize,
    }

    #[test]
    fn bfs_mode_switches_to_pull_for_large_frontier() {
        assert_eq!(select_bfs_mode(16, 500), BfsMode::Push);
        assert_eq!(select_bfs_mode(2_000, 64), BfsMode::Push);
        assert_eq!(select_bfs_mode(512, 8_192), BfsMode::Push);
        assert_eq!(select_bfs_mode(512, 4_096), BfsMode::Pull);
    }

    #[test]
    #[ignore = "performance budget gate runs this test in dedicated CI step"]
    fn perf_budget_direction_optimizing_bfs() {
        const FRONTIER_WIDTH: usize = 2_000;
        const SINK_WIDTH: usize = 64;
        const RUNS: usize = 7;
        const MAX_SLOWDOWN_PERCENT: u128 = 150;

        let mut conn = Connection::open_in_memory().expect("open in-memory db");
        run_migrations(&mut conn).expect("run migrations");
        let fixture = seed_pull_overlap_fixture(&mut conn, FRONTIER_WIDTH, SINK_WIDTH);

        let (push_visits, push_truncated) = run_push_only(
            &conn,
            fixture.repo_id,
            fixture.root_rowid,
            fixture.expected_nodes,
        );
        let (adaptive_visits, adaptive_truncated) = run_adaptive(
            &conn,
            fixture.repo_id,
            fixture.root_rowid,
            fixture.expected_nodes,
        );
        assert_eq!(push_visits, adaptive_visits);
        assert_eq!(push_truncated, adaptive_truncated);
        assert_eq!(adaptive_visits.len(), fixture.expected_nodes);

        let _ = run_push_only(
            &conn,
            fixture.repo_id,
            fixture.root_rowid,
            fixture.expected_nodes,
        );
        let _ = run_adaptive(
            &conn,
            fixture.repo_id,
            fixture.root_rowid,
            fixture.expected_nodes,
        );

        let (push_median, adaptive_median) = benchmark_pair_medians(
            RUNS,
            || {
                let _ = run_push_only(
                    &conn,
                    fixture.repo_id,
                    fixture.root_rowid,
                    fixture.expected_nodes,
                );
            },
            || {
                let _ = run_adaptive(
                    &conn,
                    fixture.repo_id,
                    fixture.root_rowid,
                    fixture.expected_nodes,
                );
            },
        );

        let push_nanos = push_median.as_nanos();
        let adaptive_nanos = adaptive_median.as_nanos();
        assert!(
            adaptive_nanos.saturating_mul(100) <= push_nanos.saturating_mul(MAX_SLOWDOWN_PERCENT),
            "adaptive BFS perf budget breached: adaptive={adaptive_median:?}, push={push_median:?}, max_slowdown={}%",
            MAX_SLOWDOWN_PERCENT
        );
    }

    fn run_push_only(
        conn: &Connection,
        repo_id: i64,
        root_rowid: i64,
        max_nodes: usize,
    ) -> (Vec<(usize, usize)>, bool) {
        let mut nodes = CompactNodeMap::default();
        let start_id = nodes.intern(root_rowid);
        let (visits, truncated) = traverse_bfs_push_only(
            conn,
            repo_id,
            start_id,
            GraphDirection::Children,
            max_nodes,
            &mut nodes,
        )
        .expect("run push-only bfs");
        (visits, truncated)
    }

    fn run_adaptive(
        conn: &Connection,
        repo_id: i64,
        root_rowid: i64,
        max_nodes: usize,
    ) -> (Vec<(usize, usize)>, bool) {
        let mut nodes = CompactNodeMap::default();
        let start_id = nodes.intern(root_rowid);
        let (visits, truncated) = traverse_bfs_adaptive(
            conn,
            repo_id,
            start_id,
            GraphDirection::Children,
            max_nodes,
            &mut nodes,
        )
        .expect("run adaptive bfs");
        (visits, truncated)
    }

    fn seed_pull_overlap_fixture(
        conn: &mut Connection,
        frontier_width: usize,
        sink_width: usize,
    ) -> PullOverlapFixture {
        let tx = conn.transaction().expect("start tx");
        tx.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/perf", "perf", "/tmp/perf/.git"],
        )
        .expect("insert repository");
        let repo_id = tx.last_insert_rowid();

        let mut commit_ids: HashMap<String, i64> = HashMap::new();

        let mut insert_commit = tx
            .prepare("INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)")
            .expect("prepare commit insert");
        insert_commit
            .execute(params![repo_id, "root"])
            .expect("insert root");
        commit_ids.insert("root".to_string(), tx.last_insert_rowid());

        for index in 0..frontier_width {
            let sha = format!("l1-{index:05}");
            insert_commit
                .execute(params![repo_id, sha.as_str()])
                .expect("insert l1 commit");
            commit_ids.insert(sha, tx.last_insert_rowid());
        }
        for index in 0..sink_width {
            let sha = format!("l2-{index:05}");
            insert_commit
                .execute(params![repo_id, sha.as_str()])
                .expect("insert l2 commit");
            commit_ids.insert(sha, tx.last_insert_rowid());
        }
        drop(insert_commit);

        let mut insert_edge = tx
            .prepare(
                "INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id) VALUES (?1, ?2, ?3)",
            )
            .expect("prepare edge insert");

        for index in 0..frontier_width {
            let child = format!("l1-{index:05}");
            let child_id = *commit_ids.get(&child).expect("resolve l1 child id");
            let root_id = *commit_ids.get("root").expect("resolve root id");
            insert_edge
                .execute(params![repo_id, child_id, root_id])
                .expect("insert root->l1 edge");
        }
        for left in 0..frontier_width {
            let parent = format!("l1-{left:05}");
            let parent_id = *commit_ids.get(&parent).expect("resolve l1 parent id");
            for right in 0..sink_width {
                let child = format!("l2-{right:05}");
                let child_id = *commit_ids.get(&child).expect("resolve l2 child id");
                insert_edge
                    .execute(params![repo_id, child_id, parent_id])
                    .expect("insert l1->l2 edge");
            }
        }
        drop(insert_edge);

        tx.commit().expect("commit fixture tx");

        let root_rowid: i64 = conn
            .query_row(
                "SELECT commit_id FROM commits WHERE repo_id = ?1 AND sha = 'root'",
                [repo_id],
                |row| row.get(0),
            )
            .expect("resolve root rowid");

        PullOverlapFixture {
            repo_id,
            root_rowid,
            expected_nodes: 1 + frontier_width + sink_width,
        }
    }

    fn benchmark_pair_medians<F, G>(runs: usize, mut left: F, mut right: G) -> (Duration, Duration)
    where
        F: FnMut(),
        G: FnMut(),
    {
        let mut left_samples = Vec::with_capacity(runs);
        let mut right_samples = Vec::with_capacity(runs);

        for run in 0..runs {
            if run % 2 == 0 {
                left_samples.push(measure_duration(&mut left));
                right_samples.push(measure_duration(&mut right));
            } else {
                right_samples.push(measure_duration(&mut right));
                left_samples.push(measure_duration(&mut left));
            }
        }

        left_samples.sort_unstable();
        right_samples.sort_unstable();
        (left_samples[runs / 2], right_samples[runs / 2])
    }

    fn measure_duration<F>(operation: &mut F) -> Duration
    where
        F: FnMut(),
    {
        let started = Instant::now();
        operation();
        started.elapsed()
    }
}
