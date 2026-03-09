use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::Connection;

use crate::adapters::db::load_repository_indexed_at;
use crate::application::limits::normalize_graph_max_nodes;
use crate::domain::models::{
    GraphDirection, GraphNodeVisit, GraphTraversalResponse, GraphTraversalStrategy,
    ShortestPathResponse,
};
use crate::support::error::{AppError, AppResult};

const BFS_PULL_MIN_FRONTIER: usize = 256;
const BFS_PULL_MIN_REMAINING: usize = 1_024;
const BFS_PULL_FRONTIER_TO_REMAINING_RATIO: usize = 8;

type NodeId = usize;

#[derive(Debug, Clone, PartialEq, Eq)]
struct QueryGraphCacheKey {
    db_path: String,
    repo_id: i64,
    indexed_at: Option<String>,
}

#[derive(Debug)]
struct QueryGraphCacheEntry {
    key: QueryGraphCacheKey,
    graph: Arc<QueryGraph>,
}

static QUERY_GRAPH_CACHE: OnceLock<Mutex<Option<QueryGraphCacheEntry>>> = OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BfsMode {
    Push,
    Pull,
}

#[derive(Debug, Clone)]
pub(super) struct QueryGraph {
    sha_to_id: HashMap<String, NodeId>,
    id_to_sha: Vec<String>,
    parent_neighbors: Vec<Vec<NodeId>>,
    child_neighbors: Vec<Vec<NodeId>>,
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

impl QueryGraph {
    pub(super) fn hydrate(conn: &Connection, repo_id: i64) -> AppResult<Self> {
        let mut commit_stmt = conn.prepare_cached(
            r#"
            SELECT commit_id, sha
            FROM commits
            WHERE repo_id = ?1
            ORDER BY commit_id ASC
        "#,
        )?;
        let commit_rows = commit_stmt.query_map([repo_id], |row| {
            let rowid: i64 = row.get(0)?;
            let sha: String = row.get(1)?;
            Ok((rowid, sha))
        })?;

        let mut rowid_to_id = HashMap::new();
        let mut sha_to_id = HashMap::new();
        let mut id_to_sha = Vec::new();

        for row in commit_rows {
            let (rowid, sha) = row?;
            let node_id = id_to_sha.len();
            rowid_to_id.insert(rowid, node_id);
            sha_to_id.insert(sha.clone(), node_id);
            id_to_sha.push(sha);
        }

        let mut parent_neighbors = vec![Vec::new(); id_to_sha.len()];
        let mut child_neighbors = vec![Vec::new(); id_to_sha.len()];

        let mut edge_stmt = conn.prepare_cached(
            r#"
            SELECT child_commit_id, parent_commit_id
            FROM commit_edges
            WHERE repo_id = ?1
        "#,
        )?;
        let edge_rows = edge_stmt.query_map([repo_id], |row| {
            let child_rowid: i64 = row.get(0)?;
            let parent_rowid: i64 = row.get(1)?;
            Ok((child_rowid, parent_rowid))
        })?;

        for row in edge_rows {
            let (child_rowid, parent_rowid) = row?;
            let child_id = rowid_to_id.get(&child_rowid).copied().ok_or_else(|| {
                AppError::Parse(format!(
                    "query graph hydration missing child commit rowid {child_rowid}"
                ))
            })?;
            let parent_id = rowid_to_id.get(&parent_rowid).copied().ok_or_else(|| {
                AppError::Parse(format!(
                    "query graph hydration missing parent commit rowid {parent_rowid}"
                ))
            })?;

            parent_neighbors[child_id].push(parent_id);
            child_neighbors[parent_id].push(child_id);
        }

        for neighbors in &mut parent_neighbors {
            neighbors.sort_unstable_by(|left, right| right.cmp(left));
            neighbors.dedup();
        }
        for neighbors in &mut child_neighbors {
            neighbors.sort_unstable_by(|left, right| right.cmp(left));
            neighbors.dedup();
        }

        Ok(Self {
            sha_to_id,
            id_to_sha,
            parent_neighbors,
            child_neighbors,
        })
    }

    pub(super) fn traverse(
        &self,
        start_sha: &str,
        strategy: GraphTraversalStrategy,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<GraphTraversalResponse> {
        let bounded_max_nodes = normalize_graph_max_nodes(max_nodes);
        let Some(start_id) = self.sha_to_id.get(start_sha).copied() else {
            return Err(AppError::InvalidArgument(format!(
                "start commit not indexed for repository: {start_sha}"
            )));
        };

        let (visits, truncated) = match strategy {
            GraphTraversalStrategy::Bfs => {
                self.traverse_bfs(start_id, direction, bounded_max_nodes)?
            }
            GraphTraversalStrategy::Dfs => {
                self.traverse_dfs(start_id, direction, bounded_max_nodes)?
            }
            GraphTraversalStrategy::Topological => {
                if direction == GraphDirection::Both {
                    return Err(AppError::InvalidArgument(
                        "topological traversal requires direction=parents or direction=children"
                            .to_string(),
                    ));
                }
                self.traverse_topological(start_id, direction, bounded_max_nodes)?
            }
        };

        let nodes = visits
            .into_iter()
            .map(|(node_id, depth)| GraphNodeVisit {
                sha: self.id_to_sha[node_id].clone(),
                depth,
            })
            .collect();

        Ok(GraphTraversalResponse {
            strategy,
            direction,
            nodes,
            truncated,
        })
    }

    pub(super) fn shortest_path(
        &self,
        from_sha: &str,
        to_sha: &str,
        direction: GraphDirection,
    ) -> AppResult<ShortestPathResponse> {
        let Some(from_id) = self.sha_to_id.get(from_sha).copied() else {
            return Err(AppError::InvalidArgument(format!(
                "source commit not indexed for repository: {from_sha}"
            )));
        };
        let Some(to_id) = self.sha_to_id.get(to_sha).copied() else {
            return Err(AppError::InvalidArgument(format!(
                "target commit not indexed for repository: {to_sha}"
            )));
        };

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
                meeting = self.expand_frontier(&mut forward, &backward.visited, direction)?;
            } else {
                meeting =
                    self.expand_frontier(&mut backward, &forward.visited, reverse_direction)?;
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
            path = id_path
                .into_iter()
                .map(|node_id| self.id_to_sha[node_id].clone())
                .collect();
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

    fn node_count(&self) -> usize {
        self.id_to_sha.len()
    }

    fn traverse_bfs(
        &self,
        start_id: NodeId,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
        let total_nodes = self.node_count();
        let mut visits = Vec::new();
        let mut visited = DenseSet::default();
        visited.insert(start_id);
        let mut depth = 0usize;
        let mut frontier = vec![start_id];

        while !frontier.is_empty() {
            let remaining_budget = max_nodes.saturating_sub(visits.len());
            if remaining_budget == 0 {
                return Ok((visits, true));
            }

            let level_len = frontier.len();
            if level_len > remaining_budget {
                for node_id in frontier.iter().take(remaining_budget) {
                    visits.push((*node_id, depth));
                }
                return Ok((visits, true));
            }

            for node_id in &frontier {
                visits.push((*node_id, depth));
            }

            let remaining_nodes = total_nodes.saturating_sub(visited.count());
            frontier = match select_bfs_mode(frontier.len(), remaining_nodes) {
                BfsMode::Push => self.expand_bfs_frontier_push(&frontier, direction, &mut visited),
                BfsMode::Pull => self.expand_bfs_frontier_pull(&frontier, direction, &mut visited),
            };
            depth = depth.saturating_add(1);
        }

        Ok((visits, false))
    }

    fn expand_bfs_frontier_push(
        &self,
        frontier: &[NodeId],
        direction: GraphDirection,
        visited: &mut DenseSet,
    ) -> Vec<NodeId> {
        let mut next_frontier = Vec::new();
        for source in frontier {
            self.for_each_neighbor(*source, direction, |neighbor| {
                if visited.insert(neighbor) {
                    next_frontier.push(neighbor);
                }
            });
        }
        next_frontier
    }

    fn expand_bfs_frontier_pull(
        &self,
        frontier: &[NodeId],
        direction: GraphDirection,
        visited: &mut DenseSet,
    ) -> Vec<NodeId> {
        let mut discovered = Vec::new();
        for source in frontier {
            self.for_each_neighbor(*source, direction, |neighbor| discovered.push(neighbor));
        }
        discovered.sort_unstable_by(|left, right| right.cmp(left));
        discovered.dedup();

        let mut next_frontier = Vec::with_capacity(discovered.len());
        for node_id in discovered {
            if visited.insert(node_id) {
                next_frontier.push(node_id);
            }
        }
        next_frontier
    }

    fn traverse_dfs(
        &self,
        start_id: NodeId,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
        let mut visits = Vec::new();
        let mut visited = DenseSet::default();
        visited.insert(start_id);
        let mut stack = vec![(start_id, 0usize)];

        while let Some((node_id, depth)) = stack.pop() {
            visits.push((node_id, depth));
            if visits.len() >= max_nodes {
                let has_more =
                    !stack.is_empty() || self.has_unvisited_neighbors(node_id, direction, &visited);
                return Ok((visits, has_more));
            }

            let mut neighbors = Vec::new();
            self.for_each_neighbor(node_id, direction, |neighbor| neighbors.push(neighbor));
            for neighbor in neighbors.into_iter().rev() {
                if visited.insert(neighbor) {
                    stack.push((neighbor, depth.saturating_add(1)));
                }
            }
        }

        Ok((visits, false))
    }

    fn traverse_topological(
        &self,
        start_id: NodeId,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<(Vec<(NodeId, usize)>, bool)> {
        let (reachable, reachable_depths, truncated_reachable) =
            self.collect_reachable_bfs(start_id, direction, max_nodes)?;
        let reachable_count = reachable.len();
        let mut local_index_by_node = vec![usize::MAX; self.node_count()];
        for (local_index, node_id) in reachable.iter().copied().enumerate() {
            local_index_by_node[node_id] = local_index;
        }

        let mut indegree = vec![0usize; reachable_count];
        let mut adjacency: Vec<Vec<usize>> = vec![Vec::new(); reachable_count];

        for (source_local_index, source_id) in reachable.iter().copied().enumerate() {
            self.for_each_neighbor(source_id, direction, |neighbor_id| {
                if let Some(&neighbor_local_index) = local_index_by_node.get(neighbor_id) {
                    if neighbor_local_index != usize::MAX {
                        adjacency[source_local_index].push(neighbor_local_index);
                    }
                }
            });

            adjacency[source_local_index].sort_unstable();
            adjacency[source_local_index].dedup();
            for &neighbor_local_index in &adjacency[source_local_index] {
                indegree[neighbor_local_index] = indegree[neighbor_local_index].saturating_add(1);
            }
        }

        let max_depth = reachable_depths.iter().copied().max().unwrap_or_default();
        let mut ready_depth_buckets: Vec<VecDeque<usize>> = std::iter::repeat_with(VecDeque::new)
            .take(max_depth.saturating_add(1))
            .collect();
        let mut next_ready_depth = ready_depth_buckets.len();
        for local_index in 0..reachable_count {
            if indegree[local_index] == 0 {
                let depth = reachable_depths[local_index];
                ready_depth_buckets[depth].push_back(local_index);
                next_ready_depth = next_ready_depth.min(depth);
            }
        }

        let mut ordered = Vec::with_capacity(reachable_count);
        let mut emitted_local = vec![false; reachable_count];
        while next_ready_depth < ready_depth_buckets.len() {
            while next_ready_depth < ready_depth_buckets.len()
                && ready_depth_buckets[next_ready_depth].is_empty()
            {
                next_ready_depth = next_ready_depth.saturating_add(1);
            }
            if next_ready_depth >= ready_depth_buckets.len() {
                break;
            }

            let Some(source_local_index) = ready_depth_buckets[next_ready_depth].pop_front() else {
                continue;
            };
            emitted_local[source_local_index] = true;
            ordered.push((
                reachable[source_local_index],
                reachable_depths[source_local_index],
            ));

            for &neighbor_local_index in &adjacency[source_local_index] {
                if indegree[neighbor_local_index] == 0 {
                    continue;
                }
                indegree[neighbor_local_index] -= 1;
                if indegree[neighbor_local_index] == 0 {
                    let neighbor_depth = reachable_depths[neighbor_local_index];
                    ready_depth_buckets[neighbor_depth].push_back(neighbor_local_index);
                    if neighbor_depth < next_ready_depth {
                        next_ready_depth = neighbor_depth;
                    }
                }
            }
        }

        let mut truncated = truncated_reachable || ordered.len() < reachable_count;
        if ordered.len() < reachable_count {
            for (local_index, was_emitted) in emitted_local.into_iter().enumerate() {
                if !was_emitted {
                    ordered.push((reachable[local_index], reachable_depths[local_index]));
                }
            }
            truncated = true;
        }

        Ok((ordered, truncated))
    }

    fn collect_reachable_bfs(
        &self,
        start_id: NodeId,
        direction: GraphDirection,
        max_nodes: usize,
    ) -> AppResult<(Vec<NodeId>, Vec<usize>, bool)> {
        let mut order = Vec::new();
        let mut reachable_depths = Vec::new();
        let mut visited = DenseSet::default();

        visited.insert(start_id);

        let mut queue = VecDeque::from([(start_id, 0usize)]);
        while !queue.is_empty() {
            let breadth = queue.len();
            let mut level_nodes = Vec::with_capacity(breadth);
            for _ in 0..breadth {
                if let Some((node_id, depth)) = queue.pop_front() {
                    level_nodes.push((node_id, depth));
                }
            }

            let remaining_budget = max_nodes.saturating_sub(order.len());
            if remaining_budget == 0 {
                return Ok((order, reachable_depths, true));
            }

            let level_len = level_nodes.len();
            if level_len > remaining_budget {
                for (source_id, depth) in level_nodes.iter().take(remaining_budget) {
                    order.push(*source_id);
                    reachable_depths.push(*depth);
                }
                return Ok((order, reachable_depths, true));
            }

            for (source_id, depth) in &level_nodes {
                order.push(*source_id);
                reachable_depths.push(*depth);
            }

            for (source_id, depth) in level_nodes {
                self.for_each_neighbor(source_id, direction, |neighbor_id| {
                    if visited.insert(neighbor_id) {
                        let next_depth = depth.saturating_add(1);
                        queue.push_back((neighbor_id, next_depth));
                    }
                });
            }
        }

        Ok((order, reachable_depths, false))
    }

    fn has_unvisited_neighbors(
        &self,
        source: NodeId,
        direction: GraphDirection,
        visited: &DenseSet,
    ) -> bool {
        let mut has_more = false;
        self.for_each_neighbor(source, direction, |neighbor| {
            if !visited.contains(neighbor) {
                has_more = true;
            }
        });
        has_more
    }

    fn expand_frontier(
        &self,
        current_side: &mut SearchSide,
        visited_other_side: &DenseSet,
        direction: GraphDirection,
    ) -> AppResult<Option<NodeId>> {
        let breadth = current_side.frontier.len();
        let mut level_nodes = Vec::with_capacity(breadth);
        for _ in 0..breadth {
            if let Some(node_id) = current_side.frontier.pop_front() {
                level_nodes.push(node_id);
            }
        }

        for source_id in level_nodes {
            let mut neighbors = Vec::new();
            self.for_each_neighbor(source_id, direction, |neighbor| neighbors.push(neighbor));
            for neighbor_id in neighbors {
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

    fn for_each_neighbor(
        &self,
        source: NodeId,
        direction: GraphDirection,
        mut visit: impl FnMut(NodeId),
    ) {
        if source >= self.node_count() {
            return;
        }
        match direction {
            GraphDirection::Parents => {
                for &neighbor in &self.parent_neighbors[source] {
                    visit(neighbor);
                }
            }
            GraphDirection::Children => {
                for &neighbor in &self.child_neighbors[source] {
                    visit(neighbor);
                }
            }
            GraphDirection::Both => {
                let parents = &self.parent_neighbors[source];
                let children = &self.child_neighbors[source];
                let mut parent_index = 0usize;
                let mut child_index = 0usize;
                let mut previous_neighbor = None;

                while parent_index < parents.len() || child_index < children.len() {
                    let next_neighbor = if child_index >= children.len()
                        || (parent_index < parents.len()
                            && parents[parent_index] >= children[child_index])
                    {
                        let next = parents[parent_index];
                        parent_index += 1;
                        next
                    } else {
                        let next = children[child_index];
                        child_index += 1;
                        next
                    };

                    if previous_neighbor != Some(next_neighbor) {
                        previous_neighbor = Some(next_neighbor);
                        visit(next_neighbor);
                    }
                }
            }
        }
    }
}

pub(super) fn load_or_hydrate_query_graph(
    conn: &Connection,
    db_path: &Path,
    repo_id: i64,
) -> AppResult<Arc<QueryGraph>> {
    let key = build_cache_key(conn, db_path, repo_id)?;
    let cache = QUERY_GRAPH_CACHE.get_or_init(|| Mutex::new(None));

    {
        let guard = cache
            .lock()
            .map_err(|_| AppError::Parse("query graph cache mutex poisoned".to_string()))?;
        if let Some(entry) = guard.as_ref() {
            if entry.key == key {
                return Ok(Arc::clone(&entry.graph));
            }
        }
    }

    let graph = Arc::new(QueryGraph::hydrate(conn, repo_id)?);
    let mut guard = cache
        .lock()
        .map_err(|_| AppError::Parse("query graph cache mutex poisoned".to_string()))?;
    *guard = Some(QueryGraphCacheEntry {
        key,
        graph: Arc::clone(&graph),
    });

    Ok(graph)
}

pub(super) fn invalidate_query_graph_cache(db_path: &Path) {
    let Some(cache) = QUERY_GRAPH_CACHE.get() else {
        return;
    };
    let normalized_db_path = normalize_db_path(db_path);
    if let Ok(mut guard) = cache.lock() {
        let should_clear = guard
            .as_ref()
            .map(|entry| entry.key.db_path == normalized_db_path)
            .unwrap_or(false);
        if should_clear {
            *guard = None;
        }
    }
}

fn build_cache_key(
    conn: &Connection,
    db_path: &Path,
    repo_id: i64,
) -> AppResult<QueryGraphCacheKey> {
    Ok(QueryGraphCacheKey {
        db_path: normalize_db_path(db_path),
        repo_id,
        indexed_at: load_repository_indexed_at(conn, repo_id)?,
    })
}

fn normalize_db_path(db_path: &Path) -> String {
    let absolute = db_path
        .canonicalize()
        .unwrap_or_else(|_| db_path.to_path_buf());
    absolute.to_string_lossy().to_string()
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

fn reverse_direction(direction: GraphDirection) -> GraphDirection {
    match direction {
        GraphDirection::Parents => GraphDirection::Children,
        GraphDirection::Children => GraphDirection::Parents,
        GraphDirection::Both => GraphDirection::Both,
    }
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

fn ensure_slot<T>(slots: &mut Vec<Option<T>>, index: usize) {
    if index >= slots.len() {
        slots.resize_with(index + 1, || None);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use rusqlite::{params, Connection};

    use super::{invalidate_query_graph_cache, load_or_hydrate_query_graph};
    use crate::adapters::db::{run_migrations, set_repository_indexed_at, upsert_repository};
    use crate::domain::models::{GraphDirection, GraphTraversalStrategy};

    #[test]
    fn query_graph_hydration_serves_traversal_and_shortest_path() {
        let db_path = unique_temp_db_path("hydration");
        let conn = open_test_database(&db_path);
        let repo_id = seed_linear_graph(&conn, "repo-hydration", &["c1", "c2", "c3"]);
        set_repository_indexed_at(&conn, repo_id, 100).expect("set repository indexed_at");

        let graph = load_or_hydrate_query_graph(&conn, &db_path, repo_id)
            .expect("hydrate query graph from sqlite");
        let traversal = graph
            .traverse(
                "c3",
                GraphTraversalStrategy::Bfs,
                GraphDirection::Parents,
                64,
            )
            .expect("run bfs traversal");
        let visited: Vec<String> = traversal.nodes.into_iter().map(|node| node.sha).collect();
        assert_eq!(
            visited,
            vec!["c3".to_string(), "c2".to_string(), "c1".to_string()]
        );

        let shortest = graph
            .shortest_path("c3", "c1", GraphDirection::Parents)
            .expect("run shortest path");
        assert!(shortest.found);
        assert_eq!(
            shortest.path,
            vec!["c3".to_string(), "c2".to_string(), "c1".to_string()]
        );

        invalidate_query_graph_cache(&db_path);
        drop(conn);
        let _ = fs::remove_file(&db_path);
    }

    #[test]
    fn cache_rehydrates_when_repository_indexed_at_changes() {
        let db_path = unique_temp_db_path("rehydrate");
        let conn = open_test_database(&db_path);
        let repo_id = seed_linear_graph(&conn, "repo-rehydrate", &["c1"]);
        set_repository_indexed_at(&conn, repo_id, 100).expect("set initial indexed_at");

        let first_graph =
            load_or_hydrate_query_graph(&conn, &db_path, repo_id).expect("hydrate first snapshot");
        let missing_in_first = first_graph
            .traverse(
                "c2",
                GraphTraversalStrategy::Bfs,
                GraphDirection::Parents,
                8,
            )
            .expect_err("c2 should be absent from first snapshot");
        assert!(missing_in_first
            .to_string()
            .contains("start commit not indexed for repository: c2"));

        append_commit_with_parent(&conn, repo_id, "c2", "c1");
        set_repository_indexed_at(&conn, repo_id, 101).expect("advance indexed_at for rehydrate");

        let second_graph =
            load_or_hydrate_query_graph(&conn, &db_path, repo_id).expect("hydrate second snapshot");
        assert!(
            !std::sync::Arc::ptr_eq(&first_graph, &second_graph),
            "cache key must refresh graph on indexed_at change"
        );
        let traversal = second_graph
            .traverse(
                "c2",
                GraphTraversalStrategy::Bfs,
                GraphDirection::Parents,
                8,
            )
            .expect("c2 should be present after rehydrate");
        let visited: Vec<String> = traversal.nodes.into_iter().map(|node| node.sha).collect();
        assert_eq!(visited, vec!["c2".to_string(), "c1".to_string()]);

        invalidate_query_graph_cache(&db_path);
        drop(conn);
        let _ = fs::remove_file(&db_path);
    }

    fn open_test_database(db_path: &Path) -> Connection {
        let mut conn = Connection::open(db_path).expect("open sqlite db");
        run_migrations(&mut conn).expect("run migrations");
        conn
    }

    fn seed_linear_graph(conn: &Connection, repo_path: &str, commits: &[&str]) -> i64 {
        let repo_id = upsert_repository(conn, repo_path, repo_path, &format!("{repo_path}/.git"))
            .expect("upsert repository");
        if commits.is_empty() {
            return repo_id;
        }

        for sha in commits {
            conn.execute(
                "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
                params![repo_id, *sha],
            )
            .expect("insert commit");
        }

        for window in commits.windows(2) {
            let parent = window[0];
            let child = window[1];
            let parent_rowid = load_commit_rowid(conn, repo_id, parent);
            let child_rowid = load_commit_rowid(conn, repo_id, child);
            conn.execute(
                "INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id) VALUES (?1, ?2, ?3)",
                params![repo_id, child_rowid, parent_rowid],
            )
            .expect("insert commit edge");
        }
        repo_id
    }

    fn append_commit_with_parent(conn: &Connection, repo_id: i64, sha: &str, parent_sha: &str) {
        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, sha],
        )
        .expect("insert appended commit");
        let child_rowid = load_commit_rowid(conn, repo_id, sha);
        let parent_rowid = load_commit_rowid(conn, repo_id, parent_sha);
        conn.execute(
            "INSERT INTO commit_edges (repo_id, child_commit_id, parent_commit_id) VALUES (?1, ?2, ?3)",
            params![repo_id, child_rowid, parent_rowid],
        )
        .expect("insert appended edge");
    }

    fn load_commit_rowid(conn: &Connection, repo_id: i64, sha: &str) -> i64 {
        conn.query_row(
            "SELECT commit_id FROM commits WHERE repo_id = ?1 AND sha = ?2",
            params![repo_id, sha],
            |row| row.get(0),
        )
        .expect("load commit rowid")
    }

    fn unique_temp_db_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("gitgraph-query-cache-{label}-{nanos}.db"))
    }
}
