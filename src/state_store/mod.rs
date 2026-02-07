//! State storage for the Treadle workflow engine.
//!
//! This module provides the [`StateStore`] trait for persisting and retrieving
//! workflow state, along with concrete implementations:
//!
//! - [`MemoryStateStore`]: Thread-safe in-memory storage for testing/development
//!
//! # Example
//!
//! ```
//! use treadle::{MemoryStateStore, StateStore, StageState};
//!
//! # async fn example() -> treadle::Result<()> {
//! let mut store = MemoryStateStore::new();
//!
//! // Save stage state
//! let mut state = StageState::new();
//! state.mark_in_progress();
//! store.save_stage_state("item-1", "scan", &state).await?;
//!
//! // Retrieve stage state
//! let retrieved = store.get_stage_state("item-1", "scan").await?;
//! assert!(retrieved.is_some());
//! # Ok(())
//! # }
//! ```

mod memory;

#[cfg(feature = "sqlite")]
mod sqlite;

pub use memory::MemoryStateStore;

#[cfg(feature = "sqlite")]
pub use sqlite::SqliteStateStore;

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
