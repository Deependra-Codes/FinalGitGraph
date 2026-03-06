use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::models::{
    CommitWindowCursor, CommitWindowItem, GraphDirection, IndexCheckpoint, ParsedFileChange,
    ParsedRef,
};
use crate::support::clock::now_epoch_seconds;
use crate::support::error::AppResult;

const MIGRATION_0001: &str = include_str!("../../migrations/0001_initial.sql");
const MIGRATION_0002: &str = include_str!("../../migrations/0002_repo_scoped_commit_keys.sql");
const MIGRATION_0003: &str = include_str!("../../migrations/0003_indexing_checkpoints.sql");
const MIGRATIONS: &[(i64, &str)] = &[
    (1, MIGRATION_0001),
    (2, MIGRATION_0002),
    (3, MIGRATION_0003),
];

pub fn open_database(db_path: &Path) -> AppResult<Connection> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        r#"
        PRAGMA foreign_keys=ON;
        PRAGMA journal_mode=WAL;
        PRAGMA synchronous=NORMAL;
        PRAGMA temp_store=MEMORY;
        PRAGMA busy_timeout=5000;
    "#,
    )?;
    // Hot graph query paths prepare many repeated statements.
    conn.set_prepared_statement_cache_capacity(256);
    Ok(conn)
}

pub fn run_migrations(conn: &mut Connection) -> AppResult<()> {
    ensure_schema_migrations_table(conn)?;

    for (version, sql) in MIGRATIONS {
        let already_applied: i64 = conn.query_row(
            "SELECT COUNT(1) FROM schema_migrations WHERE version = ?1",
            params![version],
            |row| row.get(0),
        )?;

        if already_applied == 0 {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at_epoch) VALUES (?1, ?2)",
                params![version, now_epoch_seconds()],
            )?;
            tx.commit()?;
        }
    }

    Ok(())
}

fn ensure_schema_migrations_table(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
          version INTEGER PRIMARY KEY,
          applied_at_epoch INTEGER NOT NULL
        );
    "#,
    )?;
    Ok(())
}

pub fn upsert_repository(
    conn: &Connection,
    path: &str,
    name: &str,
    git_dir: &str,
) -> AppResult<i64> {
    conn.execute(
        r#"
        INSERT INTO repositories (path, name, git_dir)
        VALUES (?1, ?2, ?3)
        ON CONFLICT(path) DO UPDATE SET
          name = excluded.name,
          git_dir = excluded.git_dir
    "#,
        params![path, name, git_dir],
    )?;

    let repo_id: i64 = conn.query_row(
        "SELECT id FROM repositories WHERE path = ?1",
        [path],
        |row| row.get(0),
    )?;
    Ok(repo_id)
}

pub fn clear_repository_index_data(conn: &Connection, repo_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM commit_edges WHERE repo_id = ?1", [repo_id])?;
    conn.execute("DELETE FROM commits WHERE repo_id = ?1", [repo_id])?;
    conn.execute("DELETE FROM refs WHERE repo_id = ?1", [repo_id])?;
    conn.execute("DELETE FROM file_changes WHERE repo_id = ?1", [repo_id])?;
    Ok(())
}

pub fn delete_file_changes_for_commits(
    conn: &Connection,
    repo_id: i64,
    commit_shas: &[String],
) -> AppResult<()> {
    if commit_shas.is_empty() {
        return Ok(());
    }

    let mut stmt =
        conn.prepare("DELETE FROM file_changes WHERE repo_id = ?1 AND commit_sha = ?2")?;
    for sha in commit_shas {
        stmt.execute(params![repo_id, sha.as_str()])?;
    }

    Ok(())
}

pub fn upsert_file_changes(
    conn: &Connection,
    repo_id: i64,
    file_changes: &[ParsedFileChange],
) -> AppResult<usize> {
    if file_changes.is_empty() {
        return Ok(0);
    }

    let mut inserted = 0usize;
    let mut stmt = conn.prepare(
        r#"
        INSERT INTO file_changes (
          repo_id, commit_sha, file_path, additions, deletions, change_type
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(repo_id, commit_sha, file_path) DO UPDATE SET
          additions = excluded.additions,
          deletions = excluded.deletions,
          change_type = excluded.change_type
    "#,
    )?;

    for change in file_changes {
        stmt.execute(params![
            repo_id,
            change.commit_sha.as_str(),
            change.file_path.as_str(),
            change.additions,
            change.deletions,
            change.change_type.as_str(),
        ])?;
        inserted += 1;
    }

    Ok(inserted)
}

pub fn synchronize_refs(conn: &Connection, repo_id: i64, refs: &[ParsedRef]) -> AppResult<usize> {
    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_refs (
          ref_name TEXT PRIMARY KEY
        );
        DELETE FROM current_refs;
    "#,
    )?;

    {
        let mut current_refs_stmt = conn.prepare(
            r#"
            INSERT INTO current_refs (ref_name)
            VALUES (?1)
            ON CONFLICT(ref_name) DO NOTHING
        "#,
        )?;

        let mut ref_stmt = conn.prepare(
            r#"
            INSERT INTO refs (repo_id, ref_name, target_sha, ref_type)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(repo_id, ref_name) DO UPDATE SET
              target_sha = excluded.target_sha,
              ref_type = excluded.ref_type
        "#,
        )?;

        for reference in refs {
            current_refs_stmt.execute([reference.ref_name.as_str()])?;
            ref_stmt.execute(params![
                repo_id,
                reference.ref_name.as_str(),
                reference.target_sha.as_str(),
                reference.ref_type.as_str()
            ])?;
        }
    }

    conn.execute(
        "DELETE FROM refs WHERE repo_id = ?1 AND ref_name NOT IN (SELECT ref_name FROM current_refs)",
        [repo_id],
    )?;

    Ok(refs.len())
}

pub fn set_repository_indexed_at(
    conn: &Connection,
    repo_id: i64,
    indexed_at_epoch: i64,
) -> AppResult<()> {
    conn.execute(
        "UPDATE repositories SET indexed_at = ?1 WHERE id = ?2",
        params![indexed_at_epoch.to_string(), repo_id],
    )?;
    Ok(())
}

pub fn load_index_checkpoint(
    conn: &Connection,
    repo_id: i64,
) -> AppResult<Option<IndexCheckpoint>> {
    let checkpoint = conn
        .query_row(
            r#"
            SELECT last_committer_time, last_commit_sha, indexed_at_epoch
            FROM indexing_checkpoints
            WHERE repo_id = ?1
        "#,
            [repo_id],
            |row| {
                Ok(IndexCheckpoint {
                    last_committer_time: row.get(0)?,
                    last_commit_sha: row.get(1)?,
                    indexed_at_epoch: row.get(2)?,
                })
            },
        )
        .optional()?;
    Ok(checkpoint)
}

pub fn upsert_index_checkpoint(
    conn: &Connection,
    repo_id: i64,
    checkpoint: &IndexCheckpoint,
) -> AppResult<()> {
    conn.execute(
        r#"
        INSERT INTO indexing_checkpoints (
          repo_id,
          last_committer_time,
          last_commit_sha,
          indexed_at_epoch
        ) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(repo_id) DO UPDATE SET
          last_committer_time = excluded.last_committer_time,
          last_commit_sha = excluded.last_commit_sha,
          indexed_at_epoch = excluded.indexed_at_epoch
    "#,
        params![
            repo_id,
            checkpoint.last_committer_time,
            checkpoint.last_commit_sha.as_str(),
            checkpoint.indexed_at_epoch,
        ],
    )?;
    Ok(())
}

pub fn load_commit_window(
    conn: &Connection,
    repo_id: i64,
    limit: usize,
    cursor: Option<&CommitWindowCursor>,
) -> AppResult<Vec<CommitWindowItem>> {
    let limit = limit.min(500);

    let mut commits = Vec::new();
    if let Some(cursor) = cursor {
        let mut stmt = conn.prepare(
            r#"
            SELECT sha, author_name, author_email, committer_time, subject, tree_sha
            FROM commits
            WHERE repo_id = ?1
              AND (committer_time < ?2 OR (committer_time = ?2 AND sha < ?3))
            ORDER BY committer_time DESC, sha DESC
            LIMIT ?4
        "#,
        )?;
        let rows = stmt.query_map(
            params![
                repo_id,
                cursor.committer_time,
                cursor.sha.as_str(),
                limit as i64
            ],
            map_commit_window_item,
        )?;

        for row in rows {
            commits.push(row?);
        }
    } else {
        let mut stmt = conn.prepare(
            r#"
            SELECT sha, author_name, author_email, committer_time, subject, tree_sha
            FROM commits
            WHERE repo_id = ?1
            ORDER BY committer_time DESC, sha DESC
            LIMIT ?2
        "#,
        )?;
        let rows = stmt.query_map(params![repo_id, limit as i64], map_commit_window_item)?;

        for row in rows {
            commits.push(row?);
        }
    }

    Ok(commits)
}

pub fn commit_exists(conn: &Connection, repo_id: i64, sha: &str) -> AppResult<bool> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM commits WHERE repo_id = ?1 AND sha = ?2 LIMIT 1",
            params![repo_id, sha],
            |_row| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

pub fn count_commits(conn: &Connection, repo_id: i64) -> AppResult<usize> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(1) FROM commits WHERE repo_id = ?1",
        [repo_id],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as usize)
}

pub fn load_commit_rowid(conn: &Connection, repo_id: i64, sha: &str) -> AppResult<Option<i64>> {
    let rowid = conn
        .query_row(
            "SELECT rowid FROM commits WHERE repo_id = ?1 AND sha = ?2 LIMIT 1",
            params![repo_id, sha],
            |row| row.get(0),
        )
        .optional()?;
    Ok(rowid)
}

pub fn load_commit_shas_by_rowids(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
) -> AppResult<HashMap<i64, String>> {
    if rowids.is_empty() {
        return Ok(HashMap::new());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_rowids (
          commit_rowid INTEGER PRIMARY KEY
        );
        DELETE FROM current_graph_rowids;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare(
            r#"
            INSERT INTO current_graph_rowids (commit_rowid)
            VALUES (?1)
            ON CONFLICT(commit_rowid) DO NOTHING
        "#,
        )?;
        for rowid in rowids {
            insert_stmt.execute([rowid])?;
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT c.rowid, c.sha
        FROM commits c
        JOIN current_graph_rowids r ON r.commit_rowid = c.rowid
        WHERE c.repo_id = ?1
    "#,
    )?;
    let rows = stmt.query_map([repo_id], |row| {
        let rowid: i64 = row.get(0)?;
        let sha: String = row.get(1)?;
        Ok((rowid, sha))
    })?;

    let mut result = HashMap::new();
    for row in rows {
        let (rowid, sha) = row?;
        result.insert(rowid, sha);
    }

    Ok(result)
}

pub fn load_commit_shas_for_rowids(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
) -> AppResult<Vec<String>> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_rowid_order (
          ord INTEGER PRIMARY KEY,
          commit_rowid INTEGER NOT NULL
        );
        DELETE FROM current_graph_rowid_order;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_graph_rowid_order (ord, commit_rowid)
            VALUES (?1, ?2)
        "#,
        )?;
        for (ord, rowid) in rowids.iter().enumerate() {
            insert_stmt.execute(params![ord as i64, rowid])?;
        }
    }

    let mut stmt = conn.prepare_cached(
        r#"
        SELECT c.sha
        FROM current_graph_rowid_order o
        JOIN commits c ON c.rowid = o.commit_rowid
        WHERE c.repo_id = ?1
        ORDER BY o.ord ASC
    "#,
    )?;
    let rows = stmt.query_map([repo_id], |row| row.get(0))?;

    let mut result = Vec::with_capacity(rowids.len());
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

pub fn load_neighbor_shas(
    conn: &Connection,
    repo_id: i64,
    sha: &str,
    direction: GraphDirection,
) -> AppResult<Vec<String>> {
    match direction {
        GraphDirection::Parents => load_parent_shas(conn, repo_id, sha),
        GraphDirection::Children => load_child_shas(conn, repo_id, sha),
        GraphDirection::Both => {
            let mut combined = load_parent_shas(conn, repo_id, sha)?;
            combined.extend(load_child_shas(conn, repo_id, sha)?);
            combined.sort();
            combined.dedup();
            Ok(combined)
        }
    }
}

pub fn load_neighbor_shas_batch(
    conn: &Connection,
    repo_id: i64,
    shas: &[String],
    direction: GraphDirection,
) -> AppResult<HashMap<String, Vec<String>>> {
    if shas.is_empty() {
        return Ok(HashMap::new());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_nodes (
          sha TEXT PRIMARY KEY
        );
        DELETE FROM current_graph_nodes;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare(
            r#"
            INSERT INTO current_graph_nodes (sha)
            VALUES (?1)
            ON CONFLICT(sha) DO NOTHING
        "#,
        )?;
        for sha in shas {
            insert_stmt.execute([sha.as_str()])?;
        }
    }

    let mut neighbors = match direction {
        GraphDirection::Parents => query_neighbors_for_sources(
            conn,
            repo_id,
            "child_sha",
            "parent_sha",
            "parent_sha DESC",
        )?,
        GraphDirection::Children => {
            query_neighbors_for_sources(conn, repo_id, "parent_sha", "child_sha", "child_sha DESC")?
        }
        GraphDirection::Both => {
            let mut merged = query_neighbors_for_sources(
                conn,
                repo_id,
                "child_sha",
                "parent_sha",
                "parent_sha DESC",
            )?;
            let children = query_neighbors_for_sources(
                conn,
                repo_id,
                "parent_sha",
                "child_sha",
                "child_sha DESC",
            )?;

            for (source_sha, mut child_neighbors) in children {
                let entry = merged.entry(source_sha).or_default();
                entry.append(&mut child_neighbors);
                entry.sort();
                entry.dedup();
                entry.reverse();
            }
            merged
        }
    };

    for sha in shas {
        neighbors.entry(sha.clone()).or_default();
    }

    Ok(neighbors)
}

pub fn load_neighbor_rowids(
    conn: &Connection,
    repo_id: i64,
    rowid: i64,
    direction: GraphDirection,
) -> AppResult<Vec<i64>> {
    match direction {
        GraphDirection::Parents => load_parent_rowids(conn, repo_id, rowid),
        GraphDirection::Children => load_child_rowids(conn, repo_id, rowid),
        GraphDirection::Both => {
            let mut combined = load_parent_rowids(conn, repo_id, rowid)?;
            combined.extend(load_child_rowids(conn, repo_id, rowid)?);
            combined.sort();
            combined.dedup();
            combined.reverse();
            Ok(combined)
        }
    }
}

pub fn load_neighbor_rowids_batch(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
    direction: GraphDirection,
) -> AppResult<HashMap<i64, Vec<i64>>> {
    let ordered_neighbors = load_neighbor_rowids_for_sources(conn, repo_id, rowids, direction)?;

    let mut neighbors = HashMap::with_capacity(rowids.len());
    for (ord, source_rowid) in rowids.iter().enumerate() {
        neighbors.insert(
            *source_rowid,
            ordered_neighbors.get(ord).cloned().unwrap_or_default(),
        );
    }

    Ok(neighbors)
}

pub fn load_neighbor_rowids_for_sources(
    conn: &Connection,
    repo_id: i64,
    rowids: &[i64],
    direction: GraphDirection,
) -> AppResult<Vec<Vec<i64>>> {
    if rowids.is_empty() {
        return Ok(Vec::new());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_rowid_order (
          ord INTEGER PRIMARY KEY,
          commit_rowid INTEGER NOT NULL
        );
        DELETE FROM current_graph_rowid_order;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare_cached(
            r#"
            INSERT INTO current_graph_rowid_order (ord, commit_rowid)
            VALUES (?1, ?2)
        "#,
        )?;
        for (ord, rowid) in rowids.iter().enumerate() {
            insert_stmt.execute(params![ord as i64, rowid])?;
        }
    }

    let mut neighbors = match direction {
        GraphDirection::Parents => query_neighbor_rowids_for_ordered_sources(
            conn,
            repo_id,
            "child_sha",
            "parent_sha",
            "neighbor.rowid DESC",
        )?,
        GraphDirection::Children => query_neighbor_rowids_for_ordered_sources(
            conn,
            repo_id,
            "parent_sha",
            "child_sha",
            "neighbor.rowid DESC",
        )?,
        GraphDirection::Both => {
            let mut merged = query_neighbor_rowids_for_ordered_sources(
                conn,
                repo_id,
                "child_sha",
                "parent_sha",
                "neighbor.rowid DESC",
            )?;
            if merged.len() < rowids.len() {
                merged.resize_with(rowids.len(), Vec::new);
            }

            let mut children = query_neighbor_rowids_for_ordered_sources(
                conn,
                repo_id,
                "parent_sha",
                "child_sha",
                "neighbor.rowid DESC",
            )?;
            if children.len() < rowids.len() {
                children.resize_with(rowids.len(), Vec::new);
            }

            for (merged_neighbors, mut child_neighbors) in
                merged.iter_mut().zip(children.into_iter())
            {
                merged_neighbors.append(&mut child_neighbors);
                merged_neighbors.sort_unstable();
                merged_neighbors.dedup();
                merged_neighbors.reverse();
            }
            merged
        }
    };

    if neighbors.len() < rowids.len() {
        neighbors.resize_with(rowids.len(), Vec::new);
    }

    Ok(neighbors)
}

pub fn load_pull_neighbor_rowids(
    conn: &Connection,
    repo_id: i64,
    frontier_rowids: &[i64],
    direction: GraphDirection,
) -> AppResult<Vec<i64>> {
    if frontier_rowids.is_empty() {
        return Ok(Vec::new());
    }

    conn.execute_batch(
        r#"
        CREATE TEMP TABLE IF NOT EXISTS current_graph_frontier (
          commit_rowid INTEGER PRIMARY KEY
        );
        DELETE FROM current_graph_frontier;
    "#,
    )?;

    {
        let mut insert_stmt = conn.prepare(
            r#"
            INSERT INTO current_graph_frontier (commit_rowid)
            VALUES (?1)
            ON CONFLICT(commit_rowid) DO NOTHING
        "#,
        )?;
        for rowid in frontier_rowids {
            insert_stmt.execute([rowid])?;
        }
    }

    match direction {
        GraphDirection::Parents => {
            query_pull_rowids_for_frontier(conn, repo_id, "parent_sha", "child_sha")
        }
        GraphDirection::Children => {
            query_pull_rowids_for_frontier(conn, repo_id, "child_sha", "parent_sha")
        }
        GraphDirection::Both => {
            let mut combined =
                query_pull_rowids_for_frontier(conn, repo_id, "parent_sha", "child_sha")?;
            combined.extend(query_pull_rowids_for_frontier(
                conn,
                repo_id,
                "child_sha",
                "parent_sha",
            )?);
            combined.sort_unstable_by(|left, right| right.cmp(left));
            combined.dedup();
            Ok(combined)
        }
    }
}

fn load_parent_shas(conn: &Connection, repo_id: i64, sha: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT parent_sha
        FROM commit_edges
        WHERE repo_id = ?1 AND child_sha = ?2
        ORDER BY parent_sha DESC
    "#,
    )?;
    let rows = stmt.query_map(params![repo_id, sha], |row| row.get(0))?;

    let mut shas = Vec::new();
    for row in rows {
        shas.push(row?);
    }

    Ok(shas)
}

fn load_child_shas(conn: &Connection, repo_id: i64, sha: &str) -> AppResult<Vec<String>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT child_sha
        FROM commit_edges
        WHERE repo_id = ?1 AND parent_sha = ?2
        ORDER BY child_sha DESC
    "#,
    )?;
    let rows = stmt.query_map(params![repo_id, sha], |row| row.get(0))?;

    let mut shas = Vec::new();
    for row in rows {
        shas.push(row?);
    }

    Ok(shas)
}

fn load_parent_rowids(conn: &Connection, repo_id: i64, rowid: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT parent.rowid
        FROM commit_edges edge
        JOIN commits child
          ON child.repo_id = edge.repo_id
         AND child.sha = edge.child_sha
        JOIN commits parent
          ON parent.repo_id = edge.repo_id
         AND parent.sha = edge.parent_sha
        WHERE edge.repo_id = ?1 AND child.rowid = ?2
        ORDER BY parent.rowid DESC
    "#,
    )?;
    let rows = stmt.query_map(params![repo_id, rowid], |row| row.get(0))?;

    let mut rowids = Vec::new();
    for row in rows {
        rowids.push(row?);
    }

    Ok(rowids)
}

fn load_child_rowids(conn: &Connection, repo_id: i64, rowid: i64) -> AppResult<Vec<i64>> {
    let mut stmt = conn.prepare_cached(
        r#"
        SELECT child.rowid
        FROM commit_edges edge
        JOIN commits parent
          ON parent.repo_id = edge.repo_id
         AND parent.sha = edge.parent_sha
        JOIN commits child
          ON child.repo_id = edge.repo_id
         AND child.sha = edge.child_sha
        WHERE edge.repo_id = ?1 AND parent.rowid = ?2
        ORDER BY child.rowid DESC
    "#,
    )?;
    let rows = stmt.query_map(params![repo_id, rowid], |row| row.get(0))?;

    let mut rowids = Vec::new();
    for row in rows {
        rowids.push(row?);
    }

    Ok(rowids)
}

fn query_neighbors_for_sources(
    conn: &Connection,
    repo_id: i64,
    source_column: &str,
    neighbor_column: &str,
    neighbor_order: &str,
) -> AppResult<HashMap<String, Vec<String>>> {
    let sql = format!(
        r#"
        SELECT edge.{source_column} AS source_sha, edge.{neighbor_column} AS neighbor_sha
        FROM commit_edges edge
        JOIN current_graph_nodes node ON node.sha = edge.{source_column}
        WHERE edge.repo_id = ?1
        ORDER BY edge.{source_column} DESC, {neighbor_order}
    "#
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([repo_id], |row| {
        let source_sha: String = row.get(0)?;
        let neighbor_sha: String = row.get(1)?;
        Ok((source_sha, neighbor_sha))
    })?;

    let mut result = HashMap::new();
    for row in rows {
        let (source_sha, neighbor_sha) = row?;
        result
            .entry(source_sha)
            .or_insert_with(Vec::new)
            .push(neighbor_sha);
    }

    Ok(result)
}

fn query_neighbor_rowids_for_ordered_sources(
    conn: &Connection,
    repo_id: i64,
    source_sha_column: &str,
    neighbor_sha_column: &str,
    neighbor_order: &str,
) -> AppResult<Vec<Vec<i64>>> {
    let sql = format!(
        r#"
        SELECT o.ord AS source_ord, neighbor.rowid AS neighbor_rowid
        FROM current_graph_rowid_order o
        JOIN commits source
          ON source.rowid = o.commit_rowid
         AND source.repo_id = ?1
        JOIN commit_edges edge
          ON edge.repo_id = source.repo_id
         AND edge.{source_sha_column} = source.sha
        JOIN commits neighbor
          ON neighbor.repo_id = edge.repo_id
         AND neighbor.sha = edge.{neighbor_sha_column}
        ORDER BY o.ord ASC, {neighbor_order}
    "#
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([repo_id], |row| {
        let source_ord: i64 = row.get(0)?;
        let neighbor_rowid: i64 = row.get(1)?;
        Ok((source_ord, neighbor_rowid))
    })?;

    let mut result: Vec<Vec<i64>> = Vec::new();
    for row in rows {
        let (source_ord, neighbor_rowid) = row?;
        if source_ord < 0 {
            continue;
        }
        let source_index = source_ord as usize;
        if source_index >= result.len() {
            result.resize_with(source_index + 1, Vec::new);
        }
        result[source_index].push(neighbor_rowid);
    }

    Ok(result)
}

fn query_pull_rowids_for_frontier(
    conn: &Connection,
    repo_id: i64,
    candidate_sha_column: &str,
    frontier_sha_column: &str,
) -> AppResult<Vec<i64>> {
    let sql = format!(
        r#"
        SELECT DISTINCT neighbor.rowid AS neighbor_rowid
        FROM commit_edges edge
        JOIN commits source
          ON source.repo_id = edge.repo_id
         AND source.sha = edge.{frontier_sha_column}
        JOIN current_graph_frontier frontier
          ON frontier.commit_rowid = source.rowid
        JOIN commits neighbor
          ON neighbor.repo_id = edge.repo_id
         AND neighbor.sha = edge.{candidate_sha_column}
        WHERE edge.repo_id = ?1
        ORDER BY neighbor.rowid DESC
    "#
    );

    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([repo_id], |row| row.get(0))?;

    let mut result = Vec::new();
    for row in rows {
        result.push(row?);
    }
    Ok(result)
}

fn map_commit_window_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<CommitWindowItem> {
    Ok(CommitWindowItem {
        sha: row.get(0)?,
        author_name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
        author_email: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        committer_time: row.get::<_, Option<i64>>(3)?.unwrap_or_default(),
        subject: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        tree_sha: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use rusqlite::{params, Connection};

    use super::{
        count_commits, load_commit_shas_by_rowids, load_commit_shas_for_rowids,
        load_index_checkpoint, load_neighbor_rowids_batch, load_neighbor_rowids_for_sources,
        load_neighbor_shas_batch, load_pull_neighbor_rowids, run_migrations,
        upsert_index_checkpoint,
    };
    use crate::domain::models::{GraphDirection, IndexCheckpoint};

    #[test]
    fn migrations_are_idempotent() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("first migration run");
        run_migrations(&mut conn).expect("second migration run");
    }

    #[test]
    fn commits_and_edges_have_repo_scoped_primary_keys() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        let commits_pk = primary_key_columns(&conn, "commits");
        assert_eq!(commits_pk, vec!["repo_id".to_string(), "sha".to_string()]);

        let edges_pk = primary_key_columns(&conn, "commit_edges");
        assert_eq!(
            edges_pk,
            vec![
                "repo_id".to_string(),
                "child_sha".to_string(),
                "parent_sha".to_string()
            ]
        );
    }

    #[test]
    fn indexing_checkpoint_roundtrip() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/repo", "repo", "/tmp/repo/.git"],
        )
        .expect("insert repo");

        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        let checkpoint = IndexCheckpoint {
            last_committer_time: 123,
            last_commit_sha: "abc123".to_string(),
            indexed_at_epoch: 456,
        };
        upsert_index_checkpoint(&conn, repo_id, &checkpoint).expect("insert checkpoint");

        let loaded = load_index_checkpoint(&conn, repo_id)
            .expect("load checkpoint")
            .expect("checkpoint should exist");
        assert_eq!(loaded.last_committer_time, 123);
        assert_eq!(loaded.last_commit_sha, "abc123");
        assert_eq!(loaded.indexed_at_epoch, 456);
    }

    #[test]
    fn neighbor_batch_lookup_returns_repo_scoped_neighbors() {
        let mut conn = Connection::open_in_memory().expect("open memory db");
        run_migrations(&mut conn).expect("migration run");

        conn.execute(
            "INSERT INTO repositories (path, name, git_dir) VALUES (?1, ?2, ?3)",
            ["/tmp/repo", "repo", "/tmp/repo/.git"],
        )
        .expect("insert repo");
        let repo_id: i64 = conn
            .query_row(
                "SELECT id FROM repositories WHERE path = ?1",
                ["/tmp/repo"],
                |row| row.get(0),
            )
            .expect("fetch repo id");

        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, "c1"],
        )
        .expect("insert c1");
        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, "c2"],
        )
        .expect("insert c2");
        conn.execute(
            "INSERT INTO commits (repo_id, sha) VALUES (?1, ?2)",
            params![repo_id, "c3"],
        )
        .expect("insert c3");

        conn.execute(
            "INSERT INTO commit_edges (repo_id, child_sha, parent_sha) VALUES (?1, ?2, ?3)",
            params![repo_id, "c2", "c1"],
        )
        .expect("insert edge c2->c1");
        conn.execute(
            "INSERT INTO commit_edges (repo_id, child_sha, parent_sha) VALUES (?1, ?2, ?3)",
            params![repo_id, "c3", "c2"],
        )
        .expect("insert edge c3->c2");

        let c1_rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM commits WHERE repo_id = ?1 AND sha = ?2",
                params![repo_id, "c1"],
                |row| row.get(0),
            )
            .expect("fetch c1 rowid");
        let c2_rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM commits WHERE repo_id = ?1 AND sha = ?2",
                params![repo_id, "c2"],
                |row| row.get(0),
            )
            .expect("fetch c2 rowid");
        let c3_rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM commits WHERE repo_id = ?1 AND sha = ?2",
                params![repo_id, "c3"],
                |row| row.get(0),
            )
            .expect("fetch c3 rowid");

        let batch = vec!["c1".to_string(), "c2".to_string(), "c3".to_string()];
        let parents = load_neighbor_shas_batch(&conn, repo_id, &batch, GraphDirection::Parents)
            .expect("load parent neighbors");
        assert_eq!(parents.get("c3"), Some(&vec!["c2".to_string()]));
        assert_eq!(parents.get("c2"), Some(&vec!["c1".to_string()]));
        assert_eq!(parents.get("c1"), Some(&Vec::new()));

        let children = load_neighbor_shas_batch(&conn, repo_id, &batch, GraphDirection::Children)
            .expect("load child neighbors");
        assert_eq!(children.get("c1"), Some(&vec!["c2".to_string()]));
        assert_eq!(children.get("c2"), Some(&vec!["c3".to_string()]));
        assert_eq!(children.get("c3"), Some(&Vec::new()));

        let mut both_for_c2 =
            load_neighbor_shas_batch(&conn, repo_id, &batch, GraphDirection::Both)
                .expect("load both neighbors")
                .remove("c2")
                .unwrap_or_default();
        both_for_c2.sort();
        assert_eq!(both_for_c2, vec!["c1".to_string(), "c3".to_string()]);

        let rowid_batch = vec![c1_rowid, c2_rowid, c3_rowid];
        let rowid_parents =
            load_neighbor_rowids_batch(&conn, repo_id, &rowid_batch, GraphDirection::Parents)
                .expect("load rowid parent neighbors");
        assert_eq!(rowid_parents.get(&c3_rowid), Some(&vec![c2_rowid]));
        assert_eq!(rowid_parents.get(&c2_rowid), Some(&vec![c1_rowid]));

        let ordered_children = load_neighbor_rowids_for_sources(
            &conn,
            repo_id,
            &[c2_rowid, c1_rowid, c3_rowid],
            GraphDirection::Children,
        )
        .expect("load ordered rowid children");
        assert_eq!(
            ordered_children,
            vec![vec![c3_rowid], vec![c2_rowid], Vec::new()]
        );

        let pull_children =
            load_pull_neighbor_rowids(&conn, repo_id, &[c1_rowid], GraphDirection::Children)
                .expect("pull children");
        assert_eq!(pull_children, vec![c2_rowid]);

        let pull_parents =
            load_pull_neighbor_rowids(&conn, repo_id, &[c3_rowid], GraphDirection::Parents)
                .expect("pull parents");
        assert_eq!(pull_parents, vec![c2_rowid]);

        let pull_both =
            load_pull_neighbor_rowids(&conn, repo_id, &[c2_rowid], GraphDirection::Both)
                .expect("pull both");
        assert_eq!(pull_both, vec![c3_rowid, c1_rowid]);

        let repo_commit_count = count_commits(&conn, repo_id).expect("count commits");
        assert_eq!(repo_commit_count, 3);

        let rowid_shas =
            load_commit_shas_by_rowids(&conn, repo_id, &rowid_batch).expect("load sha by rowids");
        assert_eq!(rowid_shas.get(&c1_rowid).map(String::as_str), Some("c1"));
        assert_eq!(rowid_shas.get(&c2_rowid).map(String::as_str), Some("c2"));
        assert_eq!(rowid_shas.get(&c3_rowid).map(String::as_str), Some("c3"));

        let ordered_shas =
            load_commit_shas_for_rowids(&conn, repo_id, &[c3_rowid, c1_rowid, c2_rowid])
                .expect("load ordered sha by rowids");
        assert_eq!(
            ordered_shas,
            vec!["c3".to_string(), "c1".to_string(), "c2".to_string()]
        );
    }

    fn primary_key_columns(conn: &Connection, table_name: &str) -> Vec<String> {
        let mut stmt = conn
            .prepare(&format!("PRAGMA table_info({table_name})"))
            .expect("prepare table_info");
        let mapped = stmt
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let pk_pos: i64 = row.get(5)?;
                Ok((name, pk_pos))
            })
            .expect("query table_info");

        let mut pairs = Vec::new();
        for row in mapped {
            let (name, pk_pos) = row.expect("read row");
            if pk_pos > 0 {
                pairs.push((pk_pos, name));
            }
        }
        pairs.sort_by_key(|(pk_pos, _)| *pk_pos);
        pairs.into_iter().map(|(_, name)| name).collect()
    }
}
