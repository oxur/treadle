# Treadle Phase 2 Implementation Plan

## Phase 2: State Store Implementations

**Goal:** Two working StateStore backends — in-memory (for testing) and SQLite (for production).

**Prerequisites:**
- Phase 1 completed (all traits and types defined)
- `cargo build` and `cargo test` pass

---

## Milestone 2.1 — In-Memory StateStore

### Files to Create/Modify
- `src/state_store/mod.rs` (refactor from `src/state_store.rs`)
- `src/state_store/memory.rs` (new)
- `src/lib.rs` (update exports)

### Implementation Details

#### 2.1.1 Refactor to Module Structure

Convert `src/state_store.rs` to a directory module.

**`src/state_store/mod.rs`:**

```rust
//! State persistence for workflow execution.
//!
//! This module provides the [`StateStore`] trait and implementations
//! for persisting workflow execution state.

mod memory;

pub use memory::MemoryStateStore;

// Re-export the trait (move from original state_store.rs)
use async_trait::async_trait;
use std::collections::HashMap;

use crate::{Result, StageState, StageStatus, SubTaskStatus, WorkItem};

/// A persistent store for workflow execution state.
///
/// The `StateStore` trait abstracts over different storage backends
/// (in-memory, SQLite, etc.) for persisting the execution state of
/// work items as they progress through a workflow.
#[async_trait]
pub trait StateStore<W: WorkItem>: Send + Sync {
    /// Retrieves the status of a stage for a work item.
    async fn get_status(&self, item_id: &W::Id, stage: &str) -> Result<Option<StageStatus>>;

    /// Sets the status of a stage for a work item.
    async fn set_status(&self, item_id: &W::Id, stage: &str, status: StageStatus) -> Result<()>;

    /// Retrieves all stage statuses for a work item.
    async fn get_all_statuses(&self, item_id: &W::Id) -> Result<HashMap<String, StageStatus>>;

    /// Queries for work items in a specific state for a stage.
    async fn query_items(&self, stage: &str, state: StageState) -> Result<Vec<String>>;

    /// Retrieves all subtask statuses for a fan-out stage.
    async fn get_subtask_statuses(
        &self,
        item_id: &W::Id,
        stage: &str,
    ) -> Result<Vec<SubTaskStatus>>;

    /// Sets the status of a specific subtask.
    async fn set_subtask_status(
        &self,
        item_id: &W::Id,
        stage: &str,
        subtask: &str,
        status: SubTaskStatus,
    ) -> Result<()>;
}
```

#### 2.1.2 Create `src/state_store/memory.rs`

```rust
//! In-memory state store implementation.
//!
//! This module provides [`MemoryStateStore`], a thread-safe in-memory
//! implementation of [`StateStore`] suitable for testing and development.

use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{Result, StageState, StageStatus, SubTaskStatus, WorkItem};
use super::StateStore;

/// Storage key for stage status.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct StageKey {
    item_id: String,
    stage: String,
}

impl StageKey {
    fn new(item_id: impl Display, stage: impl Into<String>) -> Self {
        Self {
            item_id: item_id.to_string(),
            stage: stage.into(),
        }
    }
}

/// Storage key for subtask status.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct SubTaskKey {
    item_id: String,
    stage: String,
    subtask: String,
}

impl SubTaskKey {
    fn new(item_id: impl Display, stage: impl Into<String>, subtask: impl Into<String>) -> Self {
        Self {
            item_id: item_id.to_string(),
            stage: stage.into(),
            subtask: subtask.into(),
        }
    }
}

/// Internal storage for the memory state store.
#[derive(Debug, Default)]
struct Storage {
    /// Stage statuses indexed by (item_id, stage).
    stages: HashMap<StageKey, StageStatus>,
    /// Subtask statuses indexed by (item_id, stage, subtask).
    subtasks: HashMap<SubTaskKey, SubTaskStatus>,
}

/// An in-memory implementation of [`StateStore`].
///
/// This implementation uses `Arc<RwLock<...>>` internally, making it
/// safe to clone and share across async tasks.
///
/// # Example
///
/// ```rust
/// use treadle::{MemoryStateStore, StateStore, StageStatus, WorkItem};
///
/// #[derive(Debug, Clone)]
/// struct MyItem { id: String }
///
/// impl WorkItem for MyItem {
///     type Id = String;
///     fn id(&self) -> &Self::Id { &self.id }
/// }
///
/// # async fn example() -> treadle::Result<()> {
/// let store = MemoryStateStore::<MyItem>::new();
///
/// // Store status
/// store.set_status(&"item-1".to_string(), "scan", StageStatus::completed()).await?;
///
/// // Retrieve status
/// let status = store.get_status(&"item-1".to_string(), "scan").await?;
/// assert!(status.is_some());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct MemoryStateStore<W: WorkItem> {
    storage: Arc<RwLock<Storage>>,
    _marker: std::marker::PhantomData<W>,
}

impl<W: WorkItem> MemoryStateStore<W> {
    /// Creates a new, empty in-memory state store.
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(Storage::default())),
            _marker: std::marker::PhantomData,
        }
    }

    /// Returns the number of stage status entries currently stored.
    ///
    /// Useful for testing.
    pub async fn stage_count(&self) -> usize {
        self.storage.read().await.stages.len()
    }

    /// Returns the number of subtask status entries currently stored.
    ///
    /// Useful for testing.
    pub async fn subtask_count(&self) -> usize {
        self.storage.read().await.subtasks.len()
    }

    /// Clears all stored data.
    ///
    /// Useful for resetting state between tests.
    pub async fn clear(&self) {
        let mut storage = self.storage.write().await;
        storage.stages.clear();
        storage.subtasks.clear();
    }
}

impl<W: WorkItem> Default for MemoryStateStore<W> {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<W: WorkItem> StateStore<W> for MemoryStateStore<W> {
    async fn get_status(&self, item_id: &W::Id, stage: &str) -> Result<Option<StageStatus>> {
        let storage = self.storage.read().await;
        let key = StageKey::new(item_id, stage);
        Ok(storage.stages.get(&key).cloned())
    }

    async fn set_status(
        &self,
        item_id: &W::Id,
        stage: &str,
        status: StageStatus,
    ) -> Result<()> {
        let mut storage = self.storage.write().await;
        let key = StageKey::new(item_id, stage);
        storage.stages.insert(key, status);
        Ok(())
    }

    async fn get_all_statuses(&self, item_id: &W::Id) -> Result<HashMap<String, StageStatus>> {
        let storage = self.storage.read().await;
        let item_id_str = item_id.to_string();

        let result = storage
            .stages
            .iter()
            .filter(|(k, _)| k.item_id == item_id_str)
            .map(|(k, v)| (k.stage.clone(), v.clone()))
            .collect();

        Ok(result)
    }

    async fn query_items(&self, stage: &str, state: StageState) -> Result<Vec<String>> {
        let storage = self.storage.read().await;

        let result = storage
            .stages
            .iter()
            .filter(|(k, v)| k.stage == stage && v.state == state)
            .map(|(k, _)| k.item_id.clone())
            .collect();

        Ok(result)
    }

    async fn get_subtask_statuses(
        &self,
        item_id: &W::Id,
        stage: &str,
    ) -> Result<Vec<SubTaskStatus>> {
        let storage = self.storage.read().await;
        let item_id_str = item_id.to_string();

        let result = storage
            .subtasks
            .iter()
            .filter(|(k, _)| k.item_id == item_id_str && k.stage == stage)
            .map(|(_, v)| v.clone())
            .collect();

        Ok(result)
    }

    async fn set_subtask_status(
        &self,
        item_id: &W::Id,
        stage: &str,
        subtask: &str,
        status: SubTaskStatus,
    ) -> Result<()> {
        let mut storage = self.storage.write().await;
        let key = SubTaskKey::new(item_id, stage, subtask);
        storage.subtasks.insert(key, status);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::test_helpers::TestItem;
    use crate::{ReviewData, StageState};

    type TestStore = MemoryStateStore<TestItem>;

    #[tokio::test]
    async fn test_new_store_is_empty() {
        let store = TestStore::new();
        assert_eq!(store.stage_count().await, 0);
        assert_eq!(store.subtask_count().await, 0);
    }

    #[tokio::test]
    async fn test_set_and_get_status() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Initially no status
        let status = store.get_status(&item_id, "scan").await.unwrap();
        assert!(status.is_none());

        // Set status
        store
            .set_status(&item_id, "scan", StageStatus::running())
            .await
            .unwrap();

        // Retrieve status
        let status = store.get_status(&item_id, "scan").await.unwrap();
        assert!(status.is_some());
        assert_eq!(status.unwrap().state, StageState::Running);
    }

    #[tokio::test]
    async fn test_set_status_overwrites() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        store
            .set_status(&item_id, "scan", StageStatus::pending())
            .await
            .unwrap();
        store
            .set_status(&item_id, "scan", StageStatus::completed())
            .await
            .unwrap();

        let status = store.get_status(&item_id, "scan").await.unwrap().unwrap();
        assert_eq!(status.state, StageState::Completed);
    }

    #[tokio::test]
    async fn test_get_all_statuses() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        store
            .set_status(&item_id, "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_status(&item_id, "enrich", StageStatus::running())
            .await
            .unwrap();
        store
            .set_status(&item_id, "review", StageStatus::pending())
            .await
            .unwrap();

        let all = store.get_all_statuses(&item_id).await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all.get("scan").unwrap().state, StageState::Completed);
        assert_eq!(all.get("enrich").unwrap().state, StageState::Running);
        assert_eq!(all.get("review").unwrap().state, StageState::Pending);
    }

    #[tokio::test]
    async fn test_get_all_statuses_different_items() {
        let store = TestStore::new();

        store
            .set_status(&"item-1".to_string(), "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_status(&"item-2".to_string(), "scan", StageStatus::pending())
            .await
            .unwrap();

        let item1_statuses = store.get_all_statuses(&"item-1".to_string()).await.unwrap();
        let item2_statuses = store.get_all_statuses(&"item-2".to_string()).await.unwrap();

        assert_eq!(item1_statuses.len(), 1);
        assert_eq!(item2_statuses.len(), 1);
        assert_eq!(
            item1_statuses.get("scan").unwrap().state,
            StageState::Completed
        );
        assert_eq!(
            item2_statuses.get("scan").unwrap().state,
            StageState::Pending
        );
    }

    #[tokio::test]
    async fn test_query_items() {
        let store = TestStore::new();

        store
            .set_status(&"item-1".to_string(), "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_status(&"item-2".to_string(), "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_status(&"item-3".to_string(), "scan", StageStatus::pending())
            .await
            .unwrap();

        let completed = store
            .query_items("scan", StageState::Completed)
            .await
            .unwrap();
        let pending = store
            .query_items("scan", StageState::Pending)
            .await
            .unwrap();
        let failed = store.query_items("scan", StageState::Failed).await.unwrap();

        assert_eq!(completed.len(), 2);
        assert!(completed.contains(&"item-1".to_string()));
        assert!(completed.contains(&"item-2".to_string()));
        assert_eq!(pending.len(), 1);
        assert!(pending.contains(&"item-3".to_string()));
        assert!(failed.is_empty());
    }

    #[tokio::test]
    async fn test_query_items_different_stages() {
        let store = TestStore::new();

        store
            .set_status(&"item-1".to_string(), "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_status(&"item-1".to_string(), "enrich", StageStatus::completed())
            .await
            .unwrap();

        let scan_completed = store
            .query_items("scan", StageState::Completed)
            .await
            .unwrap();
        let enrich_completed = store
            .query_items("enrich", StageState::Completed)
            .await
            .unwrap();

        assert_eq!(scan_completed.len(), 1);
        assert_eq!(enrich_completed.len(), 1);
    }

    #[tokio::test]
    async fn test_subtask_set_and_get() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Initially no subtasks
        let subtasks = store.get_subtask_statuses(&item_id, "enrich").await.unwrap();
        assert!(subtasks.is_empty());

        // Set subtask statuses
        store
            .set_subtask_status(
                &item_id,
                "enrich",
                "source-1",
                SubTaskStatus::pending("source-1"),
            )
            .await
            .unwrap();
        store
            .set_subtask_status(
                &item_id,
                "enrich",
                "source-2",
                SubTaskStatus::completed("source-2"),
            )
            .await
            .unwrap();

        // Retrieve subtasks
        let subtasks = store.get_subtask_statuses(&item_id, "enrich").await.unwrap();
        assert_eq!(subtasks.len(), 2);

        let names: Vec<_> = subtasks.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"source-1"));
        assert!(names.contains(&"source-2"));
    }

    #[tokio::test]
    async fn test_subtask_overwrites() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        store
            .set_subtask_status(
                &item_id,
                "enrich",
                "source-1",
                SubTaskStatus::pending("source-1"),
            )
            .await
            .unwrap();
        store
            .set_subtask_status(
                &item_id,
                "enrich",
                "source-1",
                SubTaskStatus::completed("source-1"),
            )
            .await
            .unwrap();

        let subtasks = store.get_subtask_statuses(&item_id, "enrich").await.unwrap();
        assert_eq!(subtasks.len(), 1);
        assert_eq!(subtasks[0].state, StageState::Completed);
    }

    #[tokio::test]
    async fn test_subtasks_different_stages() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        store
            .set_subtask_status(
                &item_id,
                "enrich",
                "sub-1",
                SubTaskStatus::pending("sub-1"),
            )
            .await
            .unwrap();
        store
            .set_subtask_status(
                &item_id,
                "process",
                "sub-1",
                SubTaskStatus::completed("sub-1"),
            )
            .await
            .unwrap();

        let enrich_subtasks = store.get_subtask_statuses(&item_id, "enrich").await.unwrap();
        let process_subtasks = store.get_subtask_statuses(&item_id, "process").await.unwrap();

        assert_eq!(enrich_subtasks.len(), 1);
        assert_eq!(enrich_subtasks[0].state, StageState::Pending);
        assert_eq!(process_subtasks.len(), 1);
        assert_eq!(process_subtasks[0].state, StageState::Completed);
    }

    #[tokio::test]
    async fn test_missing_item_returns_none() {
        let store = TestStore::new();

        let status = store
            .get_status(&"nonexistent".to_string(), "scan")
            .await
            .unwrap();
        assert!(status.is_none());

        let all = store
            .get_all_statuses(&"nonexistent".to_string())
            .await
            .unwrap();
        assert!(all.is_empty());

        let subtasks = store
            .get_subtask_statuses(&"nonexistent".to_string(), "stage")
            .await
            .unwrap();
        assert!(subtasks.is_empty());
    }

    #[tokio::test]
    async fn test_clear() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        store
            .set_status(&item_id, "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_subtask_status(&item_id, "enrich", "sub-1", SubTaskStatus::pending("sub-1"))
            .await
            .unwrap();

        assert_eq!(store.stage_count().await, 1);
        assert_eq!(store.subtask_count().await, 1);

        store.clear().await;

        assert_eq!(store.stage_count().await, 0);
        assert_eq!(store.subtask_count().await, 0);
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let store = TestStore::new();
        let store = Arc::new(store);

        let mut handles = Vec::new();

        // Spawn 10 tasks writing different items
        for i in 0..10 {
            let store = Arc::clone(&store);
            let handle = tokio::spawn(async move {
                let item_id = format!("item-{}", i);
                store
                    .set_status(&item_id, "scan", StageStatus::running())
                    .await
                    .unwrap();
                store
                    .set_status(&item_id, "scan", StageStatus::completed())
                    .await
                    .unwrap();
            });
            handles.push(handle);
        }

        // Wait for all tasks
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all items were stored
        let completed = store
            .query_items("scan", StageState::Completed)
            .await
            .unwrap();
        assert_eq!(completed.len(), 10);
    }

    #[tokio::test]
    async fn test_status_with_review_data() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        let review = ReviewData::with_details(
            "Please verify entities",
            serde_json::json!({"entities": ["Person", "Place"]}),
        );
        let status = StageStatus::awaiting_review(review);

        store.set_status(&item_id, "review", status).await.unwrap();

        let retrieved = store
            .get_status(&item_id, "review")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.state, StageState::AwaitingReview);
        assert!(retrieved.review_data.is_some());
        assert_eq!(
            retrieved.review_data.unwrap().summary,
            "Please verify entities"
        );
    }

    #[tokio::test]
    async fn test_status_with_error() {
        let store = TestStore::new();
        let item_id = "item-1".to_string();

        let status = StageStatus::failed("connection timeout after 30s");
        store.set_status(&item_id, "fetch", status).await.unwrap();

        let retrieved = store
            .get_status(&item_id, "fetch")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.state, StageState::Failed);
        assert_eq!(
            retrieved.error,
            Some("connection timeout after 30s".to_string())
        );
    }

    #[tokio::test]
    async fn test_store_is_clone() {
        let store1 = TestStore::new();
        let store2 = store1.clone();

        store1
            .set_status(&"item-1".to_string(), "scan", StageStatus::completed())
            .await
            .unwrap();

        // Changes visible through clone
        let status = store2
            .get_status(&"item-1".to_string(), "scan")
            .await
            .unwrap();
        assert!(status.is_some());
    }
}
```

**Design Decisions:**
- `Arc<RwLock<Storage>>` for thread-safe concurrent access
- `Clone` on the store shares the same underlying storage
- `PhantomData<W>` to maintain generic association without storing `W`
- Separate key types for clarity and to avoid tuple confusion
- Helper methods (`stage_count`, `subtask_count`, `clear`) for testing

#### 2.1.3 Update `src/lib.rs`

```rust
mod error;
mod stage;
mod state_store;
mod work_item;

pub use error::{Result, TreadleError};
pub use stage::{
    ReviewData, Stage, StageContext, StageOutcome, StageState, StageStatus,
    SubTask, SubTaskStatus,
};
pub use state_store::{MemoryStateStore, StateStore};
pub use work_item::WorkItem;
```

### Verification Commands
```bash
cargo build
cargo test state_store::memory::tests
cargo clippy
```

---

## Milestone 2.2 — SQLite StateStore: Schema and Connection

### Files to Create/Modify
- `src/state_store/sqlite.rs` (new)
- `src/state_store/mod.rs` (update exports)

### Implementation Details

#### 2.2.1 Create `src/state_store/sqlite.rs`

```rust
//! SQLite-backed state store implementation.
//!
//! This module provides [`SqliteStateStore`], a persistent implementation
//! of [`StateStore`] backed by SQLite.

#![cfg(feature = "sqlite")]

use async_trait::async_trait;
use rusqlite::{Connection, params};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{Result, StageState, StageStatus, SubTaskStatus, TreadleError, WorkItem};
use super::StateStore;

/// Schema version for migrations.
const SCHEMA_VERSION: i32 = 1;

/// SQL for creating the stage_statuses table.
const CREATE_STAGE_STATUSES_TABLE: &str = r#"
    CREATE TABLE IF NOT EXISTS stage_statuses (
        item_id TEXT NOT NULL,
        stage TEXT NOT NULL,
        state TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        error TEXT,
        review_data TEXT,
        PRIMARY KEY (item_id, stage)
    )
"#;

/// SQL for creating the subtask_statuses table.
const CREATE_SUBTASK_STATUSES_TABLE: &str = r#"
    CREATE TABLE IF NOT EXISTS subtask_statuses (
        item_id TEXT NOT NULL,
        stage TEXT NOT NULL,
        subtask_name TEXT NOT NULL,
        state TEXT NOT NULL,
        error TEXT,
        updated_at TEXT NOT NULL,
        PRIMARY KEY (item_id, stage, subtask_name)
    )
"#;

/// SQL for creating the schema_version table.
const CREATE_SCHEMA_VERSION_TABLE: &str = r#"
    CREATE TABLE IF NOT EXISTS schema_version (
        version INTEGER NOT NULL
    )
"#;

/// Index on stage_statuses for query_items queries.
const CREATE_STAGE_STATE_INDEX: &str = r#"
    CREATE INDEX IF NOT EXISTS idx_stage_state
    ON stage_statuses (stage, state)
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

        let conn = tokio::task::spawn_blocking(move || {
            Connection::open(&path)
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("failed to open database: {}", e)))?;

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
        let conn = tokio::task::spawn_blocking(|| {
            Connection::open_in_memory()
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("failed to open in-memory database: {}", e)))?;

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
                .query_row(
                    "SELECT version FROM schema_version LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .ok();

            if version.is_none() || version.unwrap() < SCHEMA_VERSION {
                // Run migrations
                conn.execute(CREATE_STAGE_STATUSES_TABLE, [])?;
                conn.execute(CREATE_SUBTASK_STATUSES_TABLE, [])?;
                conn.execute(CREATE_STAGE_STATE_INDEX, [])?;

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
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("migration failed: {}", e)))?;

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
                    "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('stage_statuses', 'subtask_statuses', 'schema_version')"
                )?;
                let rows = stmt.query_map([], |row| row.get(0))?;
                rows.filter_map(|r| r.ok()).collect()
            };

            Ok::<bool, rusqlite::Error>(tables.len() == 3)
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("table check failed: {}", e)))
    }
}

// Debug implementation that doesn't expose connection details
impl std::fmt::Debug for SqliteStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteStateStore").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
```

**Design Decisions:**
- `Arc<Mutex<Connection>>` for thread-safe access (rusqlite Connection is not Send)
- `spawn_blocking` for all database operations to avoid blocking async runtime
- Simple schema versioning for future migrations
- Index on `(stage, state)` for efficient `query_items` queries
- `blocking_lock()` from tokio's Mutex (designed for blocking code inside spawn_blocking)

#### 2.2.2 Update `src/state_store/mod.rs`

```rust
//! State persistence for workflow execution.

mod memory;

#[cfg(feature = "sqlite")]
mod sqlite;

pub use memory::MemoryStateStore;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStateStore;

// ... rest of the trait definition
```

### Verification Commands
```bash
cargo build --features sqlite
cargo test state_store::sqlite::tests --features sqlite
cargo clippy --features sqlite
```

---

## Milestone 2.3 — SQLite StateStore: Full Implementation

### Files to Modify
- `src/state_store/sqlite.rs` (add StateStore impl)

### Implementation Details

#### 2.3.1 Add StateStore Implementation

Add to `src/state_store/sqlite.rs`:

```rust
/// Serializes a StageState to a string for storage.
fn state_to_string(state: StageState) -> &'static str {
    match state {
        StageState::Pending => "pending",
        StageState::Running => "running",
        StageState::Completed => "completed",
        StageState::Failed => "failed",
        StageState::AwaitingReview => "awaiting_review",
        StageState::Skipped => "skipped",
    }
}

/// Parses a StageState from a stored string.
fn string_to_state(s: &str) -> StageState {
    match s {
        "pending" => StageState::Pending,
        "running" => StageState::Running,
        "completed" => StageState::Completed,
        "failed" => StageState::Failed,
        "awaiting_review" => StageState::AwaitingReview,
        "skipped" => StageState::Skipped,
        _ => StageState::Pending, // Default fallback
    }
}

/// Serializes ReviewData to JSON string.
fn review_data_to_json(data: &crate::ReviewData) -> Result<String> {
    serde_json::to_string(data)
        .map_err(|e| TreadleError::StateStoreError(format!("failed to serialize review data: {}", e)))
}

/// Parses ReviewData from JSON string.
fn json_to_review_data(json: &str) -> Result<crate::ReviewData> {
    serde_json::from_str(json)
        .map_err(|e| TreadleError::StateStoreError(format!("failed to parse review data: {}", e)))
}

#[async_trait]
impl<W: WorkItem> StateStore<W> for SqliteStateStore {
    async fn get_status(&self, item_id: &W::Id, stage: &str) -> Result<Option<StageStatus>> {
        let conn = Arc::clone(&self.conn);
        let item_id = item_id.to_string();
        let stage = stage.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            let result = conn.query_row(
                "SELECT state, updated_at, error, review_data FROM stage_statuses WHERE item_id = ?1 AND stage = ?2",
                params![item_id, stage],
                |row| {
                    let state_str: String = row.get(0)?;
                    let updated_at_str: String = row.get(1)?;
                    let error: Option<String> = row.get(2)?;
                    let review_data_json: Option<String> = row.get(3)?;
                    Ok((state_str, updated_at_str, error, review_data_json))
                },
            );

            match result {
                Ok((state_str, updated_at_str, error, review_data_json)) => {
                    let state = string_to_state(&state_str);
                    let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .unwrap_or_else(|_| chrono::Utc::now());
                    let review_data = review_data_json
                        .as_ref()
                        .map(|json| json_to_review_data(json))
                        .transpose()?;

                    Ok(Some(StageStatus {
                        state,
                        updated_at,
                        error,
                        review_data,
                        subtasks: Vec::new(), // Subtasks loaded separately
                    }))
                }
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(TreadleError::StateStoreError(format!("query failed: {}", e))),
            }
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
    }

    async fn set_status(&self, item_id: &W::Id, stage: &str, status: StageStatus) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let item_id = item_id.to_string();
        let stage = stage.to_string();
        let state = state_to_string(status.state);
        let updated_at = status.updated_at.to_rfc3339();
        let error = status.error;
        let review_data_json = status.review_data
            .as_ref()
            .map(review_data_to_json)
            .transpose()?;

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            conn.execute(
                "INSERT OR REPLACE INTO stage_statuses (item_id, stage, state, updated_at, error, review_data) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![item_id, stage, state, updated_at, error, review_data_json],
            )?;

            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("insert failed: {}", e)))?;

        Ok(())
    }

    async fn get_all_statuses(&self, item_id: &W::Id) -> Result<HashMap<String, StageStatus>> {
        let conn = Arc::clone(&self.conn);
        let item_id = item_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            let mut stmt = conn.prepare(
                "SELECT stage, state, updated_at, error, review_data FROM stage_statuses WHERE item_id = ?1"
            )?;

            let rows = stmt.query_map(params![item_id], |row| {
                let stage: String = row.get(0)?;
                let state_str: String = row.get(1)?;
                let updated_at_str: String = row.get(2)?;
                let error: Option<String> = row.get(3)?;
                let review_data_json: Option<String> = row.get(4)?;
                Ok((stage, state_str, updated_at_str, error, review_data_json))
            })?;

            let mut result = HashMap::new();
            for row in rows {
                let (stage, state_str, updated_at_str, error, review_data_json) = row?;
                let state = string_to_state(&state_str);
                let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());
                let review_data = review_data_json
                    .as_ref()
                    .map(|json| serde_json::from_str(json))
                    .transpose()
                    .ok()
                    .flatten();

                result.insert(stage, StageStatus {
                    state,
                    updated_at,
                    error,
                    review_data,
                    subtasks: Vec::new(),
                });
            }

            Ok::<HashMap<String, StageStatus>, rusqlite::Error>(result)
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("query failed: {}", e)))
    }

    async fn query_items(&self, stage: &str, state: StageState) -> Result<Vec<String>> {
        let conn = Arc::clone(&self.conn);
        let stage = stage.to_string();
        let state_str = state_to_string(state).to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            let mut stmt = conn.prepare(
                "SELECT item_id FROM stage_statuses WHERE stage = ?1 AND state = ?2"
            )?;

            let rows = stmt.query_map(params![stage, state_str], |row| {
                row.get(0)
            })?;

            let result: std::result::Result<Vec<String>, _> = rows.collect();
            result
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("query failed: {}", e)))
    }

    async fn get_subtask_statuses(
        &self,
        item_id: &W::Id,
        stage: &str,
    ) -> Result<Vec<SubTaskStatus>> {
        let conn = Arc::clone(&self.conn);
        let item_id = item_id.to_string();
        let stage = stage.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            let mut stmt = conn.prepare(
                "SELECT subtask_name, state, error, updated_at FROM subtask_statuses WHERE item_id = ?1 AND stage = ?2"
            )?;

            let rows = stmt.query_map(params![item_id, stage], |row| {
                let name: String = row.get(0)?;
                let state_str: String = row.get(1)?;
                let error: Option<String> = row.get(2)?;
                let updated_at_str: String = row.get(3)?;
                Ok((name, state_str, error, updated_at_str))
            })?;

            let mut result = Vec::new();
            for row in rows {
                let (name, state_str, error, updated_at_str) = row?;
                let state = string_to_state(&state_str);
                let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at_str)
                    .map(|dt| dt.with_timezone(&chrono::Utc))
                    .unwrap_or_else(|_| chrono::Utc::now());

                result.push(SubTaskStatus {
                    name,
                    state,
                    error,
                    updated_at,
                });
            }

            Ok::<Vec<SubTaskStatus>, rusqlite::Error>(result)
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("query failed: {}", e)))
    }

    async fn set_subtask_status(
        &self,
        item_id: &W::Id,
        stage: &str,
        subtask: &str,
        status: SubTaskStatus,
    ) -> Result<()> {
        let conn = Arc::clone(&self.conn);
        let item_id = item_id.to_string();
        let stage = stage.to_string();
        let subtask = subtask.to_string();
        let state = state_to_string(status.state);
        let updated_at = status.updated_at.to_rfc3339();
        let error = status.error;

        tokio::task::spawn_blocking(move || {
            let conn = conn.blocking_lock();

            conn.execute(
                "INSERT OR REPLACE INTO subtask_statuses (item_id, stage, subtask_name, state, error, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![item_id, stage, subtask, state, error, updated_at],
            )?;

            Ok::<(), rusqlite::Error>(())
        })
        .await
        .map_err(|e| TreadleError::StateStoreError(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| TreadleError::StateStoreError(format!("insert failed: {}", e)))?;

        Ok(())
    }
}

// Additional tests for full implementation
#[cfg(test)]
mod full_tests {
    use super::*;
    use crate::work_item::test_helpers::TestItem;
    use crate::ReviewData;

    type TestStore = SqliteStateStore;

    #[tokio::test]
    async fn test_set_and_get_status() {
        let store = TestStore::open_in_memory().await.unwrap();
        let item_id = "item-1".to_string();

        let status = store.get_status::<TestItem>(&item_id, "scan").await.unwrap();
        assert!(status.is_none());

        store
            .set_status::<TestItem>(&item_id, "scan", StageStatus::running())
            .await
            .unwrap();

        let status = store.get_status::<TestItem>(&item_id, "scan").await.unwrap();
        assert!(status.is_some());
        assert_eq!(status.unwrap().state, StageState::Running);
    }

    #[tokio::test]
    async fn test_get_all_statuses() {
        let store = TestStore::open_in_memory().await.unwrap();
        let item_id = "item-1".to_string();

        store
            .set_status::<TestItem>(&item_id, "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_status::<TestItem>(&item_id, "enrich", StageStatus::running())
            .await
            .unwrap();

        let all = store.get_all_statuses::<TestItem>(&item_id).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all.get("scan").unwrap().state, StageState::Completed);
        assert_eq!(all.get("enrich").unwrap().state, StageState::Running);
    }

    #[tokio::test]
    async fn test_query_items() {
        let store = TestStore::open_in_memory().await.unwrap();

        store
            .set_status::<TestItem>(&"item-1".to_string(), "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_status::<TestItem>(&"item-2".to_string(), "scan", StageStatus::completed())
            .await
            .unwrap();
        store
            .set_status::<TestItem>(&"item-3".to_string(), "scan", StageStatus::pending())
            .await
            .unwrap();

        let completed = store
            .query_items::<TestItem>("scan", StageState::Completed)
            .await
            .unwrap();
        assert_eq!(completed.len(), 2);

        let pending = store
            .query_items::<TestItem>("scan", StageState::Pending)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn test_subtask_status() {
        let store = TestStore::open_in_memory().await.unwrap();
        let item_id = "item-1".to_string();

        store
            .set_subtask_status::<TestItem>(
                &item_id,
                "enrich",
                "source-1",
                SubTaskStatus::pending("source-1"),
            )
            .await
            .unwrap();
        store
            .set_subtask_status::<TestItem>(
                &item_id,
                "enrich",
                "source-2",
                SubTaskStatus::completed("source-2"),
            )
            .await
            .unwrap();

        let subtasks = store
            .get_subtask_statuses::<TestItem>(&item_id, "enrich")
            .await
            .unwrap();
        assert_eq!(subtasks.len(), 2);
    }

    #[tokio::test]
    async fn test_status_with_review_data() {
        let store = TestStore::open_in_memory().await.unwrap();
        let item_id = "item-1".to_string();

        let review = ReviewData::with_details(
            "Please verify",
            serde_json::json!({"key": "value"}),
        );
        let status = StageStatus::awaiting_review(review);

        store
            .set_status::<TestItem>(&item_id, "review", status)
            .await
            .unwrap();

        let retrieved = store
            .get_status::<TestItem>(&item_id, "review")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(retrieved.state, StageState::AwaitingReview);
        assert!(retrieved.review_data.is_some());
        assert_eq!(retrieved.review_data.unwrap().summary, "Please verify");
    }

    #[tokio::test]
    async fn test_persistence_across_reopens() {
        let temp_dir = std::env::temp_dir();
        let db_path = temp_dir.join(format!("treadle_persist_test_{}.db", std::process::id()));

        let _ = std::fs::remove_file(&db_path);

        // Write data
        {
            let store = TestStore::open(&db_path).await.unwrap();
            store
                .set_status::<TestItem>(
                    &"item-1".to_string(),
                    "scan",
                    StageStatus::completed(),
                )
                .await
                .unwrap();
        }

        // Read after reopen
        {
            let store = TestStore::open(&db_path).await.unwrap();
            let status = store
                .get_status::<TestItem>(&"item-1".to_string(), "scan")
                .await
                .unwrap();

            assert!(status.is_some());
            assert_eq!(status.unwrap().state, StageState::Completed);
        }

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn test_status_with_error() {
        let store = TestStore::open_in_memory().await.unwrap();
        let item_id = "item-1".to_string();

        let status = StageStatus::failed("connection timeout");
        store
            .set_status::<TestItem>(&item_id, "fetch", status)
            .await
            .unwrap();

        let retrieved = store
            .get_status::<TestItem>(&item_id, "fetch")
            .await
            .unwrap()
            .unwrap();

        assert_eq!(retrieved.state, StageState::Failed);
        assert_eq!(retrieved.error, Some("connection timeout".to_string()));
    }
}
```

**Design Decisions:**
- RFC 3339 format for datetime storage (ISO 8601 compatible)
- JSON serialization for ReviewData (flexible schema)
- `INSERT OR REPLACE` for upsert behavior
- Subtasks not auto-loaded with stage status (loaded separately when needed)

### Verification Commands
```bash
cargo build --features sqlite
cargo test state_store::sqlite --features sqlite
cargo clippy --features sqlite
```

---

## Milestone 2.4 — Re-exports and Module Organization

### Files to Modify
- `src/lib.rs` (final exports)
- `src/state_store/mod.rs` (final organization)

### Implementation Details

#### 2.4.1 Final `src/lib.rs`

```rust
//! # Treadle
//!
//! A persistent, resumable, human-in-the-loop workflow engine backed by a
//! [petgraph](https://docs.rs/petgraph) DAG.
//!
//! Treadle fills the gap between single-shot DAG executors and heavyweight
//! distributed workflow engines. It is designed for **local, single-process
//! pipelines** where:
//!
//! - Work items progress through a DAG of stages over time
//! - Each item's state is tracked persistently (survives restarts)
//! - Stages can pause for human review and resume later
//! - Fan-out stages track each subtask independently
//! - The full pipeline is inspectable at any moment
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use treadle::{Stage, StageOutcome, StageContext, Result, WorkItem};
//!
//! // Define your work item
//! #[derive(Debug, Clone)]
//! struct Document {
//!     id: String,
//!     content: String,
//! }
//!
//! impl WorkItem for Document {
//!     type Id = String;
//!     fn id(&self) -> &Self::Id { &self.id }
//! }
//!
//! // Define a stage
//! struct ScanStage;
//!
//! #[async_trait::async_trait]
//! impl Stage<Document> for ScanStage {
//!     fn name(&self) -> &str { "scan" }
//!
//!     async fn execute(&self, doc: &Document, ctx: &StageContext) -> Result<StageOutcome> {
//!         println!("Scanning document: {}", doc.id);
//!         Ok(StageOutcome::Completed)
//!     }
//! }
//! ```
//!
//! ## Feature Flags
//!
//! - `sqlite` (default): Enables [`SqliteStateStore`] for persistent storage

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

mod error;
mod stage;
mod state_store;
mod work_item;

// Error types
pub use error::{Result, TreadleError};

// Stage types and traits
pub use stage::{
    ReviewData, Stage, StageContext, StageOutcome, StageState, StageStatus,
    SubTask, SubTaskStatus,
};

// State store trait and implementations
pub use state_store::{MemoryStateStore, StateStore};

#[cfg(feature = "sqlite")]
pub use state_store::SqliteStateStore;

// Work item trait
pub use work_item::WorkItem;

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod api_tests {
    //! Tests verifying the public API surface.

    use super::*;

    #[test]
    fn test_version() {
        assert!(!version().is_empty());
    }

    #[test]
    fn test_public_types_are_exported() {
        // This test verifies that all expected types are publicly accessible.
        // If any of these fail to compile, the export is missing.
        fn _assert_exported() {
            let _: Option<TreadleError> = None;
            let _: Result<()> = Ok(());
            let _: StageState = StageState::Pending;
            let _: StageOutcome = StageOutcome::Completed;
            let _: StageStatus = StageStatus::pending();
            let _: ReviewData = ReviewData::new("test");
            let _: SubTask = SubTask::new("test");
            let _: SubTaskStatus = SubTaskStatus::pending("test");
            let _: StageContext = StageContext::new("item", "stage");
        }
    }

    #[test]
    fn test_memory_store_exported() {
        fn _assert_exported<W: WorkItem>() {
            let _store: MemoryStateStore<W>;
        }
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn test_sqlite_store_exported() {
        fn _assert_exported() {
            // SqliteStateStore is not generic, just needs to exist
            let _: Option<SqliteStateStore> = None;
        }
    }
}
```

#### 2.4.2 Final `src/state_store/mod.rs`

```rust
//! State persistence for workflow execution.
//!
//! This module provides the [`StateStore`] trait for persisting workflow
//! execution state, along with implementations:
//!
//! - [`MemoryStateStore`]: In-memory storage for testing
//! - [`SqliteStateStore`]: SQLite-backed persistent storage (requires `sqlite` feature)
//!
//! # Example
//!
//! ```rust
//! use treadle::{MemoryStateStore, StateStore, StageStatus, WorkItem};
//!
//! #[derive(Debug, Clone)]
//! struct MyItem { id: String }
//!
//! impl WorkItem for MyItem {
//!     type Id = String;
//!     fn id(&self) -> &Self::Id { &self.id }
//! }
//!
//! # async fn example() -> treadle::Result<()> {
//! let store = MemoryStateStore::<MyItem>::new();
//!
//! // Store and retrieve status
//! store.set_status(&"item-1".to_string(), "scan", StageStatus::completed()).await?;
//! let status = store.get_status(&"item-1".to_string(), "scan").await?;
//! # Ok(())
//! # }
//! ```

mod memory;

#[cfg(feature = "sqlite")]
mod sqlite;

pub use memory::MemoryStateStore;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStateStore;

use async_trait::async_trait;
use std::collections::HashMap;

use crate::{Result, StageState, StageStatus, SubTaskStatus, WorkItem};

/// A persistent store for workflow execution state.
///
/// The `StateStore` trait abstracts over different storage backends
/// (in-memory, SQLite, etc.) for persisting the execution state of
/// work items as they progress through a workflow.
///
/// # Implementation Notes
///
/// - All methods are async to support both sync and async backends
/// - Implementations must be thread-safe (`Send + Sync`)
/// - Item IDs are serialized to strings using the `Display` trait
///
/// # Example Implementation
///
/// See [`MemoryStateStore`] for a reference implementation.
#[async_trait]
pub trait StateStore<W: WorkItem>: Send + Sync {
    /// Retrieves the status of a stage for a work item.
    ///
    /// Returns `Ok(None)` if no status has been recorded for this stage.
    async fn get_status(&self, item_id: &W::Id, stage: &str) -> Result<Option<StageStatus>>;

    /// Sets the status of a stage for a work item.
    ///
    /// This overwrites any existing status for the stage.
    async fn set_status(&self, item_id: &W::Id, stage: &str, status: StageStatus) -> Result<()>;

    /// Retrieves all stage statuses for a work item.
    ///
    /// Returns a map from stage name to status.
    async fn get_all_statuses(&self, item_id: &W::Id) -> Result<HashMap<String, StageStatus>>;

    /// Queries for work items in a specific state for a stage.
    ///
    /// Returns work item IDs as strings.
    async fn query_items(&self, stage: &str, state: StageState) -> Result<Vec<String>>;

    /// Retrieves all subtask statuses for a fan-out stage.
    async fn get_subtask_statuses(
        &self,
        item_id: &W::Id,
        stage: &str,
    ) -> Result<Vec<SubTaskStatus>>;

    /// Sets the status of a specific subtask.
    async fn set_subtask_status(
        &self,
        item_id: &W::Id,
        stage: &str,
        subtask: &str,
        status: SubTaskStatus,
    ) -> Result<()>;
}
```

### Verification Commands
```bash
# Test with all features
cargo build --all-features
cargo test --all-features
cargo clippy --all-features

# Test without sqlite feature
cargo build --no-default-features
cargo test --no-default-features

# Generate and verify documentation
cargo doc --no-deps --open
```

---

## Phase 2 Completion Checklist

- [ ] `cargo build --all-features` succeeds
- [ ] `cargo test --all-features` passes
- [ ] `cargo build --no-default-features` succeeds (no sqlite)
- [ ] `cargo test --no-default-features` passes
- [ ] `cargo clippy --all-features -- -D warnings` clean
- [ ] `cargo doc --no-deps` builds without warnings

### Public API After Phase 2

```rust
// Previously from Phase 1
treadle::TreadleError
treadle::Result<T>
treadle::WorkItem
treadle::Stage
treadle::StageContext
treadle::StageState
treadle::StageStatus
treadle::StageOutcome
treadle::SubTask
treadle::SubTaskStatus
treadle::ReviewData

// New in Phase 2
treadle::StateStore          // Trait
treadle::MemoryStateStore    // Implementation
treadle::SqliteStateStore    // Implementation (feature = "sqlite")
```

---

## Architecture After Phase 2

```
src/
├── lib.rs                  # Crate root, re-exports
├── error.rs                # TreadleError, Result
├── work_item.rs            # WorkItem trait
├── stage.rs                # Stage types and trait
└── state_store/
    ├── mod.rs              # StateStore trait, re-exports
    ├── memory.rs           # MemoryStateStore
    └── sqlite.rs           # SqliteStateStore (#[cfg(feature = "sqlite")])
```
