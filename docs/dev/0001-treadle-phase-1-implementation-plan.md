# Treadle Phase 1 Implementation Plan

## Phase 1: Core Types and Traits

**Goal:** Define the foundational type system. No execution logic yet — just the vocabulary of the crate.

**Prerequisites Checked:**
- [x] Cargo.toml already configured with correct dependencies
- [x] MSRV 1.75, edition 2021
- [x] Dependencies: thiserror 2, serde, serde_json, chrono, async-trait, tokio, petgraph, tracing
- [x] Feature flag `sqlite` with optional rusqlite
- [x] Skeleton lib.rs exists with crate-level documentation

---

## Milestone 1.1 — Error Types and Result Alias

### Files to Create/Modify
- `src/error.rs` (new)
- `src/lib.rs` (modify)

### Implementation Details

#### 1.1.1 Create `src/error.rs`

```rust
//! Error types for the Treadle workflow engine.

use thiserror::Error;

/// The primary error type for the Treadle crate.
///
/// This enum encompasses all error conditions that can occur within the
/// workflow engine, from DAG construction issues to state persistence failures.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TreadleError {
    /// A cycle was detected during workflow construction.
    ///
    /// DAG (Directed Acyclic Graph) workflows must not contain cycles.
    #[error("cycle detected in workflow DAG")]
    DagCycle,

    /// A referenced stage does not exist in the workflow.
    #[error("stage not found: {0}")]
    StageNotFound(String),

    /// A stage name was already registered in the workflow.
    #[error("duplicate stage name: {0}")]
    DuplicateStage(String),

    /// The persistence layer encountered a failure.
    #[error("state store error: {0}")]
    StateStoreError(String),

    /// A stage execution failed.
    #[error("execution error in stage '{stage}': {source}")]
    ExecutionError {
        /// The name of the stage that failed.
        stage: String,
        /// The underlying error.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// An illegal state transition was attempted.
    #[error("invalid state transition from {from:?} to {to:?}")]
    InvalidTransition {
        /// The current state.
        from: crate::stage::StageState,
        /// The attempted target state.
        to: crate::stage::StageState,
    },
}

/// A specialized `Result` type for Treadle operations.
pub type Result<T> = std::result::Result<T, TreadleError>;
```

**Design Decisions:**
- Use `#[non_exhaustive]` to allow adding error variants in future versions (ID-01)
- Use `thiserror` for ergonomic error derivation (EH-07)
- Include `#[source]` for error chaining in `ExecutionError` (EH-18)
- `Box<dyn Error + Send + Sync>` for the execution source to work across async boundaries (AP-03)
- Forward-reference to `StageState` (will be defined in Milestone 1.3)

#### 1.1.2 Update `src/lib.rs`

Add module declaration and re-exports:

```rust
// Module declarations
mod error;

// Re-exports at crate root (PS-19)
pub use error::{TreadleError, Result};
```

### Tests for Milestone 1.1

Add to `src/error.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_cycle_display() {
        let err = TreadleError::DagCycle;
        assert_eq!(err.to_string(), "cycle detected in workflow DAG");
    }

    #[test]
    fn test_stage_not_found_display() {
        let err = TreadleError::StageNotFound("process".to_string());
        assert_eq!(err.to_string(), "stage not found: process");
    }

    #[test]
    fn test_duplicate_stage_display() {
        let err = TreadleError::DuplicateStage("scan".to_string());
        assert_eq!(err.to_string(), "duplicate stage name: scan");
    }

    #[test]
    fn test_state_store_error_display() {
        let err = TreadleError::StateStoreError("connection refused".to_string());
        assert_eq!(err.to_string(), "state store error: connection refused");
    }

    #[test]
    fn test_execution_error_display() {
        let source = std::io::Error::new(std::io::ErrorKind::Other, "network timeout");
        let err = TreadleError::ExecutionError {
            stage: "fetch".to_string(),
            source: Box::new(source),
        };
        assert!(err.to_string().contains("execution error in stage 'fetch'"));
        assert!(err.to_string().contains("network timeout"));
    }

    #[test]
    fn test_result_type_alias() {
        fn returns_ok() -> Result<i32> {
            Ok(42)
        }
        fn returns_err() -> Result<i32> {
            Err(TreadleError::DagCycle)
        }
        assert_eq!(returns_ok().unwrap(), 42);
        assert!(returns_err().is_err());
    }
}
```

### Verification Commands
```bash
cargo build
cargo test error::tests
cargo clippy
```

---

## Milestone 1.2 — WorkItem Trait

### Files to Create/Modify
- `src/work_item.rs` (new)
- `src/lib.rs` (modify)

### Implementation Details

#### 1.2.1 Create `src/work_item.rs`

```rust
//! The `WorkItem` trait defines items that can flow through a workflow.

use std::fmt::Display;
use std::hash::Hash;

/// A work item that can be processed by a Treadle workflow.
///
/// Work items are the entities that flow through the pipeline stages.
/// Each work item must have a unique identifier that can be used for
/// tracking state and logging.
///
/// # Type Parameters
///
/// The associated `Id` type must support:
/// - `Clone` - for copying identifiers
/// - `Eq` + `Hash` - for use as map keys
/// - `Display` - for logging and error messages
/// - `Send + Sync + 'static` - for async and thread-safe operations
///
/// # Example
///
/// ```rust
/// use treadle::WorkItem;
///
/// #[derive(Debug, Clone)]
/// struct Document {
///     id: String,
///     content: String,
/// }
///
/// impl WorkItem for Document {
///     type Id = String;
///
///     fn id(&self) -> &Self::Id {
///         &self.id
///     }
/// }
/// ```
pub trait WorkItem: Send + Sync + 'static {
    /// The identifier type for this work item.
    type Id: Clone + Eq + Hash + Display + Send + Sync + 'static;

    /// Returns a reference to this work item's unique identifier.
    fn id(&self) -> &Self::Id;
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;

    /// A simple test work item for use in unit tests.
    #[derive(Debug, Clone, PartialEq)]
    pub struct TestItem {
        pub id: String,
        pub data: String,
    }

    impl TestItem {
        /// Creates a new test item with the given id.
        pub fn new(id: impl Into<String>) -> Self {
            let id = id.into();
            Self {
                data: format!("data for {}", id),
                id,
            }
        }
    }

    impl WorkItem for TestItem {
        type Id = String;

        fn id(&self) -> &Self::Id {
            &self.id
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_helpers::TestItem;
    use std::collections::HashMap;

    #[test]
    fn test_work_item_id() {
        let item = TestItem::new("doc-123");
        assert_eq!(item.id(), "doc-123");
    }

    #[test]
    fn test_work_item_id_as_map_key() {
        let item1 = TestItem::new("doc-1");
        let item2 = TestItem::new("doc-2");

        let mut map: HashMap<String, &TestItem> = HashMap::new();
        map.insert(item1.id().clone(), &item1);
        map.insert(item2.id().clone(), &item2);

        assert_eq!(map.get("doc-1"), Some(&&item1));
        assert_eq!(map.get("doc-2"), Some(&&item2));
    }

    #[test]
    fn test_work_item_id_display() {
        let item = TestItem::new("doc-456");
        let display = format!("Processing item: {}", item.id());
        assert_eq!(display, "Processing item: doc-456");
    }
}
```

**Design Decisions:**
- `Send + Sync + 'static` bounds enable async workflows (CA-01)
- Associated `Id` type allows flexibility (could be `String`, `Uuid`, `u64`, etc.)
- `Display` bound on `Id` enables logging without generic constraints elsewhere
- Test helper is `pub(crate)` for reuse across test modules (PS-28)

#### 1.2.2 Update `src/lib.rs`

```rust
mod error;
mod work_item;

pub use error::{TreadleError, Result};
pub use work_item::WorkItem;
```

### Verification Commands
```bash
cargo build
cargo test work_item::tests
cargo clippy
```

---

## Milestone 1.3 — Stage State, Status, and Outcome Types

### Files to Create/Modify
- `src/stage.rs` (new)
- `src/lib.rs` (modify)

### Implementation Details

#### 1.3.1 Create `src/stage.rs` (types portion)

```rust
//! Stage types, states, and outcomes for workflow execution.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The execution state of a stage for a particular work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum StageState {
    /// The stage has not yet started.
    Pending,
    /// The stage is currently executing.
    Running,
    /// The stage finished successfully.
    Completed,
    /// The stage execution failed.
    Failed,
    /// The stage is paused, awaiting human review.
    AwaitingReview,
    /// The stage determined there was nothing to do.
    Skipped,
}

impl Default for StageState {
    fn default() -> Self {
        Self::Pending
    }
}

/// Data associated with a human review checkpoint.
///
/// When a stage returns `StageOutcome::AwaitingReview`, it includes
/// this data to provide context for the reviewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewData {
    /// A human-readable summary of what needs review.
    pub summary: String,
    /// Optional structured data for review UIs (JSON-compatible).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// When this review request was created.
    pub created_at: DateTime<Utc>,
}

impl ReviewData {
    /// Creates new review data with the given summary.
    ///
    /// The `created_at` timestamp is set to the current time.
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            details: None,
            created_at: Utc::now(),
        }
    }

    /// Creates new review data with summary and details.
    pub fn with_details(summary: impl Into<String>, details: serde_json::Value) -> Self {
        Self {
            summary: summary.into(),
            details: Some(details),
            created_at: Utc::now(),
        }
    }
}

/// A subtask within a fan-out stage.
///
/// Subtasks represent parallel work items that must all complete
/// before the parent stage can complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubTask {
    /// The name of this subtask.
    pub name: String,
    /// Optional metadata for this subtask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

impl SubTask {
    /// Creates a new subtask with the given name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            metadata: None,
        }
    }

    /// Creates a new subtask with name and metadata.
    pub fn with_metadata(name: impl Into<String>, metadata: serde_json::Value) -> Self {
        Self {
            name: name.into(),
            metadata: Some(metadata),
        }
    }
}

/// The outcome of executing a stage.
///
/// Returned by `Stage::execute` to indicate what happened and what
/// should happen next.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum StageOutcome {
    /// The stage completed successfully.
    Completed,
    /// The stage requires human review before proceeding.
    AwaitingReview(ReviewData),
    /// The stage spawns parallel subtasks that must all complete.
    FanOut(Vec<SubTask>),
    /// The stage determined there was nothing to do.
    Skipped,
}

/// The status of a subtask within a fan-out stage.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubTaskStatus {
    /// The name of this subtask.
    pub name: String,
    /// The current state of this subtask.
    pub state: StageState,
    /// Error message if the subtask failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When this status was last updated.
    pub updated_at: DateTime<Utc>,
}

impl SubTaskStatus {
    /// Creates a new pending subtask status.
    pub fn pending(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: StageState::Pending,
            error: None,
            updated_at: Utc::now(),
        }
    }

    /// Creates a completed subtask status.
    pub fn completed(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: StageState::Completed,
            error: None,
            updated_at: Utc::now(),
        }
    }

    /// Creates a failed subtask status.
    pub fn failed(name: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: StageState::Failed,
            error: Some(error.into()),
            updated_at: Utc::now(),
        }
    }
}

/// The full status of a stage for a particular work item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StageStatus {
    /// The current state of this stage.
    pub state: StageState,
    /// When this status was last updated.
    pub updated_at: DateTime<Utc>,
    /// Error message if the stage failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Review data if the stage is awaiting review.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_data: Option<ReviewData>,
    /// Status of subtasks for fan-out stages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<SubTaskStatus>,
}

impl StageStatus {
    /// Creates a new pending stage status.
    pub fn pending() -> Self {
        Self {
            state: StageState::Pending,
            updated_at: Utc::now(),
            error: None,
            review_data: None,
            subtasks: Vec::new(),
        }
    }

    /// Creates a completed stage status.
    pub fn completed() -> Self {
        Self {
            state: StageState::Completed,
            updated_at: Utc::now(),
            error: None,
            review_data: None,
            subtasks: Vec::new(),
        }
    }

    /// Creates a running stage status.
    pub fn running() -> Self {
        Self {
            state: StageState::Running,
            updated_at: Utc::now(),
            error: None,
            review_data: None,
            subtasks: Vec::new(),
        }
    }

    /// Creates a failed stage status with the given error message.
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            state: StageState::Failed,
            updated_at: Utc::now(),
            error: Some(error.into()),
            review_data: None,
            subtasks: Vec::new(),
        }
    }

    /// Creates an awaiting-review stage status.
    pub fn awaiting_review(data: ReviewData) -> Self {
        Self {
            state: StageState::AwaitingReview,
            updated_at: Utc::now(),
            error: None,
            review_data: Some(data),
            subtasks: Vec::new(),
        }
    }

    /// Creates a skipped stage status.
    pub fn skipped() -> Self {
        Self {
            state: StageState::Skipped,
            updated_at: Utc::now(),
            error: None,
            review_data: None,
            subtasks: Vec::new(),
        }
    }
}

impl Default for StageStatus {
    fn default() -> Self {
        Self::pending()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stage_state_default() {
        assert_eq!(StageState::default(), StageState::Pending);
    }

    #[test]
    fn test_stage_state_serde_roundtrip() {
        let states = [
            StageState::Pending,
            StageState::Running,
            StageState::Completed,
            StageState::Failed,
            StageState::AwaitingReview,
            StageState::Skipped,
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            let parsed: StageState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, parsed);
        }
    }

    #[test]
    fn test_review_data_new() {
        let data = ReviewData::new("Please verify the extracted entities");
        assert_eq!(data.summary, "Please verify the extracted entities");
        assert!(data.details.is_none());
        // created_at should be recent
        let now = Utc::now();
        assert!(data.created_at <= now);
    }

    #[test]
    fn test_review_data_with_details() {
        let details = serde_json::json!({
            "entities": ["Person", "Location"],
            "confidence": 0.85
        });
        let data = ReviewData::with_details("Review entities", details.clone());
        assert_eq!(data.summary, "Review entities");
        assert_eq!(data.details, Some(details));
    }

    #[test]
    fn test_review_data_serde_roundtrip() {
        let data = ReviewData::with_details(
            "Test review",
            serde_json::json!({"key": "value"}),
        );
        let json = serde_json::to_string(&data).unwrap();
        let parsed: ReviewData = serde_json::from_str(&json).unwrap();
        assert_eq!(data.summary, parsed.summary);
        assert_eq!(data.details, parsed.details);
    }

    #[test]
    fn test_subtask_new() {
        let subtask = SubTask::new("source-1");
        assert_eq!(subtask.name, "source-1");
        assert!(subtask.metadata.is_none());
    }

    #[test]
    fn test_subtask_with_metadata() {
        let metadata = serde_json::json!({"url": "https://example.com"});
        let subtask = SubTask::with_metadata("fetch-url", metadata.clone());
        assert_eq!(subtask.name, "fetch-url");
        assert_eq!(subtask.metadata, Some(metadata));
    }

    #[test]
    fn test_subtask_serde_roundtrip() {
        let subtask = SubTask::with_metadata(
            "task-1",
            serde_json::json!({"priority": 1}),
        );
        let json = serde_json::to_string(&subtask).unwrap();
        let parsed: SubTask = serde_json::from_str(&json).unwrap();
        assert_eq!(subtask, parsed);
    }

    #[test]
    fn test_subtask_status_constructors() {
        let pending = SubTaskStatus::pending("task-1");
        assert_eq!(pending.state, StageState::Pending);
        assert!(pending.error.is_none());

        let completed = SubTaskStatus::completed("task-2");
        assert_eq!(completed.state, StageState::Completed);

        let failed = SubTaskStatus::failed("task-3", "timeout");
        assert_eq!(failed.state, StageState::Failed);
        assert_eq!(failed.error, Some("timeout".to_string()));
    }

    #[test]
    fn test_subtask_status_serde_roundtrip() {
        let status = SubTaskStatus::failed("task-x", "connection refused");
        let json = serde_json::to_string(&status).unwrap();
        let parsed: SubTaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status.name, parsed.name);
        assert_eq!(status.state, parsed.state);
        assert_eq!(status.error, parsed.error);
    }

    #[test]
    fn test_stage_status_constructors() {
        assert_eq!(StageStatus::pending().state, StageState::Pending);
        assert_eq!(StageStatus::running().state, StageState::Running);
        assert_eq!(StageStatus::completed().state, StageState::Completed);
        assert_eq!(StageStatus::skipped().state, StageState::Skipped);

        let failed = StageStatus::failed("disk full");
        assert_eq!(failed.state, StageState::Failed);
        assert_eq!(failed.error, Some("disk full".to_string()));

        let review = StageStatus::awaiting_review(ReviewData::new("check this"));
        assert_eq!(review.state, StageState::AwaitingReview);
        assert!(review.review_data.is_some());
    }

    #[test]
    fn test_stage_status_default() {
        let status = StageStatus::default();
        assert_eq!(status.state, StageState::Pending);
        assert!(status.error.is_none());
        assert!(status.review_data.is_none());
        assert!(status.subtasks.is_empty());
    }

    #[test]
    fn test_stage_status_serde_roundtrip() {
        let mut status = StageStatus::awaiting_review(
            ReviewData::with_details("Review", serde_json::json!({"x": 1})),
        );
        status.subtasks.push(SubTaskStatus::completed("sub-1"));
        status.subtasks.push(SubTaskStatus::pending("sub-2"));

        let json = serde_json::to_string(&status).unwrap();
        let parsed: StageStatus = serde_json::from_str(&json).unwrap();

        assert_eq!(status.state, parsed.state);
        assert_eq!(status.subtasks.len(), parsed.subtasks.len());
        assert_eq!(status.review_data.as_ref().unwrap().summary,
                   parsed.review_data.as_ref().unwrap().summary);
    }

    #[test]
    fn test_stage_outcome_variants() {
        let completed = StageOutcome::Completed;
        let skipped = StageOutcome::Skipped;
        let review = StageOutcome::AwaitingReview(ReviewData::new("check"));
        let fanout = StageOutcome::FanOut(vec![
            SubTask::new("task-1"),
            SubTask::new("task-2"),
        ]);

        // Ensure all variants are distinct
        assert_ne!(format!("{:?}", completed), format!("{:?}", skipped));
        assert!(format!("{:?}", review).contains("AwaitingReview"));
        assert!(format!("{:?}", fanout).contains("FanOut"));
    }
}
```

**Design Decisions:**
- `#[non_exhaustive]` on enums for future extensibility (ID-01)
- Serde derives for persistence (the types will be stored in SQLite)
- `skip_serializing_if` to keep JSON clean when fields are None/empty
- Constructor functions follow `new()` convention (ID-09)
- `Default` impl for `StageState` and `StageStatus` (ID-36)
- `impl Into<String>` for flexible string construction (ID-04)

#### 1.3.2 Update `src/lib.rs`

```rust
mod error;
mod stage;
mod work_item;

pub use error::{Result, TreadleError};
pub use stage::{
    ReviewData, StageOutcome, StageState, StageStatus, SubTask, SubTaskStatus,
};
pub use work_item::WorkItem;
```

### Verification Commands
```bash
cargo build
cargo test stage::tests
cargo clippy
```

---

## Milestone 1.4 — Stage Trait and StageContext

### Files to Modify
- `src/stage.rs` (add trait and context)
- `src/lib.rs` (update exports)

### Implementation Details

#### 1.4.1 Add to `src/stage.rs`

Add after the existing types:

```rust
use async_trait::async_trait;
use crate::{Result, WorkItem};

/// Context provided to a stage during execution.
///
/// Contains information about the current execution, useful for logging,
/// metrics, and retry-aware stage implementations.
#[derive(Debug, Clone)]
pub struct StageContext {
    /// The stringified ID of the work item being processed.
    pub item_id: String,
    /// The name of the stage being executed.
    pub stage_name: String,
    /// Which execution attempt this is (1-indexed).
    ///
    /// Stages that support retry can use this to adjust behavior
    /// (e.g., increasing timeouts on later attempts).
    pub attempt: u32,
    /// If this is a fan-out re-invocation, the name of the subtask.
    ///
    /// When a stage returns `StageOutcome::FanOut`, the executor
    /// re-invokes the stage once per subtask with this field set.
    pub subtask_name: Option<String>,
}

impl StageContext {
    /// Creates a new stage context.
    pub fn new(item_id: impl Into<String>, stage_name: impl Into<String>) -> Self {
        Self {
            item_id: item_id.into(),
            stage_name: stage_name.into(),
            attempt: 1,
            subtask_name: None,
        }
    }

    /// Creates a context for a specific attempt number.
    pub fn with_attempt(mut self, attempt: u32) -> Self {
        self.attempt = attempt;
        self
    }

    /// Creates a context for a subtask invocation.
    pub fn with_subtask(mut self, subtask_name: impl Into<String>) -> Self {
        self.subtask_name = Some(subtask_name.into());
        self
    }

    /// Returns true if this is a subtask invocation.
    pub fn is_subtask(&self) -> bool {
        self.subtask_name.is_some()
    }
}

/// A stage in a Treadle workflow.
///
/// Stages are the building blocks of a workflow. Each stage performs
/// some work on a work item and returns an outcome indicating what
/// should happen next.
///
/// # Implementation Notes
///
/// - Stages must be thread-safe (`Send + Sync`) for async execution
/// - The `execute` method receives an immutable reference to the work item
/// - Long-running operations should be cancellation-aware
///
/// # Example
///
/// ```rust,ignore
/// use async_trait::async_trait;
/// use treadle::{Stage, StageContext, StageOutcome, Result, WorkItem};
///
/// struct ScanStage;
///
/// #[async_trait]
/// impl<W: WorkItem> Stage<W> for ScanStage {
///     fn name(&self) -> &str {
///         "scan"
///     }
///
///     async fn execute(&self, item: &W, ctx: &StageContext) -> Result<StageOutcome> {
///         println!("Scanning item: {}", item.id());
///         // Perform scanning logic...
///         Ok(StageOutcome::Completed)
///     }
/// }
/// ```
#[async_trait]
pub trait Stage<W: WorkItem>: Send + Sync {
    /// Returns the name of this stage.
    ///
    /// The name must be unique within a workflow and is used for:
    /// - Identifying the stage in status queries
    /// - Logging and tracing
    /// - State persistence keys
    fn name(&self) -> &str;

    /// Executes this stage on the given work item.
    ///
    /// # Arguments
    ///
    /// * `item` - The work item to process
    /// * `ctx` - Execution context with metadata
    ///
    /// # Returns
    ///
    /// * `Ok(StageOutcome)` - The outcome of execution
    /// * `Err(TreadleError)` - If execution failed
    ///
    /// # Errors
    ///
    /// Implementations should return errors for recoverable failures.
    /// The executor will mark the stage as failed and store the error.
    async fn execute(&self, item: &W, ctx: &StageContext) -> Result<StageOutcome>;
}

// Additional tests for StageContext and Stage trait
#[cfg(test)]
mod stage_tests {
    use super::*;
    use crate::work_item::test_helpers::TestItem;

    #[test]
    fn test_stage_context_new() {
        let ctx = StageContext::new("item-1", "scan");
        assert_eq!(ctx.item_id, "item-1");
        assert_eq!(ctx.stage_name, "scan");
        assert_eq!(ctx.attempt, 1);
        assert!(ctx.subtask_name.is_none());
        assert!(!ctx.is_subtask());
    }

    #[test]
    fn test_stage_context_with_attempt() {
        let ctx = StageContext::new("item-1", "scan").with_attempt(3);
        assert_eq!(ctx.attempt, 3);
    }

    #[test]
    fn test_stage_context_with_subtask() {
        let ctx = StageContext::new("item-1", "enrich")
            .with_subtask("source-1");
        assert!(ctx.is_subtask());
        assert_eq!(ctx.subtask_name, Some("source-1".to_string()));
    }

    // Test implementation of Stage trait
    struct CompletingStage;

    #[async_trait]
    impl Stage<TestItem> for CompletingStage {
        fn name(&self) -> &str {
            "completing"
        }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
            Ok(StageOutcome::Completed)
        }
    }

    struct SkippingStage;

    #[async_trait]
    impl Stage<TestItem> for SkippingStage {
        fn name(&self) -> &str {
            "skipping"
        }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
            Ok(StageOutcome::Skipped)
        }
    }

    struct ReviewStage;

    #[async_trait]
    impl Stage<TestItem> for ReviewStage {
        fn name(&self) -> &str {
            "review"
        }

        async fn execute(&self, item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
            let data = ReviewData::with_details(
                format!("Review item {}", item.id()),
                serde_json::json!({"data": item.data}),
            );
            Ok(StageOutcome::AwaitingReview(data))
        }
    }

    struct FanOutStage;

    #[async_trait]
    impl Stage<TestItem> for FanOutStage {
        fn name(&self) -> &str {
            "fanout"
        }

        async fn execute(&self, _item: &TestItem, ctx: &StageContext) -> Result<StageOutcome> {
            if ctx.is_subtask() {
                // This is a subtask invocation - complete it
                Ok(StageOutcome::Completed)
            } else {
                // Initial invocation - fan out to subtasks
                Ok(StageOutcome::FanOut(vec![
                    SubTask::new("sub-1"),
                    SubTask::new("sub-2"),
                    SubTask::new("sub-3"),
                ]))
            }
        }
    }

    #[tokio::test]
    async fn test_completing_stage() {
        let stage = CompletingStage;
        let item = TestItem::new("test-1");
        let ctx = StageContext::new(item.id(), stage.name());

        let outcome = stage.execute(&item, &ctx).await.unwrap();
        assert_eq!(outcome, StageOutcome::Completed);
    }

    #[tokio::test]
    async fn test_skipping_stage() {
        let stage = SkippingStage;
        let item = TestItem::new("test-1");
        let ctx = StageContext::new(item.id(), stage.name());

        let outcome = stage.execute(&item, &ctx).await.unwrap();
        assert_eq!(outcome, StageOutcome::Skipped);
    }

    #[tokio::test]
    async fn test_review_stage() {
        let stage = ReviewStage;
        let item = TestItem::new("test-1");
        let ctx = StageContext::new(item.id(), stage.name());

        let outcome = stage.execute(&item, &ctx).await.unwrap();
        match outcome {
            StageOutcome::AwaitingReview(data) => {
                assert!(data.summary.contains("test-1"));
            }
            _ => panic!("Expected AwaitingReview outcome"),
        }
    }

    #[tokio::test]
    async fn test_fanout_stage_initial() {
        let stage = FanOutStage;
        let item = TestItem::new("test-1");
        let ctx = StageContext::new(item.id(), stage.name());

        let outcome = stage.execute(&item, &ctx).await.unwrap();
        match outcome {
            StageOutcome::FanOut(subtasks) => {
                assert_eq!(subtasks.len(), 3);
                assert_eq!(subtasks[0].name, "sub-1");
            }
            _ => panic!("Expected FanOut outcome"),
        }
    }

    #[tokio::test]
    async fn test_fanout_stage_subtask() {
        let stage = FanOutStage;
        let item = TestItem::new("test-1");
        let ctx = StageContext::new(item.id(), stage.name())
            .with_subtask("sub-1");

        let outcome = stage.execute(&item, &ctx).await.unwrap();
        assert_eq!(outcome, StageOutcome::Completed);
    }
}
```

**Design Decisions:**
- `async_trait` macro for async trait methods (required until RPITIT stabilizes further)
- Builder-style methods on `StageContext` using `mut self` pattern (ID-35)
- `Send + Sync` bounds on `Stage` trait for async executor compatibility
- Comprehensive example in doc comment (ID-14)
- Tests cover all `StageOutcome` variants

#### 1.4.2 Update `src/lib.rs`

```rust
mod error;
mod stage;
mod work_item;

pub use error::{Result, TreadleError};
pub use stage::{
    ReviewData, Stage, StageContext, StageOutcome, StageState, StageStatus,
    SubTask, SubTaskStatus,
};
pub use work_item::WorkItem;
```

### Verification Commands
```bash
cargo build
cargo test stage::stage_tests
cargo clippy
```

---

## Milestone 1.5 — StateStore Trait

### Files to Create/Modify
- `src/state_store.rs` (new)
- `src/lib.rs` (modify)

### Implementation Details

#### 1.5.1 Create `src/state_store.rs`

```rust
//! State persistence trait for workflow execution state.

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
/// - Item IDs are serialized to strings for storage using `Display`
///
/// # Example
///
/// ```rust,ignore
/// use treadle::{StateStore, StageStatus, StageState};
///
/// async fn check_status<W: WorkItem, S: StateStore<W>>(
///     store: &S,
///     item_id: &W::Id,
///     stage: &str,
/// ) -> bool {
///     match store.get_status(item_id, stage).await {
///         Ok(Some(status)) => status.state == StageState::Completed,
///         _ => false,
///     }
/// }
/// ```
#[async_trait]
pub trait StateStore<W: WorkItem>: Send + Sync {
    /// Retrieves the status of a stage for a work item.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    /// * `stage` - The stage name
    ///
    /// # Returns
    ///
    /// * `Ok(Some(status))` - The stage status if it exists
    /// * `Ok(None)` - If no status has been recorded for this stage
    /// * `Err(...)` - On storage errors
    async fn get_status(&self, item_id: &W::Id, stage: &str) -> Result<Option<StageStatus>>;

    /// Sets the status of a stage for a work item.
    ///
    /// This overwrites any existing status for the stage.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    /// * `stage` - The stage name
    /// * `status` - The new status to store
    async fn set_status(&self, item_id: &W::Id, stage: &str, status: StageStatus) -> Result<()>;

    /// Retrieves all stage statuses for a work item.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    ///
    /// # Returns
    ///
    /// A map from stage name to status for all stages that have
    /// recorded status for this item.
    async fn get_all_statuses(&self, item_id: &W::Id) -> Result<HashMap<String, StageStatus>>;

    /// Queries for work items in a specific state for a stage.
    ///
    /// This is useful for finding items that need processing,
    /// are awaiting review, or have failed.
    ///
    /// # Arguments
    ///
    /// * `stage` - The stage name to query
    /// * `state` - The state to filter by
    ///
    /// # Returns
    ///
    /// A list of work item IDs (as strings) matching the criteria.
    async fn query_items(&self, stage: &str, state: StageState) -> Result<Vec<String>>;

    /// Retrieves all subtask statuses for a fan-out stage.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    /// * `stage` - The fan-out stage name
    ///
    /// # Returns
    ///
    /// A list of subtask statuses, empty if none exist.
    async fn get_subtask_statuses(
        &self,
        item_id: &W::Id,
        stage: &str,
    ) -> Result<Vec<SubTaskStatus>>;

    /// Sets the status of a specific subtask.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    /// * `stage` - The fan-out stage name
    /// * `subtask` - The subtask name
    /// * `status` - The new subtask status
    async fn set_subtask_status(
        &self,
        item_id: &W::Id,
        stage: &str,
        subtask: &str,
        status: SubTaskStatus,
    ) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::test_helpers::TestItem;
    use std::sync::Arc;

    // Minimal mock implementation for trait testing
    struct MockStateStore;

    #[async_trait]
    impl StateStore<TestItem> for MockStateStore {
        async fn get_status(
            &self,
            _item_id: &String,
            _stage: &str,
        ) -> Result<Option<StageStatus>> {
            Ok(None)
        }

        async fn set_status(
            &self,
            _item_id: &String,
            _stage: &str,
            _status: StageStatus,
        ) -> Result<()> {
            Ok(())
        }

        async fn get_all_statuses(
            &self,
            _item_id: &String,
        ) -> Result<HashMap<String, StageStatus>> {
            Ok(HashMap::new())
        }

        async fn query_items(&self, _stage: &str, _state: StageState) -> Result<Vec<String>> {
            Ok(Vec::new())
        }

        async fn get_subtask_statuses(
            &self,
            _item_id: &String,
            _stage: &str,
        ) -> Result<Vec<SubTaskStatus>> {
            Ok(Vec::new())
        }

        async fn set_subtask_status(
            &self,
            _item_id: &String,
            _stage: &str,
            _subtask: &str,
            _status: SubTaskStatus,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_state_store_trait_is_object_safe() {
        // Verify the trait can be used as a trait object
        let store: Arc<dyn StateStore<TestItem>> = Arc::new(MockStateStore);
        let result = store.get_status(&"item-1".to_string(), "scan").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_mock_store_methods() {
        let store = MockStateStore;
        let item_id = "item-1".to_string();

        // Test get_status
        let status = store.get_status(&item_id, "scan").await.unwrap();
        assert!(status.is_none());

        // Test set_status
        let result = store
            .set_status(&item_id, "scan", StageStatus::pending())
            .await;
        assert!(result.is_ok());

        // Test get_all_statuses
        let all = store.get_all_statuses(&item_id).await.unwrap();
        assert!(all.is_empty());

        // Test query_items
        let items = store
            .query_items("scan", StageState::Pending)
            .await
            .unwrap();
        assert!(items.is_empty());

        // Test subtask methods
        let subtasks = store.get_subtask_statuses(&item_id, "enrich").await.unwrap();
        assert!(subtasks.is_empty());

        let result = store
            .set_subtask_status(
                &item_id,
                "enrich",
                "sub-1",
                SubTaskStatus::pending("sub-1"),
            )
            .await;
        assert!(result.is_ok());
    }
}
```

**Design Decisions:**
- Trait is generic over `W: WorkItem` for type-safe item ID handling
- All methods are async for flexibility with sync/async backends
- `Send + Sync` bounds enable use across async tasks
- `query_items` returns `Vec<String>` (not `Vec<W::Id>`) because storage backends work with strings
- Comprehensive doc comments with examples (ID-14)
- Object safety verified in tests (can use `dyn StateStore<W>`)

#### 1.5.2 Update `src/lib.rs`

Final version for Phase 1:

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
//! use treadle::{Workflow, Stage, StageOutcome, StageContext, Result};
//!
//! // Define a stage
//! struct ScanStage;
//!
//! #[async_trait::async_trait]
//! impl<W: WorkItem> Stage<W> for ScanStage {
//!     fn name(&self) -> &str { "scan" }
//!
//!     async fn execute(&self, item: &W, ctx: &StageContext) -> Result<StageOutcome> {
//!         // Your scanning logic here
//!         Ok(StageOutcome::Completed)
//!     }
//! }
//! ```
//!
//! ## Feature Flags
//!
//! - `sqlite` (default): Enables the SQLite-backed state store

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

mod error;
mod stage;
mod state_store;
mod work_item;

// Re-exports at crate root for ergonomic imports
pub use error::{Result, TreadleError};
pub use stage::{
    ReviewData, Stage, StageContext, StageOutcome, StageState, StageStatus,
    SubTask, SubTaskStatus,
};
pub use state_store::StateStore;
pub use work_item::WorkItem;

/// Returns the crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

### Verification Commands
```bash
cargo build
cargo test
cargo clippy
cargo doc --no-deps --open
```

---

## Phase 1 Completion Checklist

After completing all milestones, verify:

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes all tests
- [ ] `cargo clippy -- -D warnings` shows no warnings
- [ ] `cargo doc --no-deps` builds without warnings
- [ ] All public types have rustdoc comments
- [ ] All re-exports are at crate root for ergonomic imports

### Expected Public API After Phase 1

```rust
// Error handling
treadle::TreadleError
treadle::Result<T>

// Work items
treadle::WorkItem

// Stage types
treadle::Stage
treadle::StageContext
treadle::StageState
treadle::StageStatus
treadle::StageOutcome
treadle::SubTask
treadle::SubTaskStatus
treadle::ReviewData

// State persistence
treadle::StateStore
```

---

## Notes on Rust Guidelines Applied

### Anti-Patterns Avoided (11-anti-patterns.md)
- AP-02: Using `&str` instead of `&String` in function parameters
- AP-03: `Send + Sync` on error source types for async compatibility
- AP-09: No `unwrap()` in library code
- AP-61: Using thiserror, not `String` as error type

### Core Idioms Applied (01-core-idioms.md)
- ID-01: `#[non_exhaustive]` on public enums
- ID-04: `impl Into<String>` for constructor parameters
- ID-09: `new()` as canonical constructor
- ID-11: Derive common traits (Debug, Clone, PartialEq, Serialize, Deserialize)
- ID-36: Implement `Default` where sensible

### Error Handling Patterns (03-error-handling.md)
- EH-07: Custom error types with thiserror
- EH-15: Error types implement `std::error::Error`
- EH-18: Source chain via `#[source]`

### Project Structure (12-project-structure.md)
- PS-19: Re-export main types at crate root
- PS-28: `pub(crate)` for test helpers
