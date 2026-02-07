//! State storage trait for the Treadle workflow engine.
//!
//! This module defines the `StateStore` trait for persisting and retrieving
//! workflow state, including work item state and stage progress.

use crate::{Result, StageState};
use async_trait::async_trait;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// A trait for persisting and retrieving workflow state.
///
/// The state store is responsible for maintaining the persistent state of
/// work items as they progress through the workflow. This includes tracking
/// which stages have completed, retry counts, and any error information.
///
/// # Object Safety
///
/// This trait is object-safe, allowing for dynamic dispatch with
/// `dyn StateStore`. This enables different storage backends (in-memory,
/// SQLite, etc.) to be swapped at runtime.
///
/// # Examples
///
/// ```
/// use treadle::{StateStore, StageState};
/// use async_trait::async_trait;
/// use std::collections::HashMap;
///
/// struct MyStateStore {
///     data: HashMap<String, HashMap<String, StageState>>,
/// }
///
/// #[async_trait]
/// impl StateStore for MyStateStore {
///     async fn save_stage_state(
///         &mut self,
///         work_item_id: &str,
///         stage_name: &str,
///         state: &StageState,
///     ) -> treadle::Result<()> {
///         self.data
///             .entry(work_item_id.to_string())
///             .or_default()
///             .insert(stage_name.to_string(), state.clone());
///         Ok(())
///     }
///
///     async fn get_stage_state(
///         &self,
///         work_item_id: &str,
///         stage_name: &str,
///     ) -> treadle::Result<Option<StageState>> {
///         Ok(self.data
///             .get(work_item_id)
///             .and_then(|stages| stages.get(stage_name))
///             .cloned())
///     }
///
///     async fn get_all_stage_states(
///         &self,
///         work_item_id: &str,
///     ) -> treadle::Result<HashMap<String, StageState>> {
///         Ok(self.data
///             .get(work_item_id)
///             .cloned()
///             .unwrap_or_default())
///     }
///
///     async fn save_work_item_data(
///         &mut self,
///         work_item_id: &str,
///         data: &serde_json::Value,
///     ) -> treadle::Result<()> {
///         // Implementation here
///         Ok(())
///     }
///
///     async fn get_work_item_data(
///         &self,
///         work_item_id: &str,
///     ) -> treadle::Result<Option<serde_json::Value>> {
///         // Implementation here
///         Ok(None)
///     }
///
///     async fn delete_work_item(&mut self, work_item_id: &str) -> treadle::Result<()> {
///         self.data.remove(work_item_id);
///         Ok(())
///     }
///
///     async fn list_work_items(&self) -> treadle::Result<Vec<String>> {
///         Ok(self.data.keys().cloned().collect())
///     }
/// }
/// ```
#[async_trait]
pub trait StateStore: Send + Sync {
    /// Saves the state of a stage for a specific work item.
    ///
    /// # Parameters
    ///
    /// - `work_item_id`: Unique identifier for the work item
    /// - `stage_name`: Name of the stage
    /// - `state`: The stage state to persist
    ///
    /// # Errors
    ///
    /// Returns an error if the state cannot be saved.
    async fn save_stage_state(
        &mut self,
        work_item_id: &str,
        stage_name: &str,
        state: &StageState,
    ) -> Result<()>;

    /// Retrieves the state of a stage for a specific work item.
    ///
    /// # Parameters
    ///
    /// - `work_item_id`: Unique identifier for the work item
    /// - `stage_name`: Name of the stage
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(state))` if the state exists, `Ok(None)` if not found,
    /// or an error if retrieval fails.
    async fn get_stage_state(
        &self,
        work_item_id: &str,
        stage_name: &str,
    ) -> Result<Option<StageState>>;

    /// Retrieves all stage states for a specific work item.
    ///
    /// # Parameters
    ///
    /// - `work_item_id`: Unique identifier for the work item
    ///
    /// # Returns
    ///
    /// Returns a map of stage names to their states, or an error if
    /// retrieval fails.
    async fn get_all_stage_states(&self, work_item_id: &str)
        -> Result<HashMap<String, StageState>>;

    /// Saves the serialized data for a work item.
    ///
    /// This stores the actual work item payload as JSON, separate from
    /// the stage state tracking.
    ///
    /// # Parameters
    ///
    /// - `work_item_id`: Unique identifier for the work item
    /// - `data`: JSON serialized work item data
    ///
    /// # Errors
    ///
    /// Returns an error if the data cannot be saved.
    async fn save_work_item_data(&mut self, work_item_id: &str, data: &JsonValue) -> Result<()>;

    /// Retrieves the serialized data for a work item.
    ///
    /// # Parameters
    ///
    /// - `work_item_id`: Unique identifier for the work item
    ///
    /// # Returns
    ///
    /// Returns `Ok(Some(data))` if found, `Ok(None)` if not found,
    /// or an error if retrieval fails.
    async fn get_work_item_data(&self, work_item_id: &str) -> Result<Option<JsonValue>>;

    /// Deletes all state and data for a work item.
    ///
    /// This removes the work item from the state store completely, including
    /// all stage states and the work item data.
    ///
    /// # Parameters
    ///
    /// - `work_item_id`: Unique identifier for the work item
    ///
    /// # Errors
    ///
    /// Returns an error if deletion fails.
    async fn delete_work_item(&mut self, work_item_id: &str) -> Result<()>;

    /// Lists all work item IDs currently stored.
    ///
    /// # Returns
    ///
    /// Returns a vector of work item IDs, or an error if listing fails.
    async fn list_work_items(&self) -> Result<Vec<String>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StageStatus;
    use std::sync::{Arc, Mutex};

    // Test implementation of StateStore using in-memory storage
    #[derive(Debug, Clone)]
    struct TestStateStore {
        stage_states: Arc<Mutex<HashMap<String, HashMap<String, StageState>>>>,
        work_items: Arc<Mutex<HashMap<String, JsonValue>>>,
    }

    impl TestStateStore {
        fn new() -> Self {
            Self {
                stage_states: Arc::new(Mutex::new(HashMap::new())),
                work_items: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl StateStore for TestStateStore {
        async fn save_stage_state(
            &mut self,
            work_item_id: &str,
            stage_name: &str,
            state: &StageState,
        ) -> Result<()> {
            let mut states = self.stage_states.lock().unwrap();
            states
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
            let states = self.stage_states.lock().unwrap();
            Ok(states
                .get(work_item_id)
                .and_then(|stages| stages.get(stage_name))
                .cloned())
        }

        async fn get_all_stage_states(
            &self,
            work_item_id: &str,
        ) -> Result<HashMap<String, StageState>> {
            let states = self.stage_states.lock().unwrap();
            Ok(states.get(work_item_id).cloned().unwrap_or_default())
        }

        async fn save_work_item_data(
            &mut self,
            work_item_id: &str,
            data: &JsonValue,
        ) -> Result<()> {
            let mut items = self.work_items.lock().unwrap();
            items.insert(work_item_id.to_string(), data.clone());
            Ok(())
        }

        async fn get_work_item_data(&self, work_item_id: &str) -> Result<Option<JsonValue>> {
            let items = self.work_items.lock().unwrap();
            Ok(items.get(work_item_id).cloned())
        }

        async fn delete_work_item(&mut self, work_item_id: &str) -> Result<()> {
            let mut states = self.stage_states.lock().unwrap();
            let mut items = self.work_items.lock().unwrap();
            states.remove(work_item_id);
            items.remove(work_item_id);
            Ok(())
        }

        async fn list_work_items(&self) -> Result<Vec<String>> {
            let items = self.work_items.lock().unwrap();
            Ok(items.keys().cloned().collect())
        }
    }

    #[tokio::test]
    async fn test_state_store_save_and_get_stage_state() {
        let mut store = TestStateStore::new();
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
    async fn test_state_store_get_nonexistent_stage() {
        let store = TestStateStore::new();
        let result = store.get_stage_state("missing", "stage").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_state_store_get_all_stage_states() {
        let mut store = TestStateStore::new();
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
    async fn test_state_store_get_all_stage_states_empty() {
        let store = TestStateStore::new();
        let all_states = store.get_all_stage_states("missing").await.unwrap();
        assert!(all_states.is_empty());
    }

    #[tokio::test]
    async fn test_state_store_save_and_get_work_item_data() {
        let mut store = TestStateStore::new();
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
    async fn test_state_store_get_nonexistent_work_item() {
        let store = TestStateStore::new();
        let result = store.get_work_item_data("missing").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_state_store_delete_work_item() {
        let mut store = TestStateStore::new();
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
    async fn test_state_store_list_work_items() {
        let mut store = TestStateStore::new();
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
    async fn test_state_store_list_empty() {
        let store = TestStateStore::new();
        let items = store.list_work_items().await.unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn test_state_store_update_stage_state() {
        let mut store = TestStateStore::new();
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
    async fn test_state_store_trait_object() {
        let mut store: Box<dyn StateStore> = Box::new(TestStateStore::new());
        let state = StageState::new();

        store
            .save_stage_state("item-1", "stage-1", &state)
            .await
            .unwrap();

        let retrieved = store.get_stage_state("item-1", "stage-1").await.unwrap();
        assert!(retrieved.is_some());
    }

    #[tokio::test]
    async fn test_state_store_multiple_work_items() {
        let mut store = TestStateStore::new();
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
}
