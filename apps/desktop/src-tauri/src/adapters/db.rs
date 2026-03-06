use std::fs;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::models::{
    CommitWindowCursor, CommitWindowItem, IndexCheckpoint, ParsedFileChange, ParsedRef,
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
    use rusqlite::Connection;

    use super::{load_index_checkpoint, run_migrations, upsert_index_checkpoint};
    use crate::domain::models::IndexCheckpoint;

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
