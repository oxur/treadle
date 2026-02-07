//! In-memory state store implementation.
//!
//! This module provides [`MemoryStateStore`], a thread-safe in-memory
//! implementation of [`StateStore`] suitable for testing and development.

use crate::{Result, StageState};
use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::StateStore;

/// Internal storage for the memory state store.
#[derive(Debug, Default)]
struct Storage {
    /// Stage states indexed by (work_item_id, stage_name).
    stage_states: HashMap<String, HashMap<String, StageState>>,
    /// Work item data indexed by work_item_id.
    work_items: HashMap<String, JsonValue>,
}

/// An in-memory implementation of [`StateStore`].
///
/// This implementation uses `Arc<RwLock<...>>` internally, making it
/// safe to clone and share across async tasks. Multiple readers can access
/// the store concurrently, but writers get exclusive access.
///
/// # Example
///
/// ```
/// use treadle::{MemoryStateStore, StateStore, StageState};
///
/// # async fn example() -> treadle::Result<()> {
/// let mut store = MemoryStateStore::new();
///
/// // Store stage state
/// let mut state = StageState::new();
/// state.mark_in_progress();
/// store.save_stage_state("item-1", "scan", &state).await?;
///
/// // Retrieve stage state
/// let retrieved = store.get_stage_state("item-1", "scan").await?;
/// assert!(retrieved.is_some());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct MemoryStateStore {
    storage: Arc<RwLock<Storage>>,
}

impl MemoryStateStore {
    /// Creates a new, empty in-memory state store.
    pub fn new() -> Self {
        Self {
            storage: Arc::new(RwLock::new(Storage::default())),
        }
    }

    /// Returns the number of work items currently stored.
    ///
    /// Useful for testing.
    pub async fn item_count(&self) -> usize {
        self.storage.read().await.work_items.len()
    }

    /// Returns the total number of stage states currently stored.
    ///
    /// Useful for testing.
    pub async fn stage_count(&self) -> usize {
        self.storage
            .read()
            .await
            .stage_states
            .values()
            .map(|stages| stages.len())
            .sum()
    }

    /// Clears all stored data.
    ///
    /// Useful for resetting state between tests.
    pub async fn clear(&self) {
        let mut storage = self.storage.write().await;
        storage.stage_states.clear();
        storage.work_items.clear();
    }
}

impl Default for MemoryStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl StateStore for MemoryStateStore {
    async fn save_stage_state(
        &mut self,
        work_item_id: &str,
        stage_name: &str,
        state: &StageState,
    ) -> Result<()> {
        let mut storage = self.storage.write().await;
        storage
            .stage_states
            .entry(work_item_id.to_string())
            .or_default()
            .insert(stage_name.to_string(), state.clone());
        Ok(())
    }

    async fn get_stage_state(
        &self,
        work_item_id: &str,
        stage_name: &str,
    ) -> Result<Option<StageState>> {
        let storage = self.storage.read().await;
        Ok(storage
            .stage_states
            .get(work_item_id)
            .and_then(|stages| stages.get(stage_name))
            .cloned())
    }

    async fn get_all_stage_states(
        &self,
        work_item_id: &str,
    ) -> Result<HashMap<String, StageState>> {
        let storage = self.storage.read().await;
        Ok(storage
            .stage_states
            .get(work_item_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn save_work_item_data(&mut self, work_item_id: &str, data: &JsonValue) -> Result<()> {
        let mut storage = self.storage.write().await;
        storage
            .work_items
            .insert(work_item_id.to_string(), data.clone());
        Ok(())
    }

    async fn get_work_item_data(&self, work_item_id: &str) -> Result<Option<JsonValue>> {
        let storage = self.storage.read().await;
        Ok(storage.work_items.get(work_item_id).cloned())
    }

    async fn delete_work_item(&mut self, work_item_id: &str) -> Result<()> {
        let mut storage = self.storage.write().await;
        storage.stage_states.remove(work_item_id);
        storage.work_items.remove(work_item_id);
        Ok(())
    }

    async fn list_work_items(&self) -> Result<Vec<String>> {
        let storage = self.storage.read().await;
        // Collect unique work item IDs from both stage_states and work_items
        let mut ids: Vec<String> = storage
            .stage_states
            .keys()
            .chain(storage.work_items.keys())
            .cloned()
            .collect();
        ids.sort();
        ids.dedup();
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StageStatus;

    #[tokio::test]
    async fn test_new_store_is_empty() {
        let store = MemoryStateStore::new();
        assert_eq!(store.item_count().await, 0);
        assert_eq!(store.stage_count().await, 0);
    }

    #[tokio::test]
    async fn test_save_and_get_stage_state() {
        let mut store = MemoryStateStore::new();
        let mut state = StageState::new();
        state.mark_in_progress();

        store
            .save_stage_state("item-1", "stage-1", &state)
            .await
            .unwrap();

        let retrieved = store.get_stage_state("item-1", "stage-1").await.unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.status, StageStatus::InProgress);
    }

    #[tokio::test]
    async fn test_get_nonexistent_stage() {
        let store = MemoryStateStore::new();
        let result = store.get_stage_state("missing", "stage").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_get_all_stage_states() {
        let mut store = MemoryStateStore::new();
        let mut state1 = StageState::new();
        state1.mark_complete();
        let mut state2 = StageState::new();
        state2.mark_in_progress();

        store
            .save_stage_state("item-1", "stage-1", &state1)
            .await
            .unwrap();
        store
            .save_stage_state("item-1", "stage-2", &state2)
            .await
            .unwrap();

        let all_states = store.get_all_stage_states("item-1").await.unwrap();
        assert_eq!(all_states.len(), 2);
        assert_eq!(
            all_states.get("stage-1").unwrap().status,
            StageStatus::Complete
        );
        assert_eq!(
            all_states.get("stage-2").unwrap().status,
            StageStatus::InProgress
        );
    }

    #[tokio::test]
    async fn test_get_all_stage_states_empty() {
        let store = MemoryStateStore::new();
        let all_states = store.get_all_stage_states("missing").await.unwrap();
        assert!(all_states.is_empty());
    }

    #[tokio::test]
    async fn test_save_and_get_work_item_data() {
        let mut store = MemoryStateStore::new();
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
    async fn test_get_nonexistent_work_item() {
        let store = MemoryStateStore::new();
        let result = store.get_work_item_data("missing").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete_work_item() {
        let mut store = MemoryStateStore::new();
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
        let mut store = MemoryStateStore::new();
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
    async fn test_list_empty() {
        let store = MemoryStateStore::new();
        let items = store.list_work_items().await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_update_stage_state() {
        let mut store = MemoryStateStore::new();
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

    #[tokio::test]
    async fn test_store_trait_object() {
        let mut store: Box<dyn StateStore> = Box::new(MemoryStateStore::new());
        let state = StageState::new();

        store
            .save_stage_state("item-1", "stage-1", &state)
            .await
            .unwrap();

        let retrieved = store.get_stage_state("item-1", "stage-1").await.unwrap();
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_multiple_work_items() {
        let mut store = MemoryStateStore::new();
        let state1 = StageState::new();
        let state2 = StageState::new();

        store
            .save_stage_state("item-1", "stage-1", &state1)
            .await
            .unwrap();
        store
            .save_stage_state("item-2", "stage-1", &state2)
            .await
            .unwrap();

        let retrieved1 = store.get_stage_state("item-1", "stage-1").await.unwrap();
        let retrieved2 = store.get_stage_state("item-2", "stage-1").await.unwrap();

        assert!(retrieved1.is_some());
        assert!(retrieved2.is_some());
    }

    #[tokio::test]
    async fn test_clear() {
        let mut store = MemoryStateStore::new();
        let state = StageState::new();
        let data = serde_json::json!({"id": "item-1"});

        store
            .save_stage_state("item-1", "stage-1", &state)
            .await
            .unwrap();
        store.save_work_item_data("item-1", &data).await.unwrap();

        assert_eq!(store.stage_count().await, 1);
        assert_eq!(store.item_count().await, 1);

        store.clear().await;

        assert_eq!(store.stage_count().await, 0);
        assert_eq!(store.item_count().await, 0);
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let store = MemoryStateStore::new();
        let store = Arc::new(store);

        let mut handles = Vec::new();

        // Spawn 10 tasks writing different items
        for i in 0..10 {
            let mut store_clone = Arc::clone(&store).as_ref().clone();
            let handle = tokio::spawn(async move {
                let item_id = format!("item-{}", i);
                let mut state = StageState::new();
                state.mark_in_progress();
                store_clone
                    .save_stage_state(&item_id, "scan", &state)
                    .await
                    .unwrap();
                state.mark_complete();
                store_clone
                    .save_stage_state(&item_id, "scan", &state)
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
        let items = store.list_work_items().await.unwrap();
        assert_eq!(items.len(), 10);
    }

    #[tokio::test]
    async fn test_store_is_clone() {
        let mut store1 = MemoryStateStore::new();
        let store2 = store1.clone();

        let state = StageState::new();
        store1
            .save_stage_state("item-1", "scan", &state)
            .await
            .unwrap();

        // Changes visible through clone
        let retrieved = store2.get_stage_state("item-1", "scan").await.unwrap();
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_list_includes_items_with_only_stages() {
        let mut store = MemoryStateStore::new();
        let state = StageState::new();

        // Item with only stage state, no work item data
        store
            .save_stage_state("item-1", "stage-1", &state)
            .await
            .unwrap();

        let items = store.list_work_items().await.unwrap();
        assert_eq!(items.len(), 1);
        assert!(items.contains(&"item-1".to_string()));
    }
}
