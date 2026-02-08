//! Integration tests for the Treadle workflow engine.
//!
//! These tests exercise the full pipeline with all features:
//! - Stage execution and dependencies
//! - Human-in-the-loop review stages
//! - Fan-out execution with subtasks
//! - Event streaming
//! - Status reporting
//! - State persistence (Memory and SQLite)

use async_trait::async_trait;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use treadle::{
    MemoryStateStore, Result, Stage, StageContext,
    StageOutcome, StageStatus, StateStore, SubTask,
    WorkItem, Workflow, WorkflowEvent,
};

/// A document work item for testing.
#[derive(Debug, Clone)]
#[allow(dead_code)]
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
    fn id(&self) -> &str {
        &self.id
    }
}

/// Parse stage - always completes unless content is empty.
#[derive(Debug)]
struct ParseStage;

#[async_trait]
impl Stage for ParseStage {
    fn name(&self) -> &str {
        "parse"
    }

    async fn execute(&self, _item: &dyn WorkItem, _ctx: &mut StageContext) -> Result<StageOutcome> {
        // Downcast to Document (in real code you'd use a proper pattern)
        // For this test, we assume item is a Document
        Ok(StageOutcome::Complete)
    }
}

/// Enrich stage - fans out to multiple sources.
#[derive(Debug)]
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
impl Stage for EnrichStage {
    fn name(&self) -> &str {
        "enrich"
    }

    async fn execute(&self, _item: &dyn WorkItem, ctx: &mut StageContext) -> Result<StageOutcome> {
        if ctx.subtask_name.is_some() {
            // Subtask execution
            self.call_count.fetch_add(1, Ordering::SeqCst);
            Ok(StageOutcome::Complete)
        } else {
            // Initial invocation - declare subtasks
            Ok(StageOutcome::FanOut(vec![
                SubTask::new("source-1".to_string()),
                SubTask::new("source-2".to_string()),
                SubTask::new("source-3".to_string()),
            ]))
        }
    }
}

/// Review stage - requires human review.
#[derive(Debug)]
struct ReviewStage;

#[async_trait]
impl Stage for ReviewStage {
    fn name(&self) -> &str {
        "review"
    }

    async fn execute(&self, _item: &dyn WorkItem, _ctx: &mut StageContext) -> Result<StageOutcome> {
        // Simply return NeedsReview - the workflow will create the ReviewData
        Ok(StageOutcome::NeedsReview)
    }
}

/// Export stage - final stage.
#[derive(Debug)]
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
impl Stage for ExportStage {
    fn name(&self) -> &str {
        "export"
    }

    async fn execute(&self, _item: &dyn WorkItem, _ctx: &mut StageContext) -> Result<StageOutcome> {
        self.exported.fetch_add(1, Ordering::SeqCst);
        Ok(StageOutcome::Complete)
    }
}

#[tokio::test]
async fn test_full_pipeline_with_memory_store() {
    let enrich_counter = Arc::new(AtomicU32::new(0));
    let export_counter = Arc::new(AtomicU32::new(0));

    // Build workflow
    let workflow = Workflow::builder()
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
    let mut store = MemoryStateStore::new();
    let doc = Document::new("doc-1", "This is a test document with some content.");

    // First advance - should stop at review
    workflow.advance(&doc, &mut store).await.expect("advance should succeed");

    // Collect events so far
    while let Ok(event) = event_receiver.try_recv() {
        collected_events.push(event);
    }

    // Verify state
    let status = workflow.status(doc.id(), &store).await.expect("status should succeed");
    assert!(!status.is_complete());
    assert!(status.has_pending_reviews());
    assert_eq!(status.review_stages(), vec!["review"]);

    // Parse should be complete
    let parse_state = store.get_stage_state(doc.id(), "parse").await.unwrap().unwrap();
    assert!(matches!(parse_state.status, StageStatus::Complete));

    // Enrich should be complete (all subtasks done)
    let enrich_state = store.get_stage_state(doc.id(), "enrich").await.unwrap().unwrap();
    assert!(matches!(enrich_state.status, StageStatus::Complete));
    assert_eq!(enrich_counter.load(Ordering::SeqCst), 3); // 3 subtasks

    // Review should be paused
    let review_state = store.get_stage_state(doc.id(), "review").await.unwrap().unwrap();
    assert!(matches!(review_state.status, StageStatus::Paused));

    // Export should not have run
    let export_state = store.get_stage_state(doc.id(), "export").await.unwrap();
    assert!(export_state.is_none());

    // Approve the review
    workflow
        .approve_review(doc.id(), "review", &mut store)
        .await
        .expect("approve should succeed");

    // Second advance - should complete
    workflow.advance(&doc, &mut store).await.expect("advance should succeed");

    // Collect remaining events
    while let Ok(event) = event_receiver.try_recv() {
        collected_events.push(event);
    }

    // Verify completion
    assert!(workflow.is_complete(doc.id(), &store).await.unwrap());
    assert_eq!(export_counter.load(Ordering::SeqCst), 1);

    // Verify final status
    let final_status = workflow.status(doc.id(), &store).await.unwrap();
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
    let workflow = Workflow::builder()
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
    let mut store = SqliteStateStore::open_in_memory()
        .await
        .expect("sqlite should open");

    let doc = Document::new("doc-sqlite-1", "SQLite test document");

    // Advance to review
    workflow.advance(&doc, &mut store).await.unwrap();

    // Verify blocked at review
    assert!(workflow.is_blocked(doc.id(), &store).await.unwrap());

    // Approve and complete
    workflow.approve_review(doc.id(), "review", &mut store).await.unwrap();
    workflow.advance(&doc, &mut store).await.unwrap();

    // Verify complete
    assert!(workflow.is_complete(doc.id(), &store).await.unwrap());
}

#[tokio::test]
async fn test_simple_pipeline() {
    // Simpler test for basic execution
    let workflow = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("export", ExportStage::new())
        .dependency("export", "parse")
        .build()
        .unwrap();

    let mut store = MemoryStateStore::new();
    let doc = Document::new("simple-doc", "Simple content");

    workflow.advance(&doc, &mut store).await.unwrap();

    // Both stages should complete
    assert!(workflow.is_complete(doc.id(), &store).await.unwrap());
}

#[tokio::test]
async fn test_multiple_documents() {
    let export_counter = Arc::new(AtomicU32::new(0));

    let workflow = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("export", ExportStage::with_counter(export_counter.clone()))
        .dependency("export", "parse")
        .build()
        .unwrap();

    let mut store = MemoryStateStore::new();

    let docs = vec![
        Document::new("doc-1", "Content 1"),
        Document::new("doc-2", "Content 2"),
        Document::new("doc-3", "Content 3"),
    ];

    // Advance all documents
    for doc in &docs {
        workflow.advance(doc, &mut store).await.unwrap();
    }

    // All should be complete
    for doc in &docs {
        assert!(workflow.is_complete(doc.id(), &store).await.unwrap());
    }

    // Export should have run 3 times
    assert_eq!(export_counter.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn test_status_display() {
    let workflow = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("enrich", EnrichStage::new())
        .stage("review", ReviewStage)
        .stage("export", ExportStage::new())
        .dependency("enrich", "parse")
        .dependency("review", "enrich")
        .dependency("export", "review")
        .build()
        .unwrap();

    let mut store = MemoryStateStore::new();
    let doc = Document::new("status-test", "Test content");

    workflow.advance(&doc, &mut store).await.unwrap();

    let status = workflow.status(doc.id(), &store).await.unwrap();
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

#[tokio::test]
async fn test_event_streaming() {
    let workflow = Workflow::builder()
        .stage("stage1", ParseStage)
        .stage("stage2", ExportStage::new())
        .dependency("stage2", "stage1")
        .build()
        .unwrap();

    let mut events = workflow.subscribe();
    let mut store = MemoryStateStore::new();
    let doc = Document::new("event-test", "Content");

    workflow.advance(&doc, &mut store).await.unwrap();

    // Should receive StageStarted, StageCompleted events
    let mut event_types = Vec::new();
    while let Ok(event) = events.try_recv() {
        match event {
            WorkflowEvent::StageStarted { .. } => event_types.push("started"),
            WorkflowEvent::StageCompleted { .. } => event_types.push("completed"),
            WorkflowEvent::WorkflowCompleted { .. } => event_types.push("workflow_complete"),
            _ => {}
        }
    }

    assert!(event_types.contains(&"started"));
    assert!(event_types.contains(&"completed"));
    assert!(event_types.contains(&"workflow_complete"));
}

#[tokio::test]
async fn test_fanout_with_subtask_tracking() {
    let enrich_counter = Arc::new(AtomicU32::new(0));

    let workflow = Workflow::builder()
        .stage("enrich", EnrichStage::with_counter(enrich_counter.clone()))
        .build()
        .unwrap();

    let mut store = MemoryStateStore::new();
    let doc = Document::new("fanout-test", "Content");

    workflow.advance(&doc, &mut store).await.unwrap();

    // Enrich should be complete with 3 subtasks
    let state = store.get_stage_state(doc.id(), "enrich").await.unwrap().unwrap();
    assert!(matches!(state.status, StageStatus::Complete));
    assert_eq!(state.subtasks.len(), 3);
    assert_eq!(enrich_counter.load(Ordering::SeqCst), 3);

    // All subtasks should be complete
    for subtask in &state.subtasks {
        assert!(matches!(subtask.status, StageStatus::Complete));
    }
}

#[tokio::test]
async fn test_pipeline_progress_tracking() {
    let workflow = Workflow::builder()
        .stage("s1", ParseStage)
        .stage("s2", ParseStage)
        .stage("s3", ParseStage)
        .stage("s4", ParseStage)
        .dependency("s2", "s1")
        .dependency("s3", "s2")
        .dependency("s4", "s3")
        .build()
        .unwrap();

    let mut store = MemoryStateStore::new();
    let doc = Document::new("progress-test", "Content");

    // Check progress at start
    let status = workflow.status(doc.id(), &store).await.unwrap();
    assert_eq!(status.progress_percent(), 0.0);

    // Advance through completion
    workflow.advance(&doc, &mut store).await.unwrap();

    // Check final progress
    let final_status = workflow.status(doc.id(), &store).await.unwrap();
    assert_eq!(final_status.progress_percent(), 100.0);
    assert!(final_status.is_complete());
}
