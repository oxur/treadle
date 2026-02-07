//! SQLite-backed state store implementation.
//!
//! This module provides [`SqliteStateStore`], a persistent implementation
//! of [`StateStore`] backed by SQLite.

use crate::{Result, StageState, TreadleError};
use async_trait::async_trait;
use rusqlite::{params, Connection};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::StateStore;

/// Schema version for migrations.
const SCHEMA_VERSION: i32 = 1;

/// SQL for creating the stage_states table.
const CREATE_STAGE_STATES_TABLE: &str = r#"
    CREATE TABLE IF NOT EXISTS stage_states (
        work_item_id TEXT NOT NULL,
        stage_name TEXT NOT NULL,
        state_json TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (work_item_id, stage_name)
    )
"#;

/// SQL for creating the work_items table.
const CREATE_WORK_ITEMS_TABLE: &str = r#"
    CREATE TABLE IF NOT EXISTS work_items (
        work_item_id TEXT NOT NULL PRIMARY KEY,
        data_json TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )
"#;

/// SQL for creating the schema_version table.
const CREATE_SCHEMA_VERSION_TABLE: &str = r#"
    CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER NOT NULL
    )
"#;

/// Index on stage_states for querying by stage.
const CREATE_STAGE_INDEX: &str = r#"
    CREATE INDEX IF NOT EXISTS idx_stage_name
    ON stage_states (stage_name)
"#;

/// A SQLite-backed implementation of [`StateStore`].
///
/// This store persists all workflow state to a SQLite database, making
/// it suitable for production use where state must survive process restarts.
///
/// # Thread Safety
///
/// The store wraps the SQLite connection in a `Mutex` and uses
/// `spawn_blocking` for all database operations, making it safe
/// for use in async contexts.
///
/// # Example
///
/// ```rust,ignore
/// use treadle::SqliteStateStore;
///
/// // Open or create a database file
/// let store = SqliteStateStore::open("workflow.db").await?;
///
/// // Or use an in-memory database for testing
/// let store = SqliteStateStore::open_in_memory().await?;
/// ```
pub struct SqliteStateStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStateStore {
    /// Opens a SQLite database at the given path.
    ///
    /// Creates the database and schema if they don't exist.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened or the
    /// schema cannot be created.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        let conn = tokio::task::spawn_blocking(move || Connection::open(&path))
            .await
            .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
            .map_err(|e| TreadleError::StateStore(format!("failed to open database: {}", e)))?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        store.run_migrations().await?;
        Ok(store)
    }

    /// Opens an in-memory SQLite database.
    ///
    /// Useful for testing. The database is lost when the store is dropped.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be created.
    pub async fn open_in_memory() -> Result<Self> {
        let conn = tokio::task::spawn_blocking(Connection::open_in_memory)
            .await
            .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
            .map_err(|e| {
                TreadleError::StateStore(format!("failed to open in-memory database: {}", e))
            })?;

        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };

        store.run_migrations().await?;
        Ok(store)
    }

    /// Runs schema migrations.
    async fn run_migrations(&self) -> Result<()> {
        let conn = Arc::clone(&self.conn);

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            // Create schema version table
            conn.execute(CREATE_SCHEMA_VERSION_TABLE, [])?;

            // Check current version
            let version: Option<i32> = conn
                .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                    row.get(0)
                })
                .ok();

            if version.is_none() || version.unwrap() < SCHEMA_VERSION {
                // Run migrations
                conn.execute(CREATE_STAGE_STATES_TABLE, [])?;
                conn.execute(CREATE_WORK_ITEMS_TABLE, [])?;
                conn.execute(CREATE_STAGE_INDEX, [])?;

                // Update version
                conn.execute("DELETE FROM schema_version", [])?;
                conn.execute(
                    "INSERT INTO schema_version (version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
            }

            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("migration failed: {}", e)))?;

        Ok(())
    }

    /// Checks if the required tables exist.
    ///
    /// Useful for testing that the schema was created correctly.
    pub async fn tables_exist(&self) -> Result<bool> {
        let conn = Arc::clone(&self.conn);

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            let tables: Vec<String> = {
                let mut stmt = conn.prepare(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('stage_states', 'work_items', 'schema_version')"
                )?;
                let rows = stmt.query_map([], |row| row.get(0))?;
                rows.filter_map(|r| r.ok()).collect()
            };

            Ok::<bool, rusqlite::Error>(tables.len() == 3)
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("table check failed: {}", e)))
    }
}

// Debug implementation that doesn't expose connection details
impl std::fmt::Debug for SqliteStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStateStore").finish_non_exhaustive()
    }
}

#[async_trait]
impl StateStore for SqliteStateStore {
    async fn save_stage_state(
        &mut self,
        work_item_id: &str,
        stage_name: &str,
        state: &StageState,
    ) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let work_item_id = work_item_id.to_string();
        let stage_name = stage_name.to_string();
        let state_json = serde_json::to_string(state)
            .map_err(|e| TreadleError::StateStore(format!("failed to serialize state: {}", e)))?;
        let updated_at = chrono::Utc::now().to_rfc3339();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            conn.execute(
                "INSERT OR REPLACE INTO stage_states (work_item_id, stage_name, state_json, updated_at) VALUES (?1, ?2, ?3, ?4)",
                params![work_item_id, stage_name, state_json, updated_at],
            )?;

            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("insert failed: {}", e)))?;

        Ok(())
    }

    async fn get_stage_state(
        &self,
        work_item_id: &str,
        stage_name: &str,
    ) -> Result<Option<StageState>> {
        let conn = Arc::clone(&self.conn);
        let work_item_id = work_item_id.to_string();
        let stage_name = stage_name.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            let result = conn.query_row(
                "SELECT state_json FROM stage_states WHERE work_item_id = ?1 AND stage_name = ?2",
                params![work_item_id, stage_name],
                |row| {
                    let state_json: String = row.get(0)?;
                    Ok(state_json)
                },
            );

            match result {
                Ok(state_json) => {
                    let state: StageState = serde_json::from_str(&state_json)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                    Ok(Some(state))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("query failed: {}", e)))
    }

    async fn get_all_stage_states(
        &self,
        work_item_id: &str,
    ) -> Result<HashMap<String, StageState>> {
        let conn = Arc::clone(&self.conn);
        let work_item_id = work_item_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            let mut stmt = conn.prepare(
                "SELECT stage_name, state_json FROM stage_states WHERE work_item_id = ?1",
            )?;

            let rows = stmt.query_map(params![work_item_id], |row| {
                let stage_name: String = row.get(0)?;
                let state_json: String = row.get(1)?;
                Ok((stage_name, state_json))
            })?;

            let mut result = HashMap::new();
            for row in rows {
                let (stage_name, state_json) = row?;
                let state: StageState = serde_json::from_str(&state_json)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                result.insert(stage_name, state);
            }

            Ok::<HashMap<String, StageState>, rusqlite::Error>(result)
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("query failed: {}", e)))
    }

    async fn save_work_item_data(&mut self, work_item_id: &str, data: &JsonValue) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let work_item_id = work_item_id.to_string();
        let data_json = serde_json::to_string(data)
            .map_err(|e| TreadleError::StateStore(format!("failed to serialize data: {}", e)))?;
        let updated_at = chrono::Utc::now().to_rfc3339();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            conn.execute(
                "INSERT OR REPLACE INTO work_items (work_item_id, data_json, updated_at) VALUES (?1, ?2, ?3)",
                params![work_item_id, data_json, updated_at],
            )?;

            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("insert failed: {}", e)))?;

        Ok(())
    }

    async fn get_work_item_data(&self, work_item_id: &str) -> Result<Option<JsonValue>> {
        let conn = Arc::clone(&self.conn);
        let work_item_id = work_item_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            let result = conn.query_row(
                "SELECT data_json FROM work_items WHERE work_item_id = ?1",
                params![work_item_id],
                |row| {
                    let data_json: String = row.get(0)?;
                    Ok(data_json)
                },
            );

            match result {
                Ok(data_json) => {
                    let data: JsonValue = serde_json::from_str(&data_json)
                        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                    Ok(Some(data))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("query failed: {}", e)))
    }

    async fn delete_work_item(&mut self, work_item_id: &str) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let work_item_id = work_item_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            conn.execute(
                "DELETE FROM stage_states WHERE work_item_id = ?1",
                params![work_item_id],
            )?;
            conn.execute(
                "DELETE FROM work_items WHERE work_item_id = ?1",
                params![work_item_id],
            )?;

            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("delete failed: {}", e)))?;

        Ok(())
    }

    async fn list_work_items(&self) -> Result<Vec<String>> {
        let conn = Arc::clone(&self.conn);

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            // Collect unique work item IDs from both tables
            let mut stmt = conn.prepare(
                "SELECT DISTINCT work_item_id FROM (
                    SELECT work_item_id FROM stage_states
                    UNION
                    SELECT work_item_id FROM work_items
                ) ORDER BY work_item_id",
            )?;

            let rows = stmt.query_map([], |row| row.get(0))?;

            let result: std::result::Result<Vec<String>, _> = rows.collect();
            result
        })
        .await
        .map_err(|e| TreadleError::StateStore(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStore(format!("query failed: {}", e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StageStatus;

    #[tokio::test]
    async fn test_open_in_memory() {
        let store = SqliteStateStore::open_in_memory().await;
        assert!(store.is_ok());
    }

    #[tokio::test]
    async fn test_tables_created() {
        let store = SqliteStateStore::open_in_memory().await.unwrap();
        assert!(store.tables_exist().await.unwrap());
    }

    #[tokio::test]
    async fn test_open_file_creates_db() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("treadle_test_{}.db", std::process::id()));

        // Ensure clean state
        let _ = std::fs::remove_file(&db_path);

        // Open creates the file
        let store = SqliteStateStore::open(&db_path).await.unwrap();
        assert!(store.tables_exist().await.unwrap());

        // Cleanup
        drop(store);
        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn test_reopen_preserves_schema() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("treadle_test_reopen_{}.db", std::process::id()));

        let _ = std::fs::remove_file(&db_path);

        // First open
        {
            let store = SqliteStateStore::open(&db_path).await.unwrap();
            assert!(store.tables_exist().await.unwrap());
        }

        // Second open
        {
            let store = SqliteStateStore::open(&db_path).await.unwrap();
            assert!(store.tables_exist().await.unwrap());
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn test_set_and_get_stage_state() {
        let mut store = SqliteStateStore::open_in_memory().await.unwrap();

        let status = store.get_stage_state("item-1", "scan").await.unwrap();
        assert!(status.is_none());

        let mut state = StageState::new();
        state.mark_in_progress();
        store
            .save_stage_state("item-1", "scan", &state)
            .await
            .unwrap();

        let status = store.get_stage_state("item-1", "scan").await.unwrap();
        assert!(status.is_some());
        assert_eq!(status.unwrap().status, StageStatus::InProgress);
    }

    #[tokio::test]
    async fn test_get_all_stage_states() {
        let mut store = SqliteStateStore::open_in_memory().await.unwrap();

        let mut state1 = StageState::new();
        state1.mark_complete();
        let mut state2 = StageState::new();
        state2.mark_in_progress();

        store
            .save_stage_state("item-1", "scan", &state1)
            .await
            .unwrap();
        store
            .save_stage_state("item-1", "enrich", &state2)
            .await
            .unwrap();

        let all = store.get_all_stage_states("item-1").await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all.get("scan").unwrap().status, StageStatus::Complete);
        assert_eq!(all.get("enrich").unwrap().status, StageStatus::InProgress);
    }

    #[tokio::test]
    async fn test_save_and_get_work_item_data() {
        let mut store = SqliteStateStore::open_in_memory().await.unwrap();
        let data = serde_json::json!({
            "id": "item-1",
            "name": "test item"
        });

        store.save_work_item_data("item-1", &data).await.unwrap();

        let retrieved = store.get_work_item_data("item-1").await.unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap(), data);
    }

    #[tokio::test]
    async fn test_delete_work_item() {
        let mut store = SqliteStateStore::open_in_memory().await.unwrap();
        let state = StageState::new();
        let data = serde_json::json!({"id": "item-1"});

        store
            .save_stage_state("item-1", "stage-1", &state)
            .await
            .unwrap();
        store.save_work_item_data("item-1", &data).await.unwrap();

        store.delete_work_item("item-1").await.unwrap();

        let stage_result = store.get_stage_state("item-1", "stage-1").await.unwrap();
        assert!(stage_result.is_none());

        let data_result = store.get_work_item_data("item-1").await.unwrap();
        assert!(data_result.is_none());
    }

    #[tokio::test]
    async fn test_list_work_items() {
        let mut store = SqliteStateStore::open_in_memory().await.unwrap();
        let data1 = serde_json::json!({"id": "item-1"});
        let data2 = serde_json::json!({"id": "item-2"});

        store.save_work_item_data("item-1", &data1).await.unwrap();
        store.save_work_item_data("item-2", &data2).await.unwrap();

        let items = store.list_work_items().await.unwrap();
        assert_eq!(items.len(), 2);
        assert!(items.contains(&"item-1".to_string()));
        assert!(items.contains(&"item-2".to_string()));
    }

    #[tokio::test]
    async fn test_persistence_across_reopens() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("treadle_persist_test_{}.db", std::process::id()));

        let _ = std::fs::remove_file(&db_path);

        // Write data
        {
            let mut store = SqliteStateStore::open(&db_path).await.unwrap();
            let mut state = StageState::new();
            state.mark_complete();
            store
                .save_stage_state("item-1", "scan", &state)
                .await
                .unwrap();
        }

        // Read after reopen
        {
            let store = SqliteStateStore::open(&db_path).await.unwrap();
            let status = store.get_stage_state("item-1", "scan").await.unwrap();

            assert!(status.is_some());
            assert_eq!(status.unwrap().status, StageStatus::Complete);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn test_update_stage_state() {
        let mut store = SqliteStateStore::open_in_memory().await.unwrap();
        let mut state = StageState::new();

        store
            .save_stage_state("item-1", "stage-1", &state)
            .await
            .unwrap();

        state.mark_complete();
        store
            .save_stage_state("item-1", "stage-1", &state)
            .await
            .unwrap();

        let retrieved = store
            .get_stage_state("item-1", "stage-1")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.status, StageStatus::Complete);
    }
}
