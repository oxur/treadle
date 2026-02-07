# Treadle Phase 4 Implementation Plan

## Phase 4: Workflow Executor

**Goal:** The `advance` method that drives items through the pipeline, plus the event stream.

**Prerequisites:**
- Phases 1-3 completed
- `cargo build` and `cargo test` pass

---

## Milestone 4.1 — Event Types and Broadcast Channel

### Files to Create/Modify
- `src/event.rs` (new)
- `src/workflow.rs` (add event channel)
- `src/lib.rs` (update exports)

### Implementation Details

#### 4.1.1 Create `src/event.rs`

```rust
//! Workflow execution events.
//!
//! This module provides [`WorkflowEvent`] for observing workflow execution.
//! Events are broadcast through a channel that can be subscribed to for
//! monitoring, logging, or building UIs.

use crate::ReviewData;

/// An event emitted during workflow execution.
///
/// Events use `String` for item IDs (not generic `W::Id`) to keep the
/// event type non-generic and easy to serialize for logging or transmission.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum WorkflowEvent {
    /// A stage has started executing.
    StageStarted {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The stage name.
        stage: String,
    },

    /// A stage completed successfully.
    StageCompleted {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The stage name.
        stage: String,
    },

    /// A stage execution failed.
    StageFailed {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The stage name.
        stage: String,
        /// Error message describing the failure.
        error: String,
    },

    /// A stage requires human review before proceeding.
    ReviewRequired {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The stage name.
        stage: String,
        /// Data for the reviewer.
        data: ReviewData,
    },

    /// A stage was skipped (nothing to do).
    StageSkipped {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The stage name.
        stage: String,
    },

    /// A fan-out stage has declared its subtasks.
    FanOutStarted {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The stage name.
        stage: String,
        /// Names of the subtasks to execute.
        subtasks: Vec<String>,
    },

    /// A subtask within a fan-out stage has started.
    SubTaskStarted {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The parent stage name.
        stage: String,
        /// The subtask name.
        subtask: String,
    },

    /// A subtask within a fan-out stage completed.
    SubTaskCompleted {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The parent stage name.
        stage: String,
        /// The subtask name.
        subtask: String,
    },

    /// A subtask within a fan-out stage failed.
    SubTaskFailed {
        /// The work item's identifier (stringified).
        item_id: String,
        /// The parent stage name.
        stage: String,
        /// The subtask name.
        subtask: String,
        /// Error message describing the failure.
        error: String,
    },

    /// The workflow completed for a work item (all stages done).
    WorkflowCompleted {
        /// The work item's identifier (stringified).
        item_id: String,
    },
}

impl WorkflowEvent {
    /// Returns the item ID for this event.
    pub fn item_id(&self) -> &str {
        match self {
            Self::StageStarted { item_id, .. }
            | Self::StageCompleted { item_id, .. }
            | Self::StageFailed { item_id, .. }
            | Self::ReviewRequired { item_id, .. }
            | Self::StageSkipped { item_id, .. }
            | Self::FanOutStarted { item_id, .. }
            | Self::SubTaskStarted { item_id, .. }
            | Self::SubTaskCompleted { item_id, .. }
            | Self::SubTaskFailed { item_id, .. }
            | Self::WorkflowCompleted { item_id } => item_id,
        }
    }

    /// Returns the stage name for this event, if applicable.
    pub fn stage(&self) -> Option<&str> {
        match self {
            Self::StageStarted { stage, .. }
            | Self::StageCompleted { stage, .. }
            | Self::StageFailed { stage, .. }
            | Self::ReviewRequired { stage, .. }
            | Self::StageSkipped { stage, .. }
            | Self::FanOutStarted { stage, .. }
            | Self::SubTaskStarted { stage, .. }
            | Self::SubTaskCompleted { stage, .. }
            | Self::SubTaskFailed { stage, .. } => Some(stage),
            Self::WorkflowCompleted { .. } => None,
        }
    }

    /// Returns true if this is an error event.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::StageFailed { .. } | Self::SubTaskFailed { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_item_id() {
        let event = WorkflowEvent::StageStarted {
            item_id: "item-1".to_string(),
            stage: "scan".to_string(),
        };
        assert_eq!(event.item_id(), "item-1");
    }

    #[test]
    fn test_event_stage() {
        let event = WorkflowEvent::StageCompleted {
            item_id: "item-1".to_string(),
            stage: "scan".to_string(),
        };
        assert_eq!(event.stage(), Some("scan"));

        let event = WorkflowEvent::WorkflowCompleted {
            item_id: "item-1".to_string(),
        };
        assert_eq!(event.stage(), None);
    }

    #[test]
    fn test_is_error() {
        let success = WorkflowEvent::StageCompleted {
            item_id: "x".to_string(),
            stage: "s".to_string(),
        };
        assert!(!success.is_error());

        let failure = WorkflowEvent::StageFailed {
            item_id: "x".to_string(),
            stage: "s".to_string(),
            error: "err".to_string(),
        };
        assert!(failure.is_error());

        let subtask_failure = WorkflowEvent::SubTaskFailed {
            item_id: "x".to_string(),
            stage: "s".to_string(),
            subtask: "st".to_string(),
            error: "err".to_string(),
        };
        assert!(subtask_failure.is_error());
    }
}
```

#### 4.1.2 Update `src/workflow.rs` with Event Channel

Add to imports:

```rust
use tokio::sync::broadcast;
use crate::event::WorkflowEvent;
```

Update `Workflow` struct:

```rust
/// Default channel capacity for workflow events.
const DEFAULT_EVENT_CHANNEL_CAPACITY: usize = 256;

pub struct Workflow<W: WorkItem> {
    /// The underlying directed graph.
    graph: DiGraph<RegisteredStage<W>, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Cached topological order of stage names.
    topo_order: Vec<String>,
    /// Event broadcast channel sender.
    event_tx: broadcast::Sender<WorkflowEvent>,
}

impl<W: WorkItem> Workflow<W> {
    /// Subscribes to workflow execution events.
    ///
    /// Returns a receiver that will receive all events broadcast by this
    /// workflow. Events are not persisted; if the receiver is too slow,
    /// events may be dropped.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut events = workflow.subscribe();
    /// tokio::spawn(async move {
    ///     while let Ok(event) = events.recv().await {
    ///         println!("Event: {:?}", event);
    ///     }
    /// });
    /// ```
    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_tx.subscribe()
    }

    /// Emits an event to all subscribers.
    ///
    /// Ignores send errors (no subscribers or channel full).
    pub(crate) fn emit(&self, event: WorkflowEvent) {
        let _ = self.event_tx.send(event);
    }
}
```

Update `WorkflowBuilder::build()`:

```rust
pub fn build(mut self) -> Result<Workflow<W>> {
    // ... existing validation code ...

    // Create event channel
    let (event_tx, _) = broadcast::channel(DEFAULT_EVENT_CHANNEL_CAPACITY);

    Ok(Workflow {
        graph: self.graph,
        name_to_index: self.name_to_index,
        topo_order,
        event_tx,
    })
}
```

#### 4.1.3 Update `src/lib.rs`

```rust
mod error;
mod event;
mod stage;
mod state_store;
mod work_item;
mod workflow;

// ... existing exports ...

pub use event::WorkflowEvent;
```

#### 4.1.4 Add Event Tests

Add to `src/workflow.rs` tests:

```rust
mod event_tests {
    use super::*;
    use crate::WorkflowEvent;

    #[tokio::test]
    async fn test_subscribe_and_receive() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut receiver = workflow.subscribe();

        // Emit an event
        workflow.emit(WorkflowEvent::StageStarted {
            item_id: "item-1".to_string(),
            stage: "a".to_string(),
        });

        // Receive it
        let event = receiver.recv().await.unwrap();
        assert_eq!(event.item_id(), "item-1");
        assert_eq!(event.stage(), Some("a"));
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut rx1 = workflow.subscribe();
        let mut rx2 = workflow.subscribe();

        workflow.emit(WorkflowEvent::StageCompleted {
            item_id: "item-1".to_string(),
            stage: "a".to_string(),
        });

        // Both receivers get the event
        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();

        assert_eq!(e1.item_id(), e2.item_id());
    }
}
```

### Verification Commands
```bash
cargo build
cargo test event
cargo test workflow::event_tests
cargo clippy
```

---

## Milestone 4.2 — Core Advance Logic (Single Stage Execution)

### Files to Create/Modify
- `src/executor.rs` (new)
- `src/workflow.rs` (integrate executor)
- `src/lib.rs` (update if needed)

### Implementation Details

#### 4.2.1 Create `src/executor.rs`

```rust
//! Workflow execution engine.
//!
//! This module provides the core execution logic for advancing work items
//! through a workflow.

use crate::{
    Result, Stage, StageContext, StageOutcome, StageState, StageStatus,
    StateStore, SubTaskStatus, TreadleError, WorkItem, Workflow, WorkflowEvent,
};

impl<W: WorkItem> Workflow<W> {
    /// Advances a work item through the workflow.
    ///
    /// This method:
    /// 1. Finds all stages that are ready to execute
    /// 2. Executes each ready stage
    /// 3. Updates status based on outcomes
    /// 4. Recursively advances if progress was made
    ///
    /// Execution stops when:
    /// - No more stages are ready (workflow blocked or complete)
    /// - A stage returns `AwaitingReview`
    /// - A stage fails
    ///
    /// # Arguments
    ///
    /// * `item` - The work item to advance
    /// * `store` - The state store for persistence
    ///
    /// # Returns
    ///
    /// `Ok(())` when no more progress can be made in this call.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Advance until blocked or complete
    /// workflow.advance(&item, &store).await?;
    ///
    /// if workflow.is_complete(&item.id(), &store).await? {
    ///     println!("Workflow complete!");
    /// } else if workflow.is_blocked(&item.id(), &store).await? {
    ///     println!("Workflow blocked (review or failure)");
    /// }
    /// ```
    pub async fn advance<S: StateStore<W>>(&self, item: &W, store: &S) -> Result<()> {
        self.advance_internal(item, store, 0).await
    }

    /// Internal recursive advance with depth tracking.
    async fn advance_internal<S: StateStore<W>>(
        &self,
        item: &W,
        store: &S,
        depth: usize,
    ) -> Result<()> {
        // Safety limit to prevent infinite recursion
        const MAX_DEPTH: usize = 100;
        if depth > MAX_DEPTH {
            return Err(TreadleError::ExecutionError {
                stage: "advance".to_string(),
                source: "maximum recursion depth exceeded".into(),
            });
        }

        let ready_stages = self.ready_stages(item.id(), store).await?;
        if ready_stages.is_empty() {
            // Check if complete
            if self.is_complete(item.id(), store).await? {
                self.emit(WorkflowEvent::WorkflowCompleted {
                    item_id: item.id().to_string(),
                });
            }
            return Ok(());
        }

        let mut made_progress = false;

        for stage_name in ready_stages {
            let outcome = self
                .execute_stage(item, &stage_name, store, None)
                .await;

            match outcome {
                Ok(StageOutcome::Completed) => {
                    made_progress = true;
                }
                Ok(StageOutcome::Skipped) => {
                    made_progress = true;
                }
                Ok(StageOutcome::AwaitingReview(_)) => {
                    // Stop advancing this path - review required
                }
                Ok(StageOutcome::FanOut(subtasks)) => {
                    // Handle fan-out
                    let fanout_result = self
                        .execute_fanout(item, &stage_name, subtasks, store)
                        .await;

                    match fanout_result {
                        Ok(true) => made_progress = true,
                        Ok(false) => {}
                        Err(e) => return Err(e),
                    }
                }
                Err(_) => {
                    // Stage failed - already recorded, stop this path
                }
            }
        }

        // Recurse if we made progress
        if made_progress {
            self.advance_internal(item, store, depth + 1).await?;
        }

        Ok(())
    }

    /// Executes a single stage.
    async fn execute_stage<S: StateStore<W>>(
        &self,
        item: &W,
        stage_name: &str,
        store: &S,
        subtask_name: Option<&str>,
    ) -> Result<StageOutcome> {
        let stage = self.get_stage(stage_name)?;
        let item_id_str = item.id().to_string();

        // Build context
        let mut ctx = StageContext::new(&item_id_str, stage_name);
        if let Some(subtask) = subtask_name {
            ctx = ctx.with_subtask(subtask);
        }

        // Emit start event (for non-subtask executions)
        if subtask_name.is_none() {
            // Mark as running
            store
                .set_status(item.id(), stage_name, StageStatus::running())
                .await?;

            self.emit(WorkflowEvent::StageStarted {
                item_id: item_id_str.clone(),
                stage: stage_name.to_string(),
            });
        }

        // Execute the stage
        let result = stage.execute(item, &ctx).await;

        match result {
            Ok(outcome) => {
                // Handle outcome (for non-subtask executions)
                if subtask_name.is_none() {
                    self.handle_outcome(item, stage_name, &outcome, store).await?;
                }
                Ok(outcome)
            }
            Err(e) => {
                // Mark as failed
                let error_msg = e.to_string();
                store
                    .set_status(item.id(), stage_name, StageStatus::failed(&error_msg))
                    .await?;

                self.emit(WorkflowEvent::StageFailed {
                    item_id: item_id_str,
                    stage: stage_name.to_string(),
                    error: error_msg,
                });

                Err(e)
            }
        }
    }

    /// Handles a successful stage outcome.
    async fn handle_outcome<S: StateStore<W>>(
        &self,
        item: &W,
        stage_name: &str,
        outcome: &StageOutcome,
        store: &S,
    ) -> Result<()> {
        let item_id_str = item.id().to_string();

        match outcome {
            StageOutcome::Completed => {
                store
                    .set_status(item.id(), stage_name, StageStatus::completed())
                    .await?;

                self.emit(WorkflowEvent::StageCompleted {
                    item_id: item_id_str,
                    stage: stage_name.to_string(),
                });
            }
            StageOutcome::Skipped => {
                store
                    .set_status(item.id(), stage_name, StageStatus::skipped())
                    .await?;

                self.emit(WorkflowEvent::StageSkipped {
                    item_id: item_id_str,
                    stage: stage_name.to_string(),
                });
            }
            StageOutcome::AwaitingReview(data) => {
                store
                    .set_status(
                        item.id(),
                        stage_name,
                        StageStatus::awaiting_review(data.clone()),
                    )
                    .await?;

                self.emit(WorkflowEvent::ReviewRequired {
                    item_id: item_id_str,
                    stage: stage_name.to_string(),
                    data: data.clone(),
                });
            }
            StageOutcome::FanOut(_) => {
                // Fan-out handling is done separately
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::test_helpers::TestItem;
    use crate::{MemoryStateStore, ReviewData, SubTask};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // Helper stage that always completes
    struct CompletingStage {
        name: String,
        call_count: Arc<AtomicU32>,
    }

    impl CompletingStage {
        fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                call_count: Arc::new(AtomicU32::new(0)),
            }
        }

        fn with_counter(name: impl Into<String>, counter: Arc<AtomicU32>) -> Self {
            Self {
                name: name.into(),
                call_count: counter,
            }
        }
    }

    #[async_trait]
    impl Stage<TestItem> for CompletingStage {
        fn name(&self) -> &str {
            &self.name
        }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(StageOutcome::Completed)
        }
    }

    // Helper stage that skips
    struct SkippingStage {
        name: String,
    }

    #[async_trait]
    impl Stage<TestItem> for SkippingStage {
        fn name(&self) -> &str {
            &self.name
        }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
            Ok(StageOutcome::Skipped)
        }
    }

    // Helper stage that requires review
    struct ReviewStage {
        name: String,
    }

    #[async_trait]
    impl Stage<TestItem> for ReviewStage {
        fn name(&self) -> &str {
            &self.name
        }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
            Ok(StageOutcome::AwaitingReview(ReviewData::new("Please review")))
        }
    }

    // Helper stage that fails
    struct FailingStage {
        name: String,
    }

    #[async_trait]
    impl Stage<TestItem> for FailingStage {
        fn name(&self) -> &str {
            &self.name
        }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
            Err(TreadleError::ExecutionError {
                stage: self.name.clone(),
                source: "intentional failure".into(),
            })
        }
    }

    #[tokio::test]
    async fn test_advance_linear_pipeline() {
        let counter_a = Arc::new(AtomicU32::new(0));
        let counter_b = Arc::new(AtomicU32::new(0));
        let counter_c = Arc::new(AtomicU32::new(0));

        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::with_counter("a", counter_a.clone()))
            .stage("b", CompletingStage::with_counter("b", counter_b.clone()))
            .stage("c", CompletingStage::with_counter("c", counter_c.clone()))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // All stages should have been called
        assert_eq!(counter_a.load(Ordering::SeqCst), 1);
        assert_eq!(counter_b.load(Ordering::SeqCst), 1);
        assert_eq!(counter_c.load(Ordering::SeqCst), 1);

        // Workflow should be complete
        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_stops_at_review() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::new("a"))
            .stage("review", ReviewStage { name: "review".to_string() })
            .stage("c", CompletingStage::new("c"))
            .dependency("review", "a")
            .dependency("c", "review")
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // a and review should be executed, c should not
        let status_a = store.get_status(&item.id, "a").await.unwrap().unwrap();
        let status_review = store.get_status(&item.id, "review").await.unwrap().unwrap();
        let status_c = store.get_status(&item.id, "c").await.unwrap();

        assert_eq!(status_a.state, StageState::Completed);
        assert_eq!(status_review.state, StageState::AwaitingReview);
        assert!(status_c.is_none()); // Not executed yet

        // Workflow is blocked
        assert!(workflow.is_blocked(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_after_review_approval() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::new("a"))
            .stage("review", ReviewStage { name: "review".to_string() })
            .stage("c", CompletingStage::new("c"))
            .dependency("review", "a")
            .dependency("c", "review")
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        // First advance
        workflow.advance(&item, &store).await.unwrap();

        // Manually approve the review
        store
            .set_status(&item.id, "review", StageStatus::completed())
            .await
            .unwrap();

        // Second advance
        workflow.advance(&item, &store).await.unwrap();

        // Now c should be complete
        let status_c = store.get_status(&item.id, "c").await.unwrap().unwrap();
        assert_eq!(status_c.state, StageState::Completed);

        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_stage_failure() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::new("a"))
            .stage("fail", FailingStage { name: "fail".to_string() })
            .stage("c", CompletingStage::new("c"))
            .dependency("fail", "a")
            .dependency("c", "fail")
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // a should complete, fail should fail, c should not run
        let status_a = store.get_status(&item.id, "a").await.unwrap().unwrap();
        let status_fail = store.get_status(&item.id, "fail").await.unwrap().unwrap();
        let status_c = store.get_status(&item.id, "c").await.unwrap();

        assert_eq!(status_a.state, StageState::Completed);
        assert_eq!(status_fail.state, StageState::Failed);
        assert!(status_fail.error.is_some());
        assert!(status_c.is_none());

        // Workflow is blocked (due to failure)
        assert!(workflow.is_blocked(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_diamond_dag() {
        let counter_b = Arc::new(AtomicU32::new(0));
        let counter_c = Arc::new(AtomicU32::new(0));
        let counter_d = Arc::new(AtomicU32::new(0));

        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::new("a"))
            .stage("b", CompletingStage::with_counter("b", counter_b.clone()))
            .stage("c", CompletingStage::with_counter("c", counter_c.clone()))
            .stage("d", CompletingStage::with_counter("d", counter_d.clone()))
            .dependency("b", "a")
            .dependency("c", "a")
            .dependency("d", "b")
            .dependency("d", "c")
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // All stages should complete
        assert_eq!(counter_b.load(Ordering::SeqCst), 1);
        assert_eq!(counter_c.load(Ordering::SeqCst), 1);
        assert_eq!(counter_d.load(Ordering::SeqCst), 1);

        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_emits_events() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::new("a"))
            .stage("b", CompletingStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let mut receiver = workflow.subscribe();
        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // Collect events
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }

        // Should have: StageStarted(a), StageCompleted(a), StageStarted(b), StageCompleted(b), WorkflowCompleted
        assert!(events.len() >= 4);

        let stage_names: Vec<_> = events.iter().filter_map(|e| e.stage()).collect();
        assert!(stage_names.contains(&"a"));
        assert!(stage_names.contains(&"b"));
    }

    #[tokio::test]
    async fn test_advance_skipping_stage() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", SkippingStage { name: "a".to_string() })
            .stage("b", CompletingStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let status_a = store.get_status(&item.id, "a").await.unwrap().unwrap();
        let status_b = store.get_status(&item.id, "b").await.unwrap().unwrap();

        assert_eq!(status_a.state, StageState::Skipped);
        assert_eq!(status_b.state, StageState::Completed);

        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_idempotent_when_complete() {
        let counter = Arc::new(AtomicU32::new(0));

        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::with_counter("a", counter.clone()))
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        // First advance
        workflow.advance(&item, &store).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Second advance should be a no-op
        workflow.advance(&item, &store).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_advance_no_stages_is_noop() {
        let workflow: Workflow<TestItem> = Workflow::builder().build().unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        // Should not error
        workflow.advance(&item, &store).await.unwrap();

        // Empty workflow is complete
        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }
}
```

**Design Decisions:**
- Recursive `advance` with depth limit (prevents infinite loops)
- Events emitted for observability
- Stage failures recorded but don't propagate as errors (allows continuing other branches)
- Idempotent execution (completed stages not re-executed)

### Verification Commands
```bash
cargo build
cargo test executor::tests
cargo clippy
```

---

## Milestone 4.3 — Fan-Out Execution

### Files to Modify
- `src/executor.rs` (add fan-out support)
- `src/stage.rs` (add `subtask_name` to `StageContext`)

### Implementation Details

#### 4.3.1 Update `StageContext` (already done in Phase 1)

The `StageContext` already has the `subtask_name` field. Verify it exists.

#### 4.3.2 Add Fan-Out Execution

Add to `src/executor.rs` (or `src/workflow.rs`):

```rust
impl<W: WorkItem> Workflow<W> {
    /// Executes a fan-out stage.
    ///
    /// Returns `true` if all subtasks completed successfully.
    async fn execute_fanout<S: StateStore<W>>(
        &self,
        item: &W,
        stage_name: &str,
        subtasks: Vec<crate::SubTask>,
        store: &S,
    ) -> Result<bool> {
        let item_id_str = item.id().to_string();

        // Emit fan-out started event
        let subtask_names: Vec<String> = subtasks.iter().map(|s| s.name.clone()).collect();
        self.emit(WorkflowEvent::FanOutStarted {
            item_id: item_id_str.clone(),
            stage: stage_name.to_string(),
            subtasks: subtask_names.clone(),
        });

        // Initialize subtask statuses
        for subtask in &subtasks {
            store
                .set_subtask_status(
                    item.id(),
                    stage_name,
                    &subtask.name,
                    SubTaskStatus::pending(&subtask.name),
                )
                .await?;
        }

        let stage = self.get_stage(stage_name)?;
        let mut all_completed = true;
        let mut any_failed = false;

        // Execute each subtask
        for subtask in &subtasks {
            self.emit(WorkflowEvent::SubTaskStarted {
                item_id: item_id_str.clone(),
                stage: stage_name.to_string(),
                subtask: subtask.name.clone(),
            });

            // Mark subtask as running (implicitly by executing)
            let ctx = StageContext::new(&item_id_str, stage_name)
                .with_subtask(&subtask.name);

            let result = stage.execute(item, &ctx).await;

            match result {
                Ok(StageOutcome::Completed) => {
                    store
                        .set_subtask_status(
                            item.id(),
                            stage_name,
                            &subtask.name,
                            SubTaskStatus::completed(&subtask.name),
                        )
                        .await?;

                    self.emit(WorkflowEvent::SubTaskCompleted {
                        item_id: item_id_str.clone(),
                        stage: stage_name.to_string(),
                        subtask: subtask.name.clone(),
                    });
                }
                Ok(StageOutcome::Skipped) => {
                    // Treat skipped subtask as completed
                    store
                        .set_subtask_status(
                            item.id(),
                            stage_name,
                            &subtask.name,
                            SubTaskStatus::completed(&subtask.name),
                        )
                        .await?;

                    self.emit(WorkflowEvent::SubTaskCompleted {
                        item_id: item_id_str.clone(),
                        stage: stage_name.to_string(),
                        subtask: subtask.name.clone(),
                    });
                }
                Ok(_) => {
                    // Subtasks shouldn't return FanOut or AwaitingReview
                    all_completed = false;
                }
                Err(e) => {
                    let error_msg = e.to_string();
                    store
                        .set_subtask_status(
                            item.id(),
                            stage_name,
                            &subtask.name,
                            SubTaskStatus::failed(&subtask.name, &error_msg),
                        )
                        .await?;

                    self.emit(WorkflowEvent::SubTaskFailed {
                        item_id: item_id_str.clone(),
                        stage: stage_name.to_string(),
                        subtask: subtask.name.clone(),
                        error: error_msg,
                    });

                    any_failed = true;
                }
            }
        }

        // Update parent stage status based on subtask outcomes
        if any_failed {
            store
                .set_status(
                    item.id(),
                    stage_name,
                    StageStatus::failed("one or more subtasks failed"),
                )
                .await?;

            self.emit(WorkflowEvent::StageFailed {
                item_id: item_id_str,
                stage: stage_name.to_string(),
                error: "one or more subtasks failed".to_string(),
            });

            Ok(false)
        } else if all_completed {
            store
                .set_status(item.id(), stage_name, StageStatus::completed())
                .await?;

            self.emit(WorkflowEvent::StageCompleted {
                item_id: item_id_str,
                stage: stage_name.to_string(),
            });

            Ok(true)
        } else {
            // Some subtasks didn't complete normally
            Ok(false)
        }
    }
}
```

#### 4.3.3 Add Fan-Out Tests

```rust
mod fanout_tests {
    use super::*;
    use crate::work_item::test_helpers::TestItem;
    use crate::{MemoryStateStore, SubTask};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // Stage that fans out to 3 subtasks
    struct FanOutStage {
        subtask_counter: Arc<AtomicU32>,
    }

    #[async_trait]
    impl Stage<TestItem> for FanOutStage {
        fn name(&self) -> &str {
            "fanout"
        }

        async fn execute(&self, _item: &TestItem, ctx: &StageContext) -> Result<StageOutcome> {
            if let Some(_subtask) = &ctx.subtask_name {
                // Subtask execution
                self.subtask_counter.fetch_add(1, Ordering::SeqCst);
                Ok(StageOutcome::Completed)
            } else {
                // Initial invocation - declare subtasks
                Ok(StageOutcome::FanOut(vec![
                    SubTask::new("sub-1"),
                    SubTask::new("sub-2"),
                    SubTask::new("sub-3"),
                ]))
            }
        }
    }

    // Stage that has one failing subtask
    struct PartialFailFanOutStage;

    #[async_trait]
    impl Stage<TestItem> for PartialFailFanOutStage {
        fn name(&self) -> &str {
            "partial-fail"
        }

        async fn execute(&self, _item: &TestItem, ctx: &StageContext) -> Result<StageOutcome> {
            match ctx.subtask_name.as_deref() {
                None => Ok(StageOutcome::FanOut(vec![
                    SubTask::new("ok-1"),
                    SubTask::new("fail"),
                    SubTask::new("ok-2"),
                ])),
                Some("fail") => Err(TreadleError::ExecutionError {
                    stage: "partial-fail".to_string(),
                    source: "subtask failed".into(),
                }),
                Some(_) => Ok(StageOutcome::Completed),
            }
        }
    }

    #[tokio::test]
    async fn test_fanout_all_complete() {
        let counter = Arc::new(AtomicU32::new(0));

        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("fanout", FanOutStage { subtask_counter: counter.clone() })
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // All 3 subtasks should have been executed
        assert_eq!(counter.load(Ordering::SeqCst), 3);

        // Stage should be complete
        let status = store.get_status(&item.id, "fanout").await.unwrap().unwrap();
        assert_eq!(status.state, StageState::Completed);

        // Subtasks should be recorded
        let subtasks = store.get_subtask_statuses(&item.id, "fanout").await.unwrap();
        assert_eq!(subtasks.len(), 3);
        assert!(subtasks.iter().all(|s| s.state == StageState::Completed));
    }

    #[tokio::test]
    async fn test_fanout_partial_failure() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("partial-fail", PartialFailFanOutStage)
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // Stage should be failed
        let status = store
            .get_status(&item.id, "partial-fail")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.state, StageState::Failed);

        // Check subtask statuses
        let subtasks = store
            .get_subtask_statuses(&item.id, "partial-fail")
            .await
            .unwrap();
        assert_eq!(subtasks.len(), 3);

        let failed: Vec<_> = subtasks
            .iter()
            .filter(|s| s.state == StageState::Failed)
            .collect();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].name, "fail");
    }

    #[tokio::test]
    async fn test_fanout_subtask_statuses_queryable() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("fanout", FanOutStage { subtask_counter: Arc::new(AtomicU32::new(0)) })
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let subtasks = store.get_subtask_statuses(&item.id, "fanout").await.unwrap();
        let names: Vec<_> = subtasks.iter().map(|s| s.name.as_str()).collect();

        assert!(names.contains(&"sub-1"));
        assert!(names.contains(&"sub-2"));
        assert!(names.contains(&"sub-3"));
    }

    #[tokio::test]
    async fn test_fanout_emits_events() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("fanout", FanOutStage { subtask_counter: Arc::new(AtomicU32::new(0)) })
            .build()
            .unwrap();

        let mut receiver = workflow.subscribe();
        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }

        // Should have: StageStarted, FanOutStarted, 3x (SubTaskStarted, SubTaskCompleted), StageCompleted
        let fanout_started: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::FanOutStarted { .. }))
            .collect();
        assert_eq!(fanout_started.len(), 1);

        let subtask_completed: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WorkflowEvent::SubTaskCompleted { .. }))
            .collect();
        assert_eq!(subtask_completed.len(), 3);
    }

    #[tokio::test]
    async fn test_fanout_with_dependent_stage() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("fanout", FanOutStage { subtask_counter: Arc::new(AtomicU32::new(0)) })
            .stage("next", CompletingStage::new("next"))
            .dependency("next", "fanout")
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // Both stages should complete
        let fanout_status = store.get_status(&item.id, "fanout").await.unwrap().unwrap();
        let next_status = store.get_status(&item.id, "next").await.unwrap().unwrap();

        assert_eq!(fanout_status.state, StageState::Completed);
        assert_eq!(next_status.state, StageState::Completed);
    }
}
```

### Verification Commands
```bash
cargo test executor::fanout_tests
cargo test executor
cargo clippy
```

---

## Milestone 4.4 — Advance Idempotency and Edge Cases

### Files to Modify
- `src/executor.rs` (add idempotency handling)

### Implementation Details

The core idempotency is already handled by:
1. `ready_stages` excludes completed/running/failed/awaiting_review stages
2. `advance` only executes ready stages

Add these additional helper methods and tests:

```rust
impl<W: WorkItem> Workflow<W> {
    /// Resets a failed stage to pending, allowing retry.
    ///
    /// This is useful for implementing retry logic outside the engine.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    /// * `stage` - The stage name to reset
    /// * `store` - The state store
    ///
    /// # Errors
    ///
    /// Returns an error if the stage doesn't exist or the current state
    /// is not `Failed`.
    pub async fn retry_stage<S: StateStore<W>>(
        &self,
        item_id: &W::Id,
        stage: &str,
        store: &S,
    ) -> Result<()> {
        if !self.has_stage(stage) {
            return Err(TreadleError::StageNotFound(stage.to_string()));
        }

        let current = store.get_status(item_id, stage).await?;
        match current {
            Some(status) if status.state == StageState::Failed => {
                store
                    .set_status(item_id, stage, StageStatus::pending())
                    .await?;
                Ok(())
            }
            Some(status) => Err(TreadleError::InvalidTransition {
                from: status.state,
                to: StageState::Pending,
            }),
            None => {
                // No status recorded, treat as already pending
                Ok(())
            }
        }
    }

    /// Approves a review, transitioning the stage to completed.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    /// * `stage` - The stage name to approve
    /// * `store` - The state store
    ///
    /// # Errors
    ///
    /// Returns an error if the stage is not in `AwaitingReview` state.
    pub async fn approve_review<S: StateStore<W>>(
        &self,
        item_id: &W::Id,
        stage: &str,
        store: &S,
    ) -> Result<()> {
        if !self.has_stage(stage) {
            return Err(TreadleError::StageNotFound(stage.to_string()));
        }

        let current = store.get_status(item_id, stage).await?;
        match current {
            Some(status) if status.state == StageState::AwaitingReview => {
                store
                    .set_status(item_id, stage, StageStatus::completed())
                    .await?;
                Ok(())
            }
            Some(status) => Err(TreadleError::InvalidTransition {
                from: status.state,
                to: StageState::Completed,
            }),
            None => Err(TreadleError::InvalidTransition {
                from: StageState::Pending,
                to: StageState::Completed,
            }),
        }
    }

    /// Rejects a review, transitioning the stage to failed.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    /// * `stage` - The stage name to reject
    /// * `reason` - The rejection reason
    /// * `store` - The state store
    pub async fn reject_review<S: StateStore<W>>(
        &self,
        item_id: &W::Id,
        stage: &str,
        reason: impl Into<String>,
        store: &S,
    ) -> Result<()> {
        if !self.has_stage(stage) {
            return Err(TreadleError::StageNotFound(stage.to_string()));
        }

        let current = store.get_status(item_id, stage).await?;
        match current {
            Some(status) if status.state == StageState::AwaitingReview => {
                store
                    .set_status(item_id, stage, StageStatus::failed(reason))
                    .await?;
                Ok(())
            }
            Some(status) => Err(TreadleError::InvalidTransition {
                from: status.state,
                to: StageState::Failed,
            }),
            None => Err(TreadleError::InvalidTransition {
                from: StageState::Pending,
                to: StageState::Failed,
            }),
        }
    }
}
```

#### 4.4.2 Edge Case Tests

```rust
mod edge_case_tests {
    use super::*;

    #[tokio::test]
    async fn test_retry_failed_stage() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", FailingStage { name: "a".to_string() })
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        // First advance fails
        workflow.advance(&item, &store).await.unwrap();
        assert_eq!(
            store.get_status(&item.id, "a").await.unwrap().unwrap().state,
            StageState::Failed
        );

        // Reset to pending
        workflow.retry_stage(&item.id, "a", &store).await.unwrap();
        assert_eq!(
            store.get_status(&item.id, "a").await.unwrap().unwrap().state,
            StageState::Pending
        );
    }

    #[tokio::test]
    async fn test_retry_non_failed_stage_errors() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::new("a"))
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        // Trying to retry a completed stage should error
        let result = workflow.retry_stage(&item.id, "a", &store).await;
        assert!(matches!(result, Err(TreadleError::InvalidTransition { .. })));
    }

    #[tokio::test]
    async fn test_approve_review() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("review", ReviewStage { name: "review".to_string() })
            .stage("next", CompletingStage::new("next"))
            .dependency("next", "review")
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        // Advance to review
        workflow.advance(&item, &store).await.unwrap();
        assert_eq!(
            store.get_status(&item.id, "review").await.unwrap().unwrap().state,
            StageState::AwaitingReview
        );

        // Approve
        workflow.approve_review(&item.id, "review", &store).await.unwrap();
        assert_eq!(
            store.get_status(&item.id, "review").await.unwrap().unwrap().state,
            StageState::Completed
        );

        // Advance to complete
        workflow.advance(&item, &store).await.unwrap();
        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_reject_review() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("review", ReviewStage { name: "review".to_string() })
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        workflow
            .reject_review(&item.id, "review", "Looks wrong", &store)
            .await
            .unwrap();

        let status = store.get_status(&item.id, "review").await.unwrap().unwrap();
        assert_eq!(status.state, StageState::Failed);
        assert_eq!(status.error, Some("Looks wrong".to_string()));
    }

    #[tokio::test]
    async fn test_advance_on_empty_workflow() {
        let workflow: Workflow<TestItem> = Workflow::builder().build().unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        // Should not error
        workflow.advance(&item, &store).await.unwrap();

        // Should be complete (vacuously true)
        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_multiple_times_is_idempotent() {
        let counter = Arc::new(AtomicU32::new(0));

        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::with_counter("a", counter.clone()))
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item = TestItem::new("item-1");

        // Multiple advances
        workflow.advance(&item, &store).await.unwrap();
        workflow.advance(&item, &store).await.unwrap();
        workflow.advance(&item, &store).await.unwrap();

        // Stage only executed once
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_advance_different_items_independent() {
        let counter = Arc::new(AtomicU32::new(0));

        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", CompletingStage::with_counter("a", counter.clone()))
            .build()
            .unwrap();

        let store = MemoryStateStore::new();
        let item1 = TestItem::new("item-1");
        let item2 = TestItem::new("item-2");

        workflow.advance(&item1, &store).await.unwrap();
        workflow.advance(&item2, &store).await.unwrap();

        // Stage executed twice (once per item)
        assert_eq!(counter.load(Ordering::SeqCst), 2);

        // Both complete
        assert!(workflow.is_complete(&item1.id, &store).await.unwrap());
        assert!(workflow.is_complete(&item2.id, &store).await.unwrap());
    }
}
```

### Verification Commands
```bash
cargo test executor::edge_case_tests
cargo test executor
cargo test
cargo clippy
```

---

## Phase 4 Completion Checklist

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes all tests
- [ ] `cargo clippy -- -D warnings` clean
- [ ] All public types have rustdoc comments

### Public API After Phase 4

```rust
// Previously from Phases 1-3
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
treadle::StateStore
treadle::MemoryStateStore
treadle::SqliteStateStore
treadle::Workflow
treadle::WorkflowBuilder

// New in Phase 4
treadle::WorkflowEvent

// New methods on Workflow
Workflow::advance()
Workflow::subscribe()
Workflow::retry_stage()
Workflow::approve_review()
Workflow::reject_review()
```

---

## Architecture After Phase 4

```
src/
├── lib.rs                  # Crate root, re-exports
├── error.rs                # TreadleError, Result
├── work_item.rs            # WorkItem trait
├── stage.rs                # Stage types and trait
├── state_store/
│   ├── mod.rs              # StateStore trait
│   ├── memory.rs           # MemoryStateStore
│   └── sqlite.rs           # SqliteStateStore
├── workflow.rs             # Workflow, WorkflowBuilder
├── event.rs                # WorkflowEvent
└── executor.rs             # advance(), fan-out execution (or in workflow.rs)
```
