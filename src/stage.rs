//! Stage types for the Treadle workflow engine.
//!
//! This module defines the types used to represent stages in the workflow,
//! including stage state, status, outcomes, and subtask tracking.

use crate::{Result, WorkItem};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Debug;

/// The status of a work item within a stage.
///
/// Represents the current execution state of a work item as it progresses
/// through a stage in the workflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StageStatus {
    /// Work item is queued but not yet started.
    Pending,

    /// Stage is currently executing.
    InProgress,

    /// Stage completed successfully.
    Complete,

    /// Stage execution failed.
    Failed,

    /// Stage is paused awaiting human review.
    Paused,
}

/// State tracking for a work item within a stage.
///
/// Maintains the current status, timestamps, retry information, and any
/// error messages for a work item's execution within a specific stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageState {
    /// Current status of the work item in this stage.
    pub status: StageStatus,

    /// When this stage was first entered.
    pub started_at: Option<DateTime<Utc>>,

    /// When this stage completed (success or failure).
    pub completed_at: Option<DateTime<Utc>>,

    /// Number of retry attempts for this stage.
    pub retry_count: u32,

    /// Error message if the stage failed.
    pub error: Option<String>,

    /// Subtasks for fan-out stages.
    pub subtasks: Vec<SubTask>,
}

impl StageState {
    /// Creates a new stage state in pending status.
    pub fn new() -> Self {
        Self {
            status: StageStatus::Pending,
            started_at: None,
            completed_at: None,
            retry_count: 0,
            error: None,
            subtasks: Vec::new(),
        }
    }

    /// Marks the stage as in progress, recording the start time.
    pub fn mark_in_progress(&mut self) {
        self.status = StageStatus::InProgress;
        if self.started_at.is_none() {
            self.started_at = Some(Utc::now());
        }
    }

    /// Marks the stage as complete, recording the completion time.
    pub fn mark_complete(&mut self) {
        self.status = StageStatus::Complete;
        self.completed_at = Some(Utc::now());
    }

    /// Marks the stage as failed with an error message.
    pub fn mark_failed(&mut self, error: String) {
        self.status = StageStatus::Failed;
        self.completed_at = Some(Utc::now());
        self.error = Some(error);
    }

    /// Marks the stage as paused for human review.
    pub fn mark_paused(&mut self) {
        self.status = StageStatus::Paused;
    }

    /// Increments the retry count for this stage.
    pub fn increment_retry(&mut self) {
        self.retry_count += 1;
    }
}

impl Default for StageState {
    fn default() -> Self {
        Self::new()
    }
}

/// A subtask within a fan-out stage.
///
/// Fan-out stages can spawn multiple subtasks that are tracked independently.
/// Each subtask has its own status and retry count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubTask {
    /// Unique identifier for this subtask.
    pub id: String,

    /// Current status of the subtask.
    pub status: StageStatus,

    /// Number of retry attempts for this subtask.
    pub retry_count: u32,

    /// Error message if the subtask failed.
    pub error: Option<String>,

    /// Optional metadata specific to this subtask.
    pub metadata: HashMap<String, String>,
}

impl SubTask {
    /// Creates a new subtask with the given ID.
    pub fn new(id: String) -> Self {
        Self {
            id,
            status: StageStatus::Pending,
            retry_count: 0,
            error: None,
            metadata: HashMap::new(),
        }
    }

    /// Marks the subtask as complete.
    pub fn mark_complete(&mut self) {
        self.status = StageStatus::Complete;
    }

    /// Marks the subtask as failed with an error message.
    pub fn mark_failed(&mut self, error: String) {
        self.status = StageStatus::Failed;
        self.error = Some(error);
    }

    /// Increments the retry count for this subtask.
    pub fn increment_retry(&mut self) {
        self.retry_count += 1;
    }
}

/// Data structure for human review.
///
/// When a stage pauses for human review, this structure contains the
/// information needed for the reviewer to make a decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewData {
    /// Work item ID being reviewed.
    pub work_item_id: String,

    /// Stage name where review is needed.
    pub stage_name: String,

    /// Human-readable prompt for the reviewer.
    pub prompt: String,

    /// Optional context data to help with the review decision.
    pub context: HashMap<String, serde_json::Value>,

    /// When this review was requested.
    pub requested_at: DateTime<Utc>,

    /// When this review was completed (if applicable).
    pub completed_at: Option<DateTime<Utc>>,

    /// The reviewer's decision (approve/reject/etc).
    pub decision: Option<String>,

    /// Optional comments from the reviewer.
    pub comments: Option<String>,
}

impl ReviewData {
    /// Creates a new review request.
    pub fn new(work_item_id: String, stage_name: String, prompt: String) -> Self {
        Self {
            work_item_id,
            stage_name,
            prompt,
            context: HashMap::new(),
            requested_at: Utc::now(),
            completed_at: None,
            decision: None,
            comments: None,
        }
    }

    /// Adds context data for the review.
    pub fn with_context(mut self, key: String, value: serde_json::Value) -> Self {
        self.context.insert(key, value);
        self
    }

    /// Marks the review as complete with a decision.
    pub fn complete(&mut self, decision: String, comments: Option<String>) {
        self.decision = Some(decision);
        self.comments = comments;
        self.completed_at = Some(Utc::now());
    }
}

/// The outcome of executing a stage.
///
/// Returned by stage execution to indicate what should happen next
/// in the workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    /// Stage completed successfully, proceed to next stage.
    Complete,

    /// Stage requires human review before proceeding.
    NeedsReview,

    /// Stage failed and should be retried.
    Retry,

    /// Stage failed permanently, do not retry.
    Failed,

    /// Stage fans out into multiple parallel subtasks.
    ///
    /// Each subtask will be executed independently, and the stage
    /// will only complete when all subtasks succeed.
    FanOut(Vec<SubTask>),
}

/// Context provided to stages during execution.
///
/// Contains information about the current work item, stage state, and
/// any metadata needed for stage execution.
#[derive(Debug, Clone)]
pub struct StageContext {
    /// Name of the current stage.
    pub stage_name: String,

    /// Current state of this stage for the work item.
    pub stage_state: StageState,

    /// Optional metadata that can be used by stages.
    pub metadata: HashMap<String, serde_json::Value>,

    /// Optional subtask name for fan-out stages.
    ///
    /// When a stage fans out into multiple subtasks, each subtask execution
    /// receives a context with this field set to identify which subtask
    /// is being executed.
    pub subtask_name: Option<String>,
}

impl StageContext {
    /// Creates a new stage context.
    pub fn new(stage_name: String) -> Self {
        Self {
            stage_name,
            stage_state: StageState::new(),
            metadata: HashMap::new(),
            subtask_name: None,
        }
    }

    /// Adds metadata to the context.
    pub fn with_metadata(mut self, key: String, value: serde_json::Value) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Sets the subtask name for fan-out execution.
    pub fn with_subtask(mut self, subtask_name: impl Into<String>) -> Self {
        self.subtask_name = Some(subtask_name.into());
        self
    }

    /// Gets metadata by key.
    pub fn get_metadata(&self, key: &str) -> Option<&serde_json::Value> {
        self.metadata.get(key)
    }
}

/// A trait representing a stage in the workflow.
///
/// Stages are the building blocks of a workflow. Each stage processes a work
/// item and returns an outcome indicating what should happen next.
///
/// # Object Safety
///
/// This trait is object-safe, allowing for dynamic dispatch with `dyn Stage`.
///
/// # Examples
///
/// ```
/// use treadle::{Stage, StageContext, StageOutcome, WorkItem, Result};
/// use async_trait::async_trait;
///
/// #[derive(Debug)]
/// struct ScanStage;
///
/// #[async_trait]
/// impl Stage for ScanStage {
///     async fn execute(
///         &self,
///         _item: &dyn WorkItem,
///         _context: &mut StageContext,
///     ) -> Result<StageOutcome> {
///         // Perform scanning logic here
///         Ok(StageOutcome::Complete)
///     }
///
///     fn name(&self) -> &str {
///         "scan"
///     }
/// }
/// ```
#[async_trait]
pub trait Stage: Debug + Send + Sync {
    /// Executes the stage for the given work item.
    ///
    /// # Parameters
    ///
    /// - `item`: The work item being processed
    /// - `context`: Mutable context containing stage state and metadata
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing the `StageOutcome` indicating what
    /// should happen next in the workflow.
    async fn execute(
        &self,
        item: &dyn WorkItem,
        context: &mut StageContext,
    ) -> Result<StageOutcome>;

    /// Returns the name of this stage.
    ///
    /// Stage names should be unique within a workflow and are used for
    /// identification and state tracking.
    fn name(&self) -> &str;

    /// Optional hook called before stage execution.
    ///
    /// Can be used for setup, validation, or logging. Default implementation
    /// does nothing.
    async fn before_execute(&self, _item: &dyn WorkItem, _context: &StageContext) -> Result<()> {
        Ok(())
    }

    /// Optional hook called after stage execution.
    ///
    /// Can be used for cleanup, logging, or metrics. Default implementation
    /// does nothing.
    async fn after_execute(
        &self,
        _item: &dyn WorkItem,
        _context: &StageContext,
        _outcome: &StageOutcome,
    ) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_status_equality() {
        assert_eq!(StageStatus::Pending, StageStatus::Pending);
        assert_ne!(StageStatus::Pending, StageStatus::InProgress);
    }

    #[test]
    fn test_stage_status_serialize() {
        let status = StageStatus::InProgress;
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("InProgress"));
    }

    #[test]
    fn test_stage_status_deserialize() {
        let json = r#""Complete""#;
        let status: StageStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status, StageStatus::Complete);
    }

    #[test]
    fn test_stage_state_new() {
        let state = StageState::new();
        assert_eq!(state.status, StageStatus::Pending);
        assert_eq!(state.retry_count, 0);
        assert!(state.started_at.is_none());
        assert!(state.completed_at.is_none());
        assert!(state.error.is_none());
        assert!(state.subtasks.is_empty());
    }

    #[test]
    fn test_stage_state_default() {
        let state = StageState::default();
        assert_eq!(state.status, StageStatus::Pending);
    }

    #[test]
    fn test_stage_state_mark_in_progress() {
        let mut state = StageState::new();
        state.mark_in_progress();
        assert_eq!(state.status, StageStatus::InProgress);
        assert!(state.started_at.is_some());
    }

    #[test]
    fn test_stage_state_mark_complete() {
        let mut state = StageState::new();
        state.mark_complete();
        assert_eq!(state.status, StageStatus::Complete);
        assert!(state.completed_at.is_some());
    }

    #[test]
    fn test_stage_state_mark_failed() {
        let mut state = StageState::new();
        state.mark_failed("test error".to_string());
        assert_eq!(state.status, StageStatus::Failed);
        assert_eq!(state.error, Some("test error".to_string()));
        assert!(state.completed_at.is_some());
    }

    #[test]
    fn test_stage_state_mark_paused() {
        let mut state = StageState::new();
        state.mark_paused();
        assert_eq!(state.status, StageStatus::Paused);
    }

    #[test]
    fn test_stage_state_increment_retry() {
        let mut state = StageState::new();
        assert_eq!(state.retry_count, 0);
        state.increment_retry();
        assert_eq!(state.retry_count, 1);
        state.increment_retry();
        assert_eq!(state.retry_count, 2);
    }

    #[test]
    fn test_stage_state_serialize() {
        let state = StageState::new();
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("Pending"));
    }

    #[test]
    fn test_stage_state_deserialize() {
        let json = r#"{"status":"Complete","started_at":null,"completed_at":null,"retry_count":0,"error":null,"subtasks":[]}"#;
        let state: StageState = serde_json::from_str(json).unwrap();
        assert_eq!(state.status, StageStatus::Complete);
    }

    #[test]
    fn test_subtask_new() {
        let subtask = SubTask::new("sub-1".to_string());
        assert_eq!(subtask.id, "sub-1");
        assert_eq!(subtask.status, StageStatus::Pending);
        assert_eq!(subtask.retry_count, 0);
        assert!(subtask.error.is_none());
        assert!(subtask.metadata.is_empty());
    }

    #[test]
    fn test_subtask_mark_complete() {
        let mut subtask = SubTask::new("sub-2".to_string());
        subtask.mark_complete();
        assert_eq!(subtask.status, StageStatus::Complete);
    }

    #[test]
    fn test_subtask_mark_failed() {
        let mut subtask = SubTask::new("sub-3".to_string());
        subtask.mark_failed("subtask error".to_string());
        assert_eq!(subtask.status, StageStatus::Failed);
        assert_eq!(subtask.error, Some("subtask error".to_string()));
    }

    #[test]
    fn test_subtask_increment_retry() {
        let mut subtask = SubTask::new("sub-4".to_string());
        assert_eq!(subtask.retry_count, 0);
        subtask.increment_retry();
        assert_eq!(subtask.retry_count, 1);
    }

    #[test]
    fn test_subtask_metadata() {
        let mut subtask = SubTask::new("sub-5".to_string());
        subtask
            .metadata
            .insert("key".to_string(), "value".to_string());
        assert_eq!(subtask.metadata.get("key"), Some(&"value".to_string()));
    }

    #[test]
    fn test_subtask_serialize() {
        let subtask = SubTask::new("sub-6".to_string());
        let json = serde_json::to_string(&subtask).unwrap();
        assert!(json.contains("sub-6"));
    }

    #[test]
    fn test_review_data_new() {
        let review = ReviewData::new(
            "item-1".to_string(),
            "review-stage".to_string(),
            "Please review".to_string(),
        );
        assert_eq!(review.work_item_id, "item-1");
        assert_eq!(review.stage_name, "review-stage");
        assert_eq!(review.prompt, "Please review");
        assert!(review.context.is_empty());
        assert!(review.decision.is_none());
        assert!(review.comments.is_none());
    }

    #[test]
    fn test_review_data_with_context() {
        let review = ReviewData::new(
            "item-2".to_string(),
            "review-stage".to_string(),
            "Review this".to_string(),
        )
        .with_context("key".to_string(), serde_json::json!("value"));

        assert_eq!(review.context.get("key"), Some(&serde_json::json!("value")));
    }

    #[test]
    fn test_review_data_complete() {
        let mut review = ReviewData::new(
            "item-3".to_string(),
            "review-stage".to_string(),
            "Review".to_string(),
        );
        review.complete("approved".to_string(), Some("looks good".to_string()));
        assert_eq!(review.decision, Some("approved".to_string()));
        assert_eq!(review.comments, Some("looks good".to_string()));
        assert!(review.completed_at.is_some());
    }

    #[test]
    fn test_review_data_complete_without_comments() {
        let mut review = ReviewData::new(
            "item-4".to_string(),
            "review-stage".to_string(),
            "Review".to_string(),
        );
        review.complete("rejected".to_string(), None);
        assert_eq!(review.decision, Some("rejected".to_string()));
        assert!(review.comments.is_none());
        assert!(review.completed_at.is_some());
    }

    #[test]
    fn test_review_data_serialize() {
        let review = ReviewData::new(
            "item-5".to_string(),
            "stage".to_string(),
            "prompt".to_string(),
        );
        let json = serde_json::to_string(&review).unwrap();
        assert!(json.contains("item-5"));
    }

    #[test]
    fn test_stage_outcome_equality() {
        assert_eq!(StageOutcome::Complete, StageOutcome::Complete);
        assert_ne!(StageOutcome::Complete, StageOutcome::Failed);
    }

    #[test]
    fn test_stage_outcome_variants() {
        let outcomes = [
            StageOutcome::Complete,
            StageOutcome::NeedsReview,
            StageOutcome::Retry,
            StageOutcome::Failed,
        ];
        assert_eq!(outcomes.len(), 4);
    }

    #[test]
    fn test_stage_state_preserves_started_at() {
        let mut state = StageState::new();
        state.mark_in_progress();
        let first_start = state.started_at;

        // Calling mark_in_progress again should not change started_at
        state.mark_in_progress();
        assert_eq!(state.started_at, first_start);
    }

    #[test]
    fn test_stage_state_with_subtasks() {
        let mut state = StageState::new();
        let subtask1 = SubTask::new("sub-1".to_string());
        let subtask2 = SubTask::new("sub-2".to_string());
        state.subtasks.push(subtask1);
        state.subtasks.push(subtask2);
        assert_eq!(state.subtasks.len(), 2);
    }

    #[test]
    fn test_review_data_multiple_contexts() {
        let review = ReviewData::new(
            "item-6".to_string(),
            "stage".to_string(),
            "prompt".to_string(),
        )
        .with_context("key1".to_string(), serde_json::json!("value1"))
        .with_context("key2".to_string(), serde_json::json!(42));

        assert_eq!(review.context.len(), 2);
        assert_eq!(
            review.context.get("key1"),
            Some(&serde_json::json!("value1"))
        );
        assert_eq!(review.context.get("key2"), Some(&serde_json::json!(42)));
    }

    #[test]
    fn test_stage_context_new() {
        let context = StageContext::new("test-stage".to_string());
        assert_eq!(context.stage_name, "test-stage");
        assert_eq!(context.stage_state.status, StageStatus::Pending);
        assert!(context.metadata.is_empty());
    }

    #[test]
    fn test_stage_context_with_metadata() {
        let context = StageContext::new("test-stage".to_string())
            .with_metadata("key".to_string(), serde_json::json!("value"));
        assert_eq!(
            context.get_metadata("key"),
            Some(&serde_json::json!("value"))
        );
    }

    #[test]
    fn test_stage_context_get_metadata_missing() {
        let context = StageContext::new("test-stage".to_string());
        assert_eq!(context.get_metadata("missing"), None);
    }

    #[test]
    fn test_stage_context_multiple_metadata() {
        let context = StageContext::new("test-stage".to_string())
            .with_metadata("key1".to_string(), serde_json::json!("value1"))
            .with_metadata("key2".to_string(), serde_json::json!(42));
        assert_eq!(context.metadata.len(), 2);
        assert_eq!(
            context.get_metadata("key1"),
            Some(&serde_json::json!("value1"))
        );
        assert_eq!(context.get_metadata("key2"), Some(&serde_json::json!(42)));
    }

    // Test implementation of Stage trait
    #[derive(Debug)]
    struct TestStage {
        name: String,
    }

    #[async_trait]
    impl Stage for TestStage {
        async fn execute(
            &self,
            _item: &dyn WorkItem,
            _context: &mut StageContext,
        ) -> Result<StageOutcome> {
            Ok(StageOutcome::Complete)
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    #[tokio::test]
    async fn test_stage_execute() {
        use crate::WorkItem;

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct TestItem {
            id: String,
        }

        impl WorkItem for TestItem {
            fn id(&self) -> &str {
                &self.id
            }
        }

        let stage = TestStage {
            name: "test".to_string(),
        };
        let item = TestItem {
            id: "test-1".to_string(),
        };
        let mut context = StageContext::new("test".to_string());

        let outcome = stage.execute(&item, &mut context).await.unwrap();
        assert_eq!(outcome, StageOutcome::Complete);
    }

    #[tokio::test]
    async fn test_stage_name() {
        let stage = TestStage {
            name: "my-stage".to_string(),
        };
        assert_eq!(stage.name(), "my-stage");
    }

    #[tokio::test]
    async fn test_stage_before_execute() {
        use crate::WorkItem;

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct TestItem {
            id: String,
        }

        impl WorkItem for TestItem {
            fn id(&self) -> &str {
                &self.id
            }
        }

        let stage = TestStage {
            name: "test".to_string(),
        };
        let item = TestItem {
            id: "test-1".to_string(),
        };
        let context = StageContext::new("test".to_string());

        let result = stage.before_execute(&item, &context).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stage_after_execute() {
        use crate::WorkItem;

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct TestItem {
            id: String,
        }

        impl WorkItem for TestItem {
            fn id(&self) -> &str {
                &self.id
            }
        }

        let stage = TestStage {
            name: "test".to_string(),
        };
        let item = TestItem {
            id: "test-1".to_string(),
        };
        let context = StageContext::new("test".to_string());
        let outcome = StageOutcome::Complete;

        let result = stage.after_execute(&item, &context, &outcome).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_stage_trait_object() {
        use crate::WorkItem;

        #[derive(Debug, Clone, Serialize, Deserialize)]
        struct TestItem {
            id: String,
        }

        impl WorkItem for TestItem {
            fn id(&self) -> &str {
                &self.id
            }
        }

        let stage: Box<dyn Stage> = Box::new(TestStage {
            name: "boxed".to_string(),
        });
        let item = TestItem {
            id: "test-1".to_string(),
        };
        let mut context = StageContext::new("boxed".to_string());

        assert_eq!(stage.name(), "boxed");
        let outcome = stage.execute(&item, &mut context).await.unwrap();
        assert_eq!(outcome, StageOutcome::Complete);
    }

    #[test]
    fn test_stage_context_clone() {
        let context = StageContext::new("test".to_string())
            .with_metadata("key".to_string(), serde_json::json!("value"));
        let cloned = context.clone();
        assert_eq!(cloned.stage_name, "test");
        assert_eq!(
            cloned.get_metadata("key"),
            Some(&serde_json::json!("value"))
        );
    }
}
