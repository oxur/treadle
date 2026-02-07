# Treadle Phase 5 Implementation Plan

## Phase 5: Status, Visualization, and Polish

**Goal:** Pipeline introspection, documentation, examples, and release readiness.

**Prerequisites:**
- Phases 1-4 completed
- `cargo build` and `cargo test` pass

---

## Milestone 5.1 — Pipeline Status Helpers

### Files to Create/Modify
- `src/status.rs` (new)
- `src/lib.rs` (update exports)

### Implementation Details

#### 5.1.1 Create `src/status.rs`

```rust
//! Pipeline status reporting and visualization.
//!
//! This module provides [`PipelineStatus`] for inspecting the current
//! state of a workflow for a work item.

use std::fmt;

use crate::{ReviewData, StageState, StageStatus, SubTaskStatus};
use chrono::{DateTime, Utc};

/// Status entry for a single stage within a pipeline.
#[derive(Debug, Clone)]
pub struct StageStatusEntry {
    /// The stage name.
    pub name: String,
    /// The current state of this stage.
    pub state: StageState,
    /// When this status was last updated (if recorded).
    pub updated_at: Option<DateTime<Utc>>,
    /// Error message if the stage failed.
    pub error: Option<String>,
    /// Review data if the stage is awaiting review.
    pub review_data: Option<ReviewData>,
    /// Subtask statuses for fan-out stages.
    pub subtasks: Vec<SubTaskStatus>,
}

impl StageStatusEntry {
    /// Creates a new pending status entry.
    pub fn pending(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            state: StageState::Pending,
            updated_at: None,
            error: None,
            review_data: None,
            subtasks: Vec::new(),
        }
    }

    /// Creates a status entry from a stored status.
    pub fn from_status(name: impl Into<String>, status: StageStatus) -> Self {
        Self {
            name: name.into(),
            state: status.state,
            updated_at: Some(status.updated_at),
            error: status.error,
            review_data: status.review_data,
            subtasks: status.subtasks,
        }
    }

    /// Returns a status indicator character.
    pub fn status_char(&self) -> char {
        match self.state {
            StageState::Pending => '\u{23F3}',      // Hourglass
            StageState::Running => '\u{1F504}',     // Spinning arrows
            StageState::Completed => '\u{2705}',    // Green check
            StageState::Failed => '\u{274C}',       // Red X
            StageState::AwaitingReview => '\u{1F440}', // Eyes
            StageState::Skipped => '\u{23E9}',      // Fast forward
        }
    }

    /// Returns a status indicator for subtasks.
    pub fn subtask_status_char(state: StageState) -> char {
        match state {
            StageState::Pending => '\u{23F3}',
            StageState::Running => '\u{1F504}',
            StageState::Completed => '\u{2705}',
            StageState::Failed => '\u{274C}',
            StageState::AwaitingReview => '\u{1F440}',
            StageState::Skipped => '\u{23E9}',
        }
    }
}

/// The complete status of a workflow pipeline for a work item.
///
/// This provides a snapshot of where a work item is in the workflow,
/// including the status of all stages and their subtasks.
#[derive(Debug, Clone)]
pub struct PipelineStatus {
    /// The work item's identifier (stringified).
    pub item_id: String,
    /// Status of each stage in topological order.
    pub stages: Vec<StageStatusEntry>,
}

impl PipelineStatus {
    /// Creates a new pipeline status.
    pub fn new(item_id: impl Into<String>, stages: Vec<StageStatusEntry>) -> Self {
        Self {
            item_id: item_id.into(),
            stages,
        }
    }

    /// Returns true if all stages are completed or skipped.
    pub fn is_complete(&self) -> bool {
        self.stages.iter().all(|s| {
            matches!(s.state, StageState::Completed | StageState::Skipped)
        })
    }

    /// Returns true if any stage has failed.
    pub fn has_failures(&self) -> bool {
        self.stages.iter().any(|s| s.state == StageState::Failed)
    }

    /// Returns true if any stage is awaiting review.
    pub fn has_pending_reviews(&self) -> bool {
        self.stages.iter().any(|s| s.state == StageState::AwaitingReview)
    }

    /// Returns the names of stages that are currently running.
    pub fn running_stages(&self) -> Vec<&str> {
        self.stages
            .iter()
            .filter(|s| s.state == StageState::Running)
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Returns the names of stages that have failed.
    pub fn failed_stages(&self) -> Vec<&str> {
        self.stages
            .iter()
            .filter(|s| s.state == StageState::Failed)
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Returns the names of stages awaiting review.
    pub fn review_stages(&self) -> Vec<&str> {
        self.stages
            .iter()
            .filter(|s| s.state == StageState::AwaitingReview)
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Returns the overall progress as a percentage.
    pub fn progress_percent(&self) -> f32 {
        if self.stages.is_empty() {
            return 100.0;
        }

        let completed = self.stages.iter().filter(|s| {
            matches!(s.state, StageState::Completed | StageState::Skipped)
        }).count();

        (completed as f32 / self.stages.len() as f32) * 100.0
    }
}

impl fmt::Display for PipelineStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Pipeline status for item \"{}\":", self.item_id)?;
        writeln!(f)?;

        for stage in &self.stages {
            // Main stage line
            let status_char = stage.status_char();
            let state_str = format!("{:?}", stage.state);
            let time_str = stage
                .updated_at
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "-".to_string());

            write!(f, "  {} {:<15} {:<15} {}", status_char, stage.name, state_str, time_str)?;

            // Error message if present
            if let Some(ref error) = stage.error {
                write!(f, "  Error: {}", error)?;
            }

            writeln!(f)?;

            // Subtasks if present
            for subtask in &stage.subtasks {
                let sub_char = StageStatusEntry::subtask_status_char(subtask.state);
                let sub_state = format!("{:?}", subtask.state);

                write!(f, "     \u{2514}\u{2500} {} {:<12} {}", sub_char, subtask.name, sub_state)?;

                if let Some(ref error) = subtask.error {
                    write!(f, "  Error: {}", error)?;
                }

                writeln!(f)?;
            }
        }

        // Summary line
        writeln!(f)?;
        writeln!(f, "Progress: {:.0}%", self.progress_percent())?;

        if self.is_complete() {
            writeln!(f, "Status: Complete")?;
        } else if self.has_failures() {
            writeln!(f, "Status: Failed ({} stage(s))", self.failed_stages().len())?;
        } else if self.has_pending_reviews() {
            writeln!(f, "Status: Awaiting review ({} stage(s))", self.review_stages().len())?;
        } else {
            writeln!(f, "Status: In progress")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_status_empty() {
        let status = PipelineStatus::new("item-1", vec![]);
        assert!(status.is_complete());
        assert!(!status.has_failures());
        assert_eq!(status.progress_percent(), 100.0);
    }

    #[test]
    fn test_pipeline_status_all_pending() {
        let stages = vec![
            StageStatusEntry::pending("a"),
            StageStatusEntry::pending("b"),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(!status.is_complete());
        assert_eq!(status.progress_percent(), 0.0);
    }

    #[test]
    fn test_pipeline_status_partial_complete() {
        let stages = vec![
            StageStatusEntry::from_status("a", StageStatus::completed()),
            StageStatusEntry::pending("b"),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(!status.is_complete());
        assert!((status.progress_percent() - 33.33).abs() < 1.0);
    }

    #[test]
    fn test_pipeline_status_complete() {
        let stages = vec![
            StageStatusEntry::from_status("a", StageStatus::completed()),
            StageStatusEntry::from_status("b", StageStatus::skipped()),
            StageStatusEntry::from_status("c", StageStatus::completed()),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(status.is_complete());
        assert_eq!(status.progress_percent(), 100.0);
    }

    #[test]
    fn test_pipeline_status_with_failure() {
        let stages = vec![
            StageStatusEntry::from_status("a", StageStatus::completed()),
            StageStatusEntry::from_status("b", StageStatus::failed("oops")),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(!status.is_complete());
        assert!(status.has_failures());
        assert_eq!(status.failed_stages(), vec!["b"]);
    }

    #[test]
    fn test_pipeline_status_with_review() {
        let stages = vec![
            StageStatusEntry::from_status("a", StageStatus::completed()),
            StageStatusEntry::from_status(
                "b",
                StageStatus::awaiting_review(ReviewData::new("check")),
            ),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(status.has_pending_reviews());
        assert_eq!(status.review_stages(), vec!["b"]);
    }

    #[test]
    fn test_pipeline_status_display() {
        let stages = vec![
            StageStatusEntry::from_status("scan", StageStatus::completed()),
            StageStatusEntry::from_status("enrich", StageStatus::running()),
            StageStatusEntry::pending("review"),
        ];
        let status = PipelineStatus::new("doc-1", stages);

        let display = format!("{}", status);
        assert!(display.contains("doc-1"));
        assert!(display.contains("scan"));
        assert!(display.contains("enrich"));
        assert!(display.contains("review"));
        assert!(display.contains("In progress"));
    }

    #[test]
    fn test_stage_status_chars() {
        assert_eq!(StageStatusEntry::pending("a").status_char(), '\u{23F3}');

        let completed = StageStatusEntry::from_status("a", StageStatus::completed());
        assert_eq!(completed.status_char(), '\u{2705}');

        let failed = StageStatusEntry::from_status("a", StageStatus::failed("err"));
        assert_eq!(failed.status_char(), '\u{274C}');
    }
}
```

#### 5.1.2 Add `status()` Method to Workflow

Add to `src/workflow.rs`:

```rust
use crate::status::{PipelineStatus, StageStatusEntry};

impl<W: WorkItem> Workflow<W> {
    /// Returns the current status of the pipeline for a work item.
    ///
    /// This provides a complete snapshot of the workflow state, including
    /// all stage statuses and subtask details.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let status = workflow.status(&item.id(), &store).await?;
    /// println!("{}", status);
    ///
    /// if status.is_complete() {
    ///     println!("All done!");
    /// } else if status.has_pending_reviews() {
    ///     for stage in status.review_stages() {
    ///         println!("Stage {} needs review", stage);
    ///     }
    /// }
    /// ```
    pub async fn status<S: StateStore<W>>(
        &self,
        item_id: &W::Id,
        store: &S,
    ) -> Result<PipelineStatus> {
        let all_statuses = store.get_all_statuses(item_id).await?;
        let mut stages = Vec::with_capacity(self.topo_order.len());

        for stage_name in &self.topo_order {
            let entry = match all_statuses.get(stage_name) {
                Some(status) => {
                    let mut entry = StageStatusEntry::from_status(stage_name, status.clone());

                    // Load subtasks if any
                    let subtasks = store.get_subtask_statuses(item_id, stage_name).await?;
                    entry.subtasks = subtasks;

                    entry
                }
                None => StageStatusEntry::pending(stage_name),
            };
            stages.push(entry);
        }

        Ok(PipelineStatus::new(item_id.to_string(), stages))
    }
}
```

#### 5.1.3 Update `src/lib.rs`

```rust
mod error;
mod event;
mod stage;
mod state_store;
mod status;
mod work_item;
mod workflow;

// ... existing exports ...

pub use status::{PipelineStatus, StageStatusEntry};
```

### Verification Commands
```bash
cargo build
cargo test status
cargo clippy
```

---

## Milestone 5.2 — Tracing Integration

### Files to Modify
- `src/workflow.rs` / `src/executor.rs` (add tracing)
- `Cargo.toml` (already has tracing dependency)

### Implementation Details

#### 5.2.1 Add Tracing to Advance

Update the executor code with tracing spans:

```rust
use tracing::{debug, info, info_span, warn, Instrument};

impl<W: WorkItem> Workflow<W> {
    pub async fn advance<S: StateStore<W>>(&self, item: &W, store: &S) -> Result<()> {
        let span = info_span!("advance", item_id = %item.id());
        self.advance_internal(item, store, 0).instrument(span).await
    }

    async fn advance_internal<S: StateStore<W>>(
        &self,
        item: &W,
        store: &S,
        depth: usize,
    ) -> Result<()> {
        debug!(depth = depth, "checking for ready stages");

        let ready_stages = self.ready_stages(item.id(), store).await?;
        if ready_stages.is_empty() {
            debug!("no ready stages");
            if self.is_complete(item.id(), store).await? {
                info!("workflow complete");
                self.emit(WorkflowEvent::WorkflowCompleted {
                    item_id: item.id().to_string(),
                });
            }
            return Ok(());
        }

        debug!(ready = ?ready_stages, "found ready stages");

        // ... rest of implementation with additional tracing
    }

    async fn execute_stage<S: StateStore<W>>(
        &self,
        item: &W,
        stage_name: &str,
        store: &S,
        subtask_name: Option<&str>,
    ) -> Result<StageOutcome> {
        let span = info_span!(
            "stage",
            stage = %stage_name,
            subtask = subtask_name.unwrap_or("-")
        );

        async {
            info!("executing stage");

            // ... existing implementation ...

            match &result {
                Ok(outcome) => {
                    info!(outcome = ?outcome, "stage completed");
                }
                Err(e) => {
                    warn!(error = %e, "stage failed");
                }
            }

            result
        }
        .instrument(span)
        .await
    }

    async fn execute_fanout<S: StateStore<W>>(
        &self,
        item: &W,
        stage_name: &str,
        subtasks: Vec<crate::SubTask>,
        store: &S,
    ) -> Result<bool> {
        let span = info_span!(
            "fanout",
            stage = %stage_name,
            subtask_count = subtasks.len()
        );

        async {
            info!("starting fan-out execution");

            // ... existing implementation with debug/info calls ...

            for subtask in &subtasks {
                debug!(subtask = %subtask.name, "executing subtask");
                // ...
            }

            result
        }
        .instrument(span)
        .await
    }
}
```

**Note:** Full tracing integration is added throughout the executor code. The above shows the pattern - wrap async blocks in `.instrument(span)` and add `debug!`, `info!`, `warn!` calls at appropriate points.

### Verification Commands
```bash
cargo build
cargo test
cargo clippy
```

---

## Milestone 5.3 — Integration Test: Full Pipeline

### Files to Create
- `tests/integration.rs` (new)

### Implementation Details

#### 5.3.1 Create `tests/integration.rs`

```rust
//! Integration tests for the Treadle workflow engine.

use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use treadle::{
    MemoryStateStore, PipelineStatus, Result, ReviewData, Stage, StageContext,
    StageOutcome, StageState, StageStatus, StateStore, SubTask, TreadleError,
    WorkItem, Workflow, WorkflowEvent,
};

/// A document work item for testing.
#[derive(Debug, Clone)]
struct Document {
    id: String,
    content: String,
}

impl Document {
    fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
        }
    }
}

impl WorkItem for Document {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

/// Parse stage - always completes.
struct ParseStage;

#[async_trait]
impl Stage<Document> for ParseStage {
    fn name(&self) -> &str {
        "parse"
    }

    async fn execute(&self, doc: &Document, _ctx: &StageContext) -> Result<StageOutcome> {
        // Simulate parsing the document content
        if doc.content.is_empty() {
            Ok(StageOutcome::Skipped)
        } else {
            Ok(StageOutcome::Completed)
        }
    }
}

/// Enrich stage - fans out to multiple sources.
struct EnrichStage {
    call_count: Arc<AtomicU32>,
}

impl EnrichStage {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }

    fn with_counter(counter: Arc<AtomicU32>) -> Self {
        Self { call_count: counter }
    }
}

#[async_trait]
impl Stage<Document> for EnrichStage {
    fn name(&self) -> &str {
        "enrich"
    }

    async fn execute(&self, _doc: &Document, ctx: &StageContext) -> Result<StageOutcome> {
        if let Some(subtask) = &ctx.subtask_name {
            // Subtask execution
            self.call_count.fetch_add(1, Ordering::SeqCst);

            // Simulate one source failing
            if subtask == "source-3" && ctx.attempt == 1 {
                // On first attempt, source-3 succeeds (for happy path test)
            }

            Ok(StageOutcome::Completed)
        } else {
            // Initial invocation - declare subtasks
            Ok(StageOutcome::FanOut(vec![
                SubTask::new("source-1"),
                SubTask::new("source-2"),
                SubTask::new("source-3"),
            ]))
        }
    }
}

/// Review stage - requires human review.
struct ReviewStage;

#[async_trait]
impl Stage<Document> for ReviewStage {
    fn name(&self) -> &str {
        "review"
    }

    async fn execute(&self, doc: &Document, _ctx: &StageContext) -> Result<StageOutcome> {
        Ok(StageOutcome::AwaitingReview(ReviewData::with_details(
            format!("Please review document: {}", doc.id),
            serde_json::json!({
                "content_length": doc.content.len(),
                "preview": &doc.content[..doc.content.len().min(100)]
            }),
        )))
    }
}

/// Export stage - final stage.
struct ExportStage {
    exported: Arc<AtomicU32>,
}

impl ExportStage {
    fn new() -> Self {
        Self {
            exported: Arc::new(AtomicU32::new(0)),
        }
    }

    fn with_counter(counter: Arc<AtomicU32>) -> Self {
        Self { exported: counter }
    }
}

#[async_trait]
impl Stage<Document> for ExportStage {
    fn name(&self) -> &str {
        "export"
    }

    async fn execute(&self, _doc: &Document, _ctx: &StageContext) -> Result<StageOutcome> {
        self.exported.fetch_add(1, Ordering::SeqCst);
        Ok(StageOutcome::Completed)
    }
}

#[tokio::test]
async fn test_full_pipeline_with_memory_store() {
    let enrich_counter = Arc::new(AtomicU32::new(0));
    let export_counter = Arc::new(AtomicU32::new(0));

    // Build workflow
    let workflow: Workflow<Document> = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("enrich", EnrichStage::with_counter(enrich_counter.clone()))
        .stage("review", ReviewStage)
        .stage("export", ExportStage::with_counter(export_counter.clone()))
        .dependency("enrich", "parse")
        .dependency("review", "enrich")
        .dependency("export", "review")
        .build()
        .expect("workflow should build");

    // Subscribe to events
    let mut event_receiver = workflow.subscribe();
    let mut collected_events = Vec::new();

    // Create store and document
    let store = MemoryStateStore::new();
    let doc = Document::new("doc-1", "This is a test document with some content.");

    // First advance - should stop at review
    workflow.advance(&doc, &store).await.expect("advance should succeed");

    // Collect events so far
    while let Ok(event) = event_receiver.try_recv() {
        collected_events.push(event);
    }

    // Verify state
    let status = workflow.status(&doc.id, &store).await.expect("status should succeed");
    assert!(!status.is_complete());
    assert!(status.has_pending_reviews());
    assert_eq!(status.review_stages(), vec!["review"]);

    // Parse should be complete
    let parse_status = store.get_status(&doc.id, "parse").await.unwrap().unwrap();
    assert_eq!(parse_status.state, StageState::Completed);

    // Enrich should be complete (all subtasks done)
    let enrich_status = store.get_status(&doc.id, "enrich").await.unwrap().unwrap();
    assert_eq!(enrich_status.state, StageState::Completed);
    assert_eq!(enrich_counter.load(Ordering::SeqCst), 3); // 3 subtasks

    // Review should be awaiting
    let review_status = store.get_status(&doc.id, "review").await.unwrap().unwrap();
    assert_eq!(review_status.state, StageState::AwaitingReview);
    assert!(review_status.review_data.is_some());

    // Export should not have run
    let export_status = store.get_status(&doc.id, "export").await.unwrap();
    assert!(export_status.is_none());

    // Approve the review
    workflow
        .approve_review(&doc.id, "review", &store)
        .await
        .expect("approve should succeed");

    // Second advance - should complete
    workflow.advance(&doc, &store).await.expect("advance should succeed");

    // Collect remaining events
    while let Ok(event) = event_receiver.try_recv() {
        collected_events.push(event);
    }

    // Verify completion
    assert!(workflow.is_complete(&doc.id, &store).await.unwrap());
    assert_eq!(export_counter.load(Ordering::SeqCst), 1);

    // Verify final status
    let final_status = workflow.status(&doc.id, &store).await.unwrap();
    assert!(final_status.is_complete());
    assert_eq!(final_status.progress_percent(), 100.0);

    // Verify events
    let stage_completed_events: Vec<_> = collected_events
        .iter()
        .filter(|e| matches!(e, WorkflowEvent::StageCompleted { .. }))
        .collect();
    assert_eq!(stage_completed_events.len(), 4); // parse, enrich, review, export

    let workflow_completed = collected_events
        .iter()
        .any(|e| matches!(e, WorkflowEvent::WorkflowCompleted { .. }));
    assert!(workflow_completed);

    // Print status for visual verification
    println!("{}", final_status);
}

#[tokio::test]
#[cfg(feature = "sqlite")]
async fn test_full_pipeline_with_sqlite_store() {
    use treadle::SqliteStateStore;

    // Build workflow
    let workflow: Workflow<Document> = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("enrich", EnrichStage::new())
        .stage("review", ReviewStage)
        .stage("export", ExportStage::new())
        .dependency("enrich", "parse")
        .dependency("review", "enrich")
        .dependency("export", "review")
        .build()
        .expect("workflow should build");

    // Use in-memory SQLite
    let store = SqliteStateStore::open_in_memory()
        .await
        .expect("sqlite should open");

    let doc = Document::new("doc-sqlite-1", "SQLite test document");

    // Advance to review
    workflow.advance(&doc, &store).await.unwrap();

    // Verify blocked at review
    assert!(workflow.is_blocked(&doc.id, &store).await.unwrap());

    // Approve and complete
    workflow.approve_review(&doc.id, "review", &store).await.unwrap();
    workflow.advance(&doc, &store).await.unwrap();

    // Verify complete
    assert!(workflow.is_complete(&doc.id, &store).await.unwrap());
}

#[tokio::test]
async fn test_empty_document_skips_parse() {
    let workflow: Workflow<Document> = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("next", ExportStage::new())
        .dependency("next", "parse")
        .build()
        .unwrap();

    let store = MemoryStateStore::new();
    let doc = Document::new("empty-doc", ""); // Empty content

    workflow.advance(&doc, &store).await.unwrap();

    // Parse should be skipped
    let parse_status = store.get_status(&doc.id, "parse").await.unwrap().unwrap();
    assert_eq!(parse_status.state, StageState::Skipped);

    // Next should still run (skipped counts as satisfied)
    let next_status = store.get_status(&doc.id, "next").await.unwrap().unwrap();
    assert_eq!(next_status.state, StageState::Completed);
}

#[tokio::test]
async fn test_multiple_documents() {
    let workflow: Workflow<Document> = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("export", ExportStage::new())
        .dependency("export", "parse")
        .build()
        .unwrap();

    let store = MemoryStateStore::new();

    let docs = vec![
        Document::new("doc-1", "Content 1"),
        Document::new("doc-2", "Content 2"),
        Document::new("doc-3", "Content 3"),
    ];

    // Advance all documents
    for doc in &docs {
        workflow.advance(doc, &store).await.unwrap();
    }

    // All should be complete
    for doc in &docs {
        assert!(workflow.is_complete(&doc.id, &store).await.unwrap());
    }
}

#[tokio::test]
async fn test_status_display() {
    let workflow: Workflow<Document> = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("enrich", EnrichStage::new())
        .stage("review", ReviewStage)
        .stage("export", ExportStage::new())
        .dependency("enrich", "parse")
        .dependency("review", "enrich")
        .dependency("export", "review")
        .build()
        .unwrap();

    let store = MemoryStateStore::new();
    let doc = Document::new("status-test", "Test content");

    workflow.advance(&doc, &store).await.unwrap();

    let status = workflow.status(&doc.id, &store).await.unwrap();
    let display = format!("{}", status);

    // Verify display contains expected elements
    assert!(display.contains("status-test"));
    assert!(display.contains("parse"));
    assert!(display.contains("enrich"));
    assert!(display.contains("review"));
    assert!(display.contains("export"));
    assert!(display.contains("Awaiting review"));

    println!("{}", display);
}
```

### Verification Commands
```bash
cargo test --test integration
cargo test --test integration --features sqlite
cargo test
```

---

## Milestone 5.4 — Documentation and Examples

### Files to Create/Modify
- `src/lib.rs` (expand crate docs)
- `examples/basic_pipeline.rs` (new)

### Implementation Details

#### 5.4.1 Expand Crate Documentation

Update `src/lib.rs` with comprehensive documentation:

```rust
//! # Treadle
//!
//! A persistent, resumable, human-in-the-loop workflow engine backed by a
//! [petgraph](https://docs.rs/petgraph) DAG.
//!
//! Treadle fills the gap between single-shot DAG executors (like
//! [dagrs](https://crates.io/crates/dagrs)) and heavyweight distributed
//! workflow engines (like [Restate](https://restate.dev) or
//! [Temporal](https://temporal.io)). It is designed for **local,
//! single-process pipelines** where:
//!
//! - Work items progress through a DAG of stages over time
//! - Each item's state is tracked persistently (survives restarts)
//! - Stages can pause for human review and resume later
//! - Fan-out stages (e.g., enriching from multiple sources) track each
//!   subtask independently with per-subtask retry
//! - The full pipeline is inspectable at any moment
//!
//! ## Quick Start
//!
//! Define your work item and stages:
//!
//! ```rust,ignore
//! use treadle::{Stage, StageOutcome, StageContext, Result, WorkItem};
//! use async_trait::async_trait;
//!
//! // Your work item
//! #[derive(Debug, Clone)]
//! struct Document { id: String }
//!
//! impl WorkItem for Document {
//!     type Id = String;
//!     fn id(&self) -> &Self::Id { &self.id }
//! }
//!
//! // A processing stage
//! struct ParseStage;
//!
//! #[async_trait]
//! impl Stage<Document> for ParseStage {
//!     fn name(&self) -> &str { "parse" }
//!
//!     async fn execute(&self, doc: &Document, ctx: &StageContext) -> Result<StageOutcome> {
//!         println!("Parsing document: {}", doc.id);
//!         Ok(StageOutcome::Completed)
//!     }
//! }
//! ```
//!
//! Build and run the workflow:
//!
//! ```rust,ignore
//! use treadle::{Workflow, MemoryStateStore};
//!
//! # async fn example() -> treadle::Result<()> {
//! let workflow = Workflow::builder()
//!     .stage("parse", ParseStage)
//!     .stage("process", ProcessStage)
//!     .dependency("process", "parse")
//!     .build()?;
//!
//! let store = MemoryStateStore::new();
//! let doc = Document { id: "doc-1".into() };
//!
//! workflow.advance(&doc, &store).await?;
//!
//! if workflow.is_complete(&doc.id, &store).await? {
//!     println!("Done!");
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Human-in-the-Loop
//!
//! Stages can pause for human review:
//!
//! ```rust,ignore
//! async fn execute(&self, _doc: &Document, _ctx: &StageContext) -> Result<StageOutcome> {
//!     Ok(StageOutcome::AwaitingReview(ReviewData::new("Please verify entities")))
//! }
//! ```
//!
//! Later, approve or reject:
//!
//! ```rust,ignore
//! workflow.approve_review(&doc.id, "review", &store).await?;
//! workflow.advance(&doc, &store).await?;  // Continues to next stage
//! ```
//!
//! ## Fan-Out Execution
//!
//! Stages can spawn parallel subtasks:
//!
//! ```rust,ignore
//! async fn execute(&self, _doc: &Document, ctx: &StageContext) -> Result<StageOutcome> {
//!     if ctx.is_subtask() {
//!         // Handle individual subtask
//!         Ok(StageOutcome::Completed)
//!     } else {
//!         // Declare subtasks
//!         Ok(StageOutcome::FanOut(vec![
//!             SubTask::new("source-a"),
//!             SubTask::new("source-b"),
//!         ]))
//!     }
//! }
//! ```
//!
//! ## State Persistence
//!
//! Use [`MemoryStateStore`] for testing or [`SqliteStateStore`] for production:
//!
//! ```rust,ignore
//! // In-memory (testing)
//! let store = MemoryStateStore::new();
//!
//! // SQLite (production)
//! let store = SqliteStateStore::open("workflow.db").await?;
//! ```
//!
//! ## Event Observation
//!
//! Subscribe to workflow events for logging or building UIs:
//!
//! ```rust,ignore
//! let mut events = workflow.subscribe();
//!
//! tokio::spawn(async move {
//!     while let Ok(event) = events.recv().await {
//!         match event {
//!             WorkflowEvent::StageCompleted { item_id, stage } => {
//!                 println!("Stage {} completed for {}", stage, item_id);
//!             }
//!             _ => {}
//!         }
//!     }
//! });
//! ```
//!
//! ## Feature Flags
//!
//! - `sqlite` (default): Enables [`SqliteStateStore`] for persistent storage
//!
//! ## Design Philosophy
//!
//! The name comes from the **treadle** — the foot-operated lever that drives
//! a loom, spinning wheel, or lathe. The machine has stages and mechanisms,
//! but without the human pressing the treadle, nothing moves. This captures
//! the core design: a pipeline engine where human judgment gates the flow.

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

// ... module declarations and exports ...
```

#### 5.4.2 Create Example

Create `examples/basic_pipeline.rs`:

```rust
//! Basic Treadle pipeline example.
//!
//! This example demonstrates:
//! - Building a workflow with stages and dependencies
//! - Advancing work items through the pipeline
//! - Handling review stages
//! - Checking pipeline status
//!
//! Run with: `cargo run --example basic_pipeline`

use async_trait::async_trait;
use treadle::{
    MemoryStateStore, Result, ReviewData, Stage, StageContext, StageOutcome,
    StageState, Workflow, WorkItem,
};

/// Our work item - a document to process.
#[derive(Debug, Clone)]
struct Document {
    id: String,
    content: String,
}

impl Document {
    fn new(id: &str, content: &str) -> Self {
        Self {
            id: id.to_string(),
            content: content.to_string(),
        }
    }
}

impl WorkItem for Document {
    type Id = String;

    fn id(&self) -> &Self::Id {
        &self.id
    }
}

/// Stage 1: Scan the document for structure.
struct ScanStage;

#[async_trait]
impl Stage<Document> for ScanStage {
    fn name(&self) -> &str {
        "scan"
    }

    async fn execute(&self, doc: &Document, _ctx: &StageContext) -> Result<StageOutcome> {
        println!("  Scanning document '{}' ({} chars)", doc.id, doc.content.len());
        // Simulate some processing
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        Ok(StageOutcome::Completed)
    }
}

/// Stage 2: Extract entities from the document.
struct ExtractStage;

#[async_trait]
impl Stage<Document> for ExtractStage {
    fn name(&self) -> &str {
        "extract"
    }

    async fn execute(&self, doc: &Document, _ctx: &StageContext) -> Result<StageOutcome> {
        println!("  Extracting entities from '{}'", doc.id);
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        Ok(StageOutcome::Completed)
    }
}

/// Stage 3: Human review of extracted entities.
struct ReviewStage;

#[async_trait]
impl Stage<Document> for ReviewStage {
    fn name(&self) -> &str {
        "review"
    }

    async fn execute(&self, doc: &Document, _ctx: &StageContext) -> Result<StageOutcome> {
        println!("  Review requested for '{}'", doc.id);
        Ok(StageOutcome::AwaitingReview(ReviewData::with_details(
            format!("Please review entities for document '{}'", doc.id),
            serde_json::json!({
                "doc_id": doc.id,
                "extracted_entities": ["Person: John Doe", "Org: Acme Inc"]
            }),
        )))
    }
}

/// Stage 4: Export the finalized document.
struct ExportStage;

#[async_trait]
impl Stage<Document> for ExportStage {
    fn name(&self) -> &str {
        "export"
    }

    async fn execute(&self, doc: &Document, _ctx: &StageContext) -> Result<StageOutcome> {
        println!("  Exporting document '{}'", doc.id);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Ok(StageOutcome::Completed)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("=== Treadle Basic Pipeline Example ===\n");

    // Build the workflow
    let workflow: Workflow<Document> = Workflow::builder()
        .stage("scan", ScanStage)
        .stage("extract", ExtractStage)
        .stage("review", ReviewStage)
        .stage("export", ExportStage)
        .dependency("extract", "scan")
        .dependency("review", "extract")
        .dependency("export", "review")
        .build()?;

    println!("Workflow stages: {:?}\n", workflow.stages());

    // Create state store (in-memory for this example)
    let store = MemoryStateStore::new();

    // Create a document to process
    let doc = Document::new(
        "doc-001",
        "John Doe works at Acme Inc. He manages the engineering team.",
    );

    println!("Processing document: {}\n", doc.id);

    // First advance - will stop at review
    println!("--- First Advance ---");
    workflow.advance(&doc, &store).await?;

    // Check status
    let status = workflow.status(&doc.id, &store).await?;
    println!("\nStatus after first advance:");
    println!("{}", status);

    // Simulate human approval
    println!("\n--- Human approves the review ---\n");
    workflow.approve_review(&doc.id, "review", &store).await?;

    // Second advance - will complete
    println!("--- Second Advance ---");
    workflow.advance(&doc, &store).await?;

    // Final status
    let final_status = workflow.status(&doc.id, &store).await?;
    println!("\nFinal status:");
    println!("{}", final_status);

    if workflow.is_complete(&doc.id, &store).await? {
        println!("Document processing complete!");
    }

    Ok(())
}
```

### Verification Commands
```bash
cargo test --doc
cargo run --example basic_pipeline
cargo doc --no-deps --open
```

---

## Milestone 5.5 — Release Checklist

### Files to Create/Modify
- `CHANGELOG.md` (new)
- Verify `Cargo.toml` metadata
- Verify license files exist

### Implementation Details

#### 5.5.1 Create CHANGELOG.md

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-XX-XX

### Added

- Initial release of Treadle workflow engine
- Core types: `WorkItem`, `Stage`, `StageOutcome`, `StageContext`
- State persistence: `StateStore` trait with `MemoryStateStore` and `SqliteStateStore`
- Workflow construction: `Workflow::builder()` with DAG validation
- Execution engine: `workflow.advance()` with automatic progression
- Human-in-the-loop: `AwaitingReview` outcome with `approve_review()`/`reject_review()`
- Fan-out execution: `FanOut` outcome with subtask tracking
- Event system: `WorkflowEvent` broadcast for observability
- Status reporting: `PipelineStatus` with `Display` implementation
- Tracing integration for debugging
- Comprehensive documentation and examples

### Features

- `sqlite` (default): Enables SQLite-backed state persistence
```

#### 5.5.2 Verification Commands

```bash
# Clippy with all warnings as errors
cargo clippy -- -D warnings

# All tests with all features
cargo test --all-features

# Tests without default features
cargo test --no-default-features

# Documentation builds
cargo doc --no-deps

# Verify Cargo.toml completeness
cargo metadata --format-version 1 | jq '.packages[] | select(.name == "treadle") | {name, version, description, license, repository, keywords, categories}'

# Dry-run publish
cargo publish --dry-run

# Verify example runs
cargo run --example basic_pipeline
```

#### 5.5.3 Final Cargo.toml Verification

Ensure these fields are present and correct:

```toml
[package]
name = "treadle"
version = "0.1.0"
edition = "2021"
rust-version = "1.75"
description = "A persistent, resumable, human-in-the-loop workflow engine backed by a petgraph DAG"
license = "MIT OR Apache-2.0"
repository = "https://github.com/oxur/treadle"
documentation = "https://docs.rs/treadle"
readme = "README.md"
keywords = ["pipeline", "workflow", "dag", "async", "resumable"]
categories = ["asynchronous", "data-structures"]
```

---

## Phase 5 Completion Checklist

- [ ] `cargo clippy -- -D warnings` passes
- [ ] `cargo test --all-features` passes
- [ ] `cargo test --no-default-features` passes
- [ ] `cargo doc --no-deps` builds cleanly
- [ ] `cargo run --example basic_pipeline` works
- [ ] `cargo publish --dry-run` succeeds
- [ ] CHANGELOG.md exists with 0.1.0 entry
- [ ] LICENSE-MIT and LICENSE-APACHE exist
- [ ] All public types have rustdoc comments
- [ ] Crate-level documentation is comprehensive

### Final Public API

```rust
// Error handling
treadle::TreadleError
treadle::Result<T>

// Work items
treadle::WorkItem

// Stage types and traits
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
treadle::MemoryStateStore
treadle::SqliteStateStore      // (feature = "sqlite")

// Workflow
treadle::Workflow
treadle::WorkflowBuilder
treadle::WorkflowEvent

// Status reporting
treadle::PipelineStatus
treadle::StageStatusEntry
```

---

## Final Architecture

```
src/
├── lib.rs                  # Crate root, comprehensive docs, re-exports
├── error.rs                # TreadleError, Result
├── work_item.rs            # WorkItem trait
├── stage.rs                # Stage types and trait
├── state_store/
│   ├── mod.rs              # StateStore trait
│   ├── memory.rs           # MemoryStateStore
│   └── sqlite.rs           # SqliteStateStore
├── workflow.rs             # Workflow, WorkflowBuilder
├── event.rs                # WorkflowEvent
├── executor.rs             # advance(), fan-out (or in workflow.rs)
└── status.rs               # PipelineStatus, StageStatusEntry

tests/
└── integration.rs          # Full end-to-end tests

examples/
└── basic_pipeline.rs       # Runnable example

CHANGELOG.md
LICENSE-MIT
LICENSE-APACHE
```
