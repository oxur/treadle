# Treadle Phase 11 Implementation Plan

## Phase 11: Integration Tests and Documentation

**Goal:** Validate the full v2 feature set through end-to-end integration tests that exercise mixed v1/v2 pipelines, review outcome propagation, and attempt timeouts. Update crate-level documentation and provide a complete, runnable example demonstrating the Claude Code document processing use case.

**Prerequisites:**
- Phases 1--10 completed
- `Artefact` trait, `StageOutput`, revised `Stage::execute` (Phase 6)
- `QualityGate` trait, `QualityVerdict`, `QualityFeedback`, `QualityContext` (Phase 7)
- `RetryBudget`, `ReviewPolicy`, `ReviewOutcome`, `resolve_review` (Phase 8)
- Retry loop in `advance()`, `AttemptRecord`, `attempt_timeout` (Phase 9)
- Quality gate evaluation wired into retry loop, events (Phase 10)
- `cargo build` and `cargo test` pass

---

## Milestone 11.1 --- Mixed Pipeline Integration Test

### Files to Create
- `tests/mixed_pipeline.rs` (new integration test)

### Implementation Details

#### 11.1.1 Create `tests/mixed_pipeline.rs`

This test constructs a four-stage linear pipeline mixing v1-style stages (no quality gate, no retry budget) with v2-style stages (quality gate, retry budget, review policy). It verifies that the engine handles both styles transparently within the same workflow.

```rust
//! Integration test: pipeline mixing v1-style stages (no gates) and
//! v2-style stages (with quality gates, retry budgets, and review policies).
//!
//! Pipeline:
//!   scan (v1) --> process (v2, gate + retry) --> review (v2, ReviewPolicy::Always)
//!                                                         --> export (v1)
//!
//! The `process` stage is rejected by its quality gate on the first two
//! attempts and accepted on the third. The `review` stage always blocks
//! for human review. After `resolve_review(Approve)`, the `export` stage
//! completes as a plain v1 stage.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use treadle::{
    Artefact, AttemptRecord, CriterionResult, ExhaustedAction, MemoryStateStore, QualityContext,
    QualityFeedback, QualityGate, QualityVerdict, Result, RetryBudget, ReviewOutcome,
    ReviewPolicy, Stage, StageContext, StageOutcome, StageOutput, StateStore, WorkItem, Workflow,
    WorkflowEvent,
};

// ---------------------------------------------------------------------------
// Test work item
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Document {
    id: String,
    title: String,
}

impl WorkItem for Document {
    fn id(&self) -> &str {
        &self.id
    }
}

// ---------------------------------------------------------------------------
// v1-style stages (no artefacts, no quality gate)
// ---------------------------------------------------------------------------

/// A simple v1-style stage that always completes.
#[derive(Debug)]
struct ScanStage;

#[async_trait]
impl Stage for ScanStage {
    fn name(&self) -> &str {
        "scan"
    }

    async fn execute(
        &self,
        _item: &dyn WorkItem,
        _ctx: &StageContext,
    ) -> Result<StageOutput> {
        // v1-style: return a bare outcome converted via From impl.
        Ok(StageOutcome::Complete.into())
    }
}

/// A simple v1-style export stage.
#[derive(Debug)]
struct ExportStage;

#[async_trait]
impl Stage for ExportStage {
    fn name(&self) -> &str {
        "export"
    }

    async fn execute(
        &self,
        _item: &dyn WorkItem,
        _ctx: &StageContext,
    ) -> Result<StageOutput> {
        Ok(StageOutcome::Complete.into())
    }
}

// ---------------------------------------------------------------------------
// v2-style stage: produces artefacts, tracks attempt number
// ---------------------------------------------------------------------------

/// A processing stage that produces a JSON artefact.
/// The artefact content changes with each attempt so the quality gate
/// can decide when to accept.
#[derive(Debug)]
struct ProcessStage {
    /// Tracks how many times execute() has been called (across all items).
    call_count: Arc<AtomicU32>,
}

impl ProcessStage {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }
}

#[async_trait]
impl Stage for ProcessStage {
    fn name(&self) -> &str {
        "process"
    }

    async fn execute(
        &self,
        _item: &dyn WorkItem,
        ctx: &StageContext,
    ) -> Result<StageOutput> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        // On the first two calls the artefact has quality_score < 80.
        // On the third call the score is 95 (good enough).
        let quality_score: u32 = match n {
            1 => 30,
            2 => 55,
            _ => 95,
        };

        let artefact = serde_json::json!({
            "quality_score": quality_score,
            "attempt": ctx.stage_state.retry_count + 1,
        });

        Ok(StageOutput::completed_with_summary(
            artefact,
            format!("Processed with score {quality_score}"),
        ))
    }
}

// ---------------------------------------------------------------------------
// Quality gate for the process stage
// ---------------------------------------------------------------------------

/// Accepts only when `quality_score >= 80`.
#[derive(Debug)]
struct ProcessQualityGate;

#[async_trait]
impl QualityGate for ProcessQualityGate {
    async fn evaluate(
        &self,
        _item: &dyn WorkItem,
        _stage: &str,
        output: &StageOutput,
        _ctx: &QualityContext,
    ) -> Result<QualityVerdict> {
        let score = output
            .artefacts
            .as_ref()
            .and_then(|a| a.to_json())
            .and_then(|j| j.get("quality_score").and_then(|v| v.as_u64()))
            .unwrap_or(0);

        if score >= 80 {
            Ok(QualityVerdict::Accepted)
        } else {
            Ok(QualityVerdict::Rejected {
                feedback: QualityFeedback {
                    summary: format!("Quality score {score} is below threshold 80"),
                    failed_criteria: vec![CriterionResult {
                        name: "quality_score".into(),
                        expected: ">= 80".into(),
                        actual: format!("{score}"),
                        passed: false,
                    }],
                    guidance: Some(serde_json::json!({
                        "hint": "Increase extraction fidelity"
                    })),
                },
            })
        }
    }
}

// ---------------------------------------------------------------------------
// A review stage that does nothing itself (review policy handles blocking)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ReviewStage;

#[async_trait]
impl Stage for ReviewStage {
    fn name(&self) -> &str {
        "review"
    }

    async fn execute(
        &self,
        _item: &dyn WorkItem,
        _ctx: &StageContext,
    ) -> Result<StageOutput> {
        Ok(StageOutcome::Complete.into())
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_mixed_pipeline_v1_and_v2_stages() {
    // Build the workflow
    let workflow = Workflow::builder()
        .stage("scan", ScanStage)
        .stage("process", ProcessStage::new())
        .stage("review", ReviewStage)
        .stage("export", ExportStage)
        .dependency("process", "scan")
        .dependency("review", "process")
        .dependency("export", "review")
        // v2 configuration for the process stage
        .quality_gate("process", ProcessQualityGate)
        .retry_budget(
            "process",
            RetryBudget {
                max_attempts: 3,
                delay: None,
                attempt_timeout: None,
                on_exhausted: ExhaustedAction::Escalate,
            },
        )
        // review always blocks for human review
        .review_policy("review", ReviewPolicy::Always)
        .build()
        .expect("workflow should build");

    // Subscribe to events before advancing
    let mut event_rx = workflow.subscribe();

    let mut store = MemoryStateStore::new();
    let doc = Document {
        id: "doc-1".into(),
        title: "Test Document".into(),
    };

    // --- First advance: should run scan, process (3 attempts), review (blocks) ---
    workflow.advance(&doc, &mut store).await.unwrap();

    // Verify scan completed (v1-style, no gate)
    let scan_state = store.get_stage_state("doc-1", "scan").await.unwrap().unwrap();
    assert_eq!(
        scan_state.status,
        treadle::StageStatus::Complete,
        "scan should complete as a v1 stage"
    );

    // Verify process completed after retries (accepted on attempt 3)
    let process_state = store.get_stage_state("doc-1", "process").await.unwrap().unwrap();
    // process should be Complete (quality gate accepted on attempt 3) or
    // Paused if ReviewPolicy applies. Since process has no review policy
    // (default Never), it should be Complete.
    assert_eq!(
        process_state.status,
        treadle::StageStatus::Complete,
        "process should complete after quality gate accepts on attempt 3"
    );

    // Verify attempt history for process: 3 attempts recorded
    let process_attempts = store.get_attempts("doc-1", "process").await.unwrap();
    assert_eq!(
        process_attempts.len(),
        3,
        "process should have 3 attempt records"
    );
    assert_eq!(process_attempts[0].attempt, 1);
    assert_eq!(process_attempts[1].attempt, 2);
    assert_eq!(process_attempts[2].attempt, 3);

    // Verify review is blocked (ReviewPolicy::Always)
    let review_state = store.get_stage_state("doc-1", "review").await.unwrap().unwrap();
    assert_eq!(
        review_state.status,
        treadle::StageStatus::Paused,
        "review should be paused awaiting human review"
    );

    // Export should not have run yet (blocked behind review)
    let export_state = store.get_stage_state("doc-1", "export").await;
    assert!(
        export_state.unwrap().is_none()
            || export_state
                .as_ref()
                .unwrap()
                .as_ref()
                .map(|s| s.status == treadle::StageStatus::Pending)
                .unwrap_or(true),
        "export should not have started yet"
    );

    // --- Resolve the review ---
    workflow
        .resolve_review("doc-1", "review", ReviewOutcome::Approve, &mut store)
        .await
        .unwrap();

    // --- Second advance: should run export ---
    workflow.advance(&doc, &mut store).await.unwrap();

    let export_state = store.get_stage_state("doc-1", "export").await.unwrap().unwrap();
    assert_eq!(
        export_state.status,
        treadle::StageStatus::Complete,
        "export should complete after review is approved"
    );

    // Verify workflow is complete
    assert!(
        workflow.is_complete("doc-1", &store).await.unwrap(),
        "workflow should be complete"
    );

    // --- Verify events were emitted in the expected order ---
    let mut events = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        events.push(event);
    }

    // We expect at a minimum:
    //   StageStarted(scan), StageCompleted(scan),
    //   RetryAttempt/RetryScheduled for process attempts,
    //   QualityCheckFailed (x2), QualityCheckPassed (x1),
    //   StageCompleted(process),
    //   StageStarted(review), ReviewRequired(review),
    //   StageStarted(export), StageCompleted(export),
    //   WorkflowCompleted
    let stage_names: Vec<Option<&str>> = events.iter().map(|e| e.stage()).collect();

    // scan must appear before process
    let first_scan = stage_names.iter().position(|s| *s == Some("scan"));
    let first_process = stage_names.iter().position(|s| *s == Some("process"));
    assert!(
        first_scan.unwrap() < first_process.unwrap(),
        "scan events should precede process events"
    );

    // process must appear before review
    let first_review = stage_names.iter().position(|s| *s == Some("review"));
    assert!(
        first_process.unwrap() < first_review.unwrap(),
        "process events should precede review events"
    );

    // Verify we got retry-related events for process
    let retry_events: Vec<&WorkflowEvent> = events
        .iter()
        .filter(|e| {
            matches!(
                e,
                WorkflowEvent::RetryScheduled { stage, .. }
                | WorkflowEvent::RetryAttempt { stage, .. }
                    if stage == "process"
            )
        })
        .collect();
    assert!(
        !retry_events.is_empty(),
        "should have retry events for the process stage"
    );

    // Verify quality check events
    let quality_failed: Vec<&WorkflowEvent> = events
        .iter()
        .filter(|e| matches!(e, WorkflowEvent::QualityCheckFailed { stage, .. } if stage == "process"))
        .collect();
    assert_eq!(
        quality_failed.len(),
        2,
        "process should have 2 quality check failures (attempts 1 and 2)"
    );

    let quality_passed: Vec<&WorkflowEvent> = events
        .iter()
        .filter(|e| matches!(e, WorkflowEvent::QualityCheckPassed { stage, .. } if stage == "process"))
        .collect();
    assert_eq!(
        quality_passed.len(),
        1,
        "process should have 1 quality check pass (attempt 3)"
    );
}
```

### Design Decisions

- **v1 stages use `StageOutcome::Complete.into()`** to demonstrate backward compatibility via `From<StageOutcome> for StageOutput`.
- **The process stage uses an `AtomicU32` counter** rather than relying on `StageContext.attempt` to independently verify how many times the engine invoked the stage.
- **Event ordering assertions** use relative position checks (scan before process, process before review) rather than exact indices, making the test resilient to additional events being added in future phases.
- **The quality gate inspects `artefact.to_json()`** rather than downcasting, demonstrating the JSON-based artefact inspection path.

### Tests

This file is itself a test. Verification is via `cargo test --test mixed_pipeline`.

### Verification Commands

```bash
cargo test --test mixed_pipeline
cargo clippy --tests
```

---

## Milestone 11.2 --- ApproveWithEdits Integration Test

### Files to Create
- `tests/approve_with_edits.rs` (new integration test)

### Implementation Details

#### 11.2.1 Create `tests/approve_with_edits.rs`

This test verifies that when a human reviewer calls `resolve_review` with `ReviewOutcome::ApproveWithEdits`, the edited artefacts replace the original in the state store and are visible to downstream stages via `upstream_artefacts`.

```rust
//! Integration test: `ApproveWithEdits` review outcome flowing through
//! the pipeline.
//!
//! Pipeline:
//!   produce --> downstream
//!
//! The `produce` stage generates artefacts and has `ReviewPolicy::Always`.
//! A human reviewer calls `resolve_review` with `ApproveWithEdits`,
//! supplying modified artefacts. The `downstream` stage verifies it
//! receives the *edited* artefacts, not the originals.

use std::any::Any;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use treadle::{
    Artefact, MemoryStateStore, QualityContext, QualityFeedback, QualityGate, QualityVerdict,
    Result, ReviewOutcome, ReviewPolicy, Stage, StageContext, StageOutcome, StageOutput,
    StateStore, WorkItem, Workflow,
};

// ---------------------------------------------------------------------------
// Test work item
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Task {
    id: String,
}

impl WorkItem for Task {
    fn id(&self) -> &str {
        &self.id
    }
}

// ---------------------------------------------------------------------------
// A custom artefact type for typed inspection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TextArtefact {
    content: String,
    revision: u32,
}

impl Artefact for TextArtefact {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "content": self.content,
            "revision": self.revision,
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ---------------------------------------------------------------------------
// Stages
// ---------------------------------------------------------------------------

/// Produces the original artefact (revision 1).
#[derive(Debug)]
struct ProduceStage;

#[async_trait]
impl Stage for ProduceStage {
    fn name(&self) -> &str {
        "produce"
    }

    async fn execute(
        &self,
        _item: &dyn WorkItem,
        _ctx: &StageContext,
    ) -> Result<StageOutput> {
        let artefact = TextArtefact {
            content: "Original draft content".into(),
            revision: 1,
        };
        Ok(StageOutput::completed_with_summary(
            artefact,
            "Produced original draft",
        ))
    }
}

/// Downstream stage that records the upstream artefact it received
/// so the test can inspect it.
#[derive(Debug)]
struct DownstreamStage {
    /// Stores the upstream artefact JSON received during execution.
    received_artefact: Arc<Mutex<Option<serde_json::Value>>>,
}

impl DownstreamStage {
    fn new() -> (Self, Arc<Mutex<Option<serde_json::Value>>>) {
        let received = Arc::new(Mutex::new(None));
        (
            Self {
                received_artefact: Arc::clone(&received),
            },
            received,
        )
    }
}

#[async_trait]
impl Stage for DownstreamStage {
    fn name(&self) -> &str {
        "downstream"
    }

    async fn execute(
        &self,
        _item: &dyn WorkItem,
        ctx: &StageContext,
    ) -> Result<StageOutput> {
        // Capture the upstream artefact from the produce stage.
        if let Some(artefact) = ctx.upstream_artefacts.get("produce") {
            let json = artefact.to_json();
            *self.received_artefact.lock().unwrap() = json;
        }
        Ok(StageOutcome::Complete.into())
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_approve_with_edits_replaces_artefacts_for_downstream() {
    let (downstream_stage, received_artefact) = DownstreamStage::new();

    let workflow = Workflow::builder()
        .stage("produce", ProduceStage)
        .stage("downstream", downstream_stage)
        .dependency("downstream", "produce")
        .review_policy("produce", ReviewPolicy::Always)
        .build()
        .expect("workflow should build");

    let mut store = MemoryStateStore::new();
    let task = Task {
        id: "task-1".into(),
    };

    // First advance: produce runs and blocks for review.
    workflow.advance(&task, &mut store).await.unwrap();

    let produce_state = store
        .get_stage_state("task-1", "produce")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        produce_state.status,
        treadle::StageStatus::Paused,
        "produce should be awaiting review"
    );

    // Downstream should not have run yet.
    let ds_state = store.get_stage_state("task-1", "downstream").await.unwrap();
    assert!(
        ds_state.is_none()
            || ds_state
                .as_ref()
                .map(|s| s.status == treadle::StageStatus::Pending)
                .unwrap_or(true),
        "downstream should not have executed yet"
    );

    // Human reviewer approves with edits: updated content, revision 2.
    let edited = TextArtefact {
        content: "Revised and improved content".into(),
        revision: 2,
    };

    workflow
        .resolve_review(
            "task-1",
            "produce",
            ReviewOutcome::ApproveWithEdits {
                edited_artefacts: Box::new(edited),
                note: Some("Fixed grammar and added missing section".into()),
            },
            &mut store,
        )
        .await
        .unwrap();

    // Verify the artefact summary in the store was updated to the edited version.
    let stored_summary = store
        .get_artefact_summary("task-1", "produce")
        .await
        .unwrap();
    assert!(stored_summary.is_some(), "artefact summary should exist");
    let summary = stored_summary.unwrap();
    assert_eq!(
        summary["revision"], 2,
        "stored artefact should be the edited version (revision 2)"
    );
    assert_eq!(
        summary["content"], "Revised and improved content",
        "stored artefact should contain the edited content"
    );

    // Second advance: downstream should run and receive the edited artefacts.
    workflow.advance(&task, &mut store).await.unwrap();

    let ds_state = store
        .get_stage_state("task-1", "downstream")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ds_state.status,
        treadle::StageStatus::Complete,
        "downstream should have completed"
    );

    // Verify downstream received the EDITED artefact, not the original.
    let received = received_artefact.lock().unwrap();
    assert!(received.is_some(), "downstream should have received an artefact");
    let received_json = received.as_ref().unwrap();
    assert_eq!(
        received_json["revision"], 2,
        "downstream should receive the edited artefact (revision 2), not the original"
    );
    assert_eq!(
        received_json["content"], "Revised and improved content",
        "downstream should receive the edited content"
    );

    // Workflow should be complete.
    assert!(
        workflow.is_complete("task-1", &store).await.unwrap(),
        "workflow should be complete"
    );
}

#[tokio::test]
async fn test_approve_without_edits_preserves_original_artefacts() {
    let (downstream_stage, received_artefact) = DownstreamStage::new();

    let workflow = Workflow::builder()
        .stage("produce", ProduceStage)
        .stage("downstream", downstream_stage)
        .dependency("downstream", "produce")
        .review_policy("produce", ReviewPolicy::Always)
        .build()
        .expect("workflow should build");

    let mut store = MemoryStateStore::new();
    let task = Task {
        id: "task-2".into(),
    };

    // First advance: produce runs and blocks for review.
    workflow.advance(&task, &mut store).await.unwrap();

    // Approve without edits.
    workflow
        .resolve_review("task-2", "produce", ReviewOutcome::Approve, &mut store)
        .await
        .unwrap();

    // Second advance: downstream runs.
    workflow.advance(&task, &mut store).await.unwrap();

    // Verify downstream received the ORIGINAL artefact (revision 1).
    let received = received_artefact.lock().unwrap();
    assert!(received.is_some(), "downstream should have received an artefact");
    let received_json = received.as_ref().unwrap();
    assert_eq!(
        received_json["revision"], 1,
        "downstream should receive the original artefact (revision 1)"
    );
}
```

### Design Decisions

- **Two tests in one file:** `test_approve_with_edits_replaces_artefacts_for_downstream` validates the happy path for `ApproveWithEdits`; `test_approve_without_edits_preserves_original_artefacts` is a control test confirming the original artefact flows through on a plain `Approve`.
- **The `DownstreamStage` captures upstream artefacts** via a shared `Arc<Mutex<...>>` so the test can assert on what the stage received at execution time.
- **`TextArtefact` implements both `to_json()` and `as_any()`** to demonstrate both JSON-based and typed inspection paths.

### Tests

This file is itself a test. Verification is via `cargo test --test approve_with_edits`.

### Verification Commands

```bash
cargo test --test approve_with_edits
cargo clippy --tests
```

---

## Milestone 11.3 --- Attempt Timeout Integration Test

### Files to Create
- `tests/attempt_timeout.rs` (new integration test)

### Implementation Details

#### 11.3.1 Create `tests/attempt_timeout.rs`

This test verifies that `attempt_timeout` correctly terminates a slow stage, records the timeout in the attempt history, and allows a subsequent fast attempt to succeed.

```rust
//! Integration test: `attempt_timeout` triggering on a slow stage.
//!
//! A stage that takes 5 seconds on attempt 1 but succeeds instantly
//! on attempt 2. With `attempt_timeout: 100ms` and `max_attempts: 3`,
//! attempt 1 should time out and attempt 2 should succeed.
//!
//! Uses `start_paused = true` for deterministic timing.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use treadle::{
    ExhaustedAction, MemoryStateStore, Result, RetryBudget, Stage, StageContext, StageOutcome,
    StageOutput, StateStore, WorkItem, Workflow, WorkflowEvent,
};

// ---------------------------------------------------------------------------
// Test work item
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Job {
    id: String,
}

impl WorkItem for Job {
    fn id(&self) -> &str {
        &self.id
    }
}

// ---------------------------------------------------------------------------
// A stage that is slow on attempt 1, fast on subsequent attempts
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ConditionallySlowStage {
    /// Counts how many times execute() has been called.
    call_count: Arc<AtomicU32>,
}

impl ConditionallySlowStage {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }

    fn call_count(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.call_count)
    }
}

#[async_trait]
impl Stage for ConditionallySlowStage {
    fn name(&self) -> &str {
        "slow_then_fast"
    }

    async fn execute(
        &self,
        _item: &dyn WorkItem,
        _ctx: &StageContext,
    ) -> Result<StageOutput> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;

        if n == 1 {
            // First attempt: sleep for 5 seconds (will be timed out).
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        // Subsequent attempts: return immediately.
        Ok(StageOutput::completed_with_summary(
            serde_json::json!({"attempt": n}),
            format!("Completed on call {n}"),
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_attempt_timeout_triggers_then_succeeds() {
    let stage = ConditionallySlowStage::new();
    let call_counter = stage.call_count();

    let workflow = Workflow::builder()
        .stage("slow_then_fast", stage)
        .retry_budget(
            "slow_then_fast",
            RetryBudget {
                max_attempts: 3,
                delay: None,
                attempt_timeout: Some(Duration::from_millis(100)),
                on_exhausted: ExhaustedAction::Fail,
            },
        )
        .build()
        .expect("workflow should build");

    let mut event_rx = workflow.subscribe();
    let mut store = MemoryStateStore::new();
    let job = Job {
        id: "job-1".into(),
    };

    workflow.advance(&job, &mut store).await.unwrap();

    // The stage should have been called twice:
    //   attempt 1: timed out after 100ms
    //   attempt 2: succeeded instantly
    assert_eq!(
        call_counter.load(Ordering::SeqCst),
        2,
        "stage should have been called exactly twice"
    );

    // Stage should be complete.
    let state = store
        .get_stage_state("job-1", "slow_then_fast")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.status,
        treadle::StageStatus::Complete,
        "stage should be complete after succeeding on attempt 2"
    );

    // Verify attempt history.
    let attempts = store
        .get_attempts("job-1", "slow_then_fast")
        .await
        .unwrap();
    assert_eq!(attempts.len(), 2, "should have 2 attempt records");

    // Attempt 1 should record the timeout.
    assert_eq!(attempts[0].attempt, 1);
    assert!(
        attempts[0]
            .output_summary
            .as_ref()
            .map(|s| s.contains("timed out"))
            .unwrap_or(false),
        "attempt 1 summary should mention timeout: got {:?}",
        attempts[0].output_summary
    );

    // Attempt 2 should record success.
    assert_eq!(attempts[1].attempt, 2);

    // Verify events.
    let mut events = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        events.push(event);
    }

    // Should have a RetryScheduled event for attempt 2.
    let retry_scheduled = events.iter().any(|e| {
        matches!(
            e,
            WorkflowEvent::RetryScheduled {
                stage,
                attempt: 2,
                ..
            } if stage == "slow_then_fast"
        )
    });
    assert!(
        retry_scheduled,
        "should have a RetryScheduled event for attempt 2"
    );

    // Should have a StageCompleted event.
    let stage_completed = events.iter().any(|e| {
        matches!(
            e,
            WorkflowEvent::StageCompleted { stage, .. }
                if stage == "slow_then_fast"
        )
    });
    assert!(stage_completed, "should have a StageCompleted event");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_attempt_timeout_exhaustion_fails() {
    /// A stage that always takes too long.
    #[derive(Debug)]
    struct AlwaysSlowStage;

    #[async_trait]
    impl Stage for AlwaysSlowStage {
        fn name(&self) -> &str {
            "always_slow"
        }

        async fn execute(
            &self,
            _item: &dyn WorkItem,
            _ctx: &StageContext,
        ) -> Result<StageOutput> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(StageOutcome::Complete.into())
        }
    }

    let workflow = Workflow::builder()
        .stage("always_slow", AlwaysSlowStage)
        .retry_budget(
            "always_slow",
            RetryBudget {
                max_attempts: 2,
                delay: None,
                attempt_timeout: Some(Duration::from_millis(50)),
                on_exhausted: ExhaustedAction::Fail,
            },
        )
        .build()
        .expect("workflow should build");

    let mut store = MemoryStateStore::new();
    let job = Job {
        id: "job-2".into(),
    };

    workflow.advance(&job, &mut store).await.unwrap();

    // Stage should have failed (all attempts timed out, ExhaustedAction::Fail).
    let state = store
        .get_stage_state("job-2", "always_slow")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.status,
        treadle::StageStatus::Failed,
        "stage should be failed after exhausting retry budget"
    );

    // Should have 2 attempt records, both timed out.
    let attempts = store.get_attempts("job-2", "always_slow").await.unwrap();
    assert_eq!(attempts.len(), 2, "should have 2 attempt records");

    for (i, attempt) in attempts.iter().enumerate() {
        assert!(
            attempt
                .output_summary
                .as_ref()
                .map(|s| s.contains("timed out"))
                .unwrap_or(false),
            "attempt {} should record a timeout: got {:?}",
            i + 1,
            attempt.output_summary
        );
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn test_attempt_timeout_exhaustion_escalates() {
    /// A stage that always takes too long.
    #[derive(Debug)]
    struct AlwaysSlowStage;

    #[async_trait]
    impl Stage for AlwaysSlowStage {
        fn name(&self) -> &str {
            "always_slow"
        }

        async fn execute(
            &self,
            _item: &dyn WorkItem,
            _ctx: &StageContext,
        ) -> Result<StageOutput> {
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(StageOutcome::Complete.into())
        }
    }

    let workflow = Workflow::builder()
        .stage("always_slow", AlwaysSlowStage)
        .retry_budget(
            "always_slow",
            RetryBudget {
                max_attempts: 2,
                delay: None,
                attempt_timeout: Some(Duration::from_millis(50)),
                on_exhausted: ExhaustedAction::Escalate,
            },
        )
        .build()
        .expect("workflow should build");

    let mut event_rx = workflow.subscribe();
    let mut store = MemoryStateStore::new();
    let job = Job {
        id: "job-3".into(),
    };

    workflow.advance(&job, &mut store).await.unwrap();

    // Stage should be awaiting review (escalated).
    let state = store
        .get_stage_state("job-3", "always_slow")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.status,
        treadle::StageStatus::Paused,
        "stage should be paused (escalated to review) after exhausting timeout budget"
    );

    // Verify escalation event was emitted.
    let mut events = Vec::new();
    while let Ok(event) = event_rx.try_recv() {
        events.push(event);
    }

    let escalated = events.iter().any(|e| {
        matches!(
            e,
            WorkflowEvent::Escalated { stage, .. }
                if stage == "always_slow"
        )
    });
    assert!(
        escalated,
        "should have an Escalated event for always_slow"
    );
}
```

### Design Decisions

- **`start_paused = true`** gives deterministic time control. Tokio's time auto-advances when all tasks are waiting, so the 5-second sleep and 100ms timeout resolve instantly in wall-clock time. This prevents slow tests.
- **Three tests cover three scenarios:** timeout-then-succeed, exhaustion-with-fail, exhaustion-with-escalate.
- **The `AtomicU32` call counter** independently verifies how many times the stage was invoked, guarding against off-by-one errors in the retry loop.
- **Timeout detection in attempt history** is verified by checking the `output_summary` field, which the engine populates with "timed out" (as specified in Phase 9).

### Tests

This file is itself a test. Verification is via `cargo test --test attempt_timeout`.

### Verification Commands

```bash
cargo test --test attempt_timeout
cargo clippy --tests
```

---

## Milestone 11.4 --- Crate Documentation and Example

### Files to Modify
- `src/lib.rs` (update crate-level docs)

### Files to Create
- `examples/quality_pipeline.rs` (new example)

### Implementation Details

#### 11.4.1 Update `src/lib.rs` Crate-Level Docs

Replace the existing crate-level documentation with an expanded version that covers v2 concepts.

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
//! - Fan-out stages track each subtask independently with per-subtask retry
//! - The full pipeline is inspectable at any moment
//!
//! ## v2: Work, Judgement, and Policy
//!
//! Treadle v2 separates three concerns that v1 conflated:
//!
//! | Concern | v1 | v2 |
//! |---------|----|----|
//! | **Do the work** | `Stage::execute` returns `StageOutcome` | `Stage::execute` returns [`StageOutput`] (outcome + artefacts) |
//! | **Judge quality** | Implicit in the outcome choice | [`QualityGate::evaluate`] returns [`QualityVerdict`] |
//! | **Decide next action** | Hardcoded (complete/fail/review) | [`ReviewPolicy`] configuration per stage |
//!
//! This separation makes it natural to build iterative refinement pipelines
//! where an AI agent executes a stage, a quality gate evaluates the output,
//! and the engine automatically retries with structured feedback if quality
//! criteria are not met.
//!
//! ## Key v2 Types
//!
//! - [`Artefact`] -- trait for stage output; supports opt-in JSON serialisation
//!   and type-safe downcasting
//! - [`StageOutput`] -- wraps [`StageOutcome`] with optional artefacts and summary
//! - [`QualityGate`] -- trait for evaluating stage output quality
//! - [`QualityVerdict`] -- accepted, rejected (with feedback), or uncertain
//! - [`RetryBudget`] -- controls retry count, delay, timeout, and exhaustion behaviour
//! - [`ReviewPolicy`] -- when human review is required (never, always, on escalation, etc.)
//! - [`ReviewOutcome`] -- approve, reject, or approve with edits
//! - [`AttemptRecord`] -- history of each execution attempt for a stage
//!
//! ## Quick Example (v1 style)
//!
//! ```rust,ignore
//! use treadle::{Workflow, Stage, StageOutcome, StageOutput, StageContext};
//!
//! // Define your stages by implementing the Stage trait.
//! // v1-style stages return StageOutcome converted via From impl.
//! struct Scan;
//! struct Enrich;
//! struct Review;
//!
//! // Build the workflow DAG
//! let workflow = Workflow::builder()
//!     .stage("scan", Scan)
//!     .stage("enrich", Enrich)
//!     .stage("review", Review)
//!     .dependency("enrich", "scan")
//!     .dependency("review", "enrich")
//!     .build()?;
//!
//! // Advance a work item through the pipeline
//! workflow.advance(&my_item, &mut state_store).await?;
//! ```
//!
//! ## Quick Example (v2 style with quality gates)
//!
//! ```rust,ignore
//! use treadle::*;
//!
//! let workflow = Workflow::builder()
//!     .stage("convert", PdfToMarkdown::new())
//!     .stage("extract", ConceptExtractor::new())
//!     .dependency("extract", "convert")
//!     // Attach quality gates
//!     .quality_gate("convert", MarkdownQualityCheck::new())
//!     .quality_gate("extract", ConceptGraphValidator::new())
//!     // Configure retry budgets
//!     .retry_budget("convert", RetryBudget {
//!         max_attempts: 3,
//!         delay: None,
//!         attempt_timeout: Some(std::time::Duration::from_secs(120)),
//!         on_exhausted: ExhaustedAction::Escalate,
//!     })
//!     // Configure review policies
//!     .review_policy("extract", ReviewPolicy::OnEscalation)
//!     .build()?;
//!
//! // The engine handles retry loops, quality evaluation, and escalation.
//! workflow.advance(&document, &mut store).await?;
//! ```
//!
//! See [`examples/quality_pipeline.rs`](https://github.com/oxur/treadle/blob/main/examples/quality_pipeline.rs)
//! for a complete, runnable example demonstrating the Claude Code document
//! processing use case.
//!
//! ## Design Philosophy
//!
//! The name comes from the **treadle** -- the foot-operated lever that drives
//! a loom, spinning wheel, or lathe. The machine has stages and mechanisms,
//! but without the human pressing the treadle, nothing moves. This captures
//! the core design: a pipeline engine where human judgement gates the flow.
```

#### 11.4.2 Create `examples/quality_pipeline.rs`

```rust
//! # Quality Pipeline Example
//!
//! Demonstrates Treadle v2's quality gate, retry budget, and review policy
//! features using a simplified version of the Claude Code document processing
//! use case.
//!
//! ## Pipeline
//!
//! ```text
//!                   +--> concept_extractor --+
//! pdf_to_markdown --+                        +--> final_review
//!                   +--> embedding_gen -------+
//! ```
//!
//! - `pdf_to_markdown`: converts a (stub) PDF to markdown, checked by a
//!   quality gate that requires tables and a minimum word count. Has a retry
//!   budget of 3 attempts with a 2-second timeout per attempt. Escalates to
//!   human review if quality is never met.
//!
//! - `concept_extractor`: extracts concepts from the markdown output, checked
//!   by a gate that requires at least 3 concepts. Has a retry budget of 2.
//!
//! - `embedding_gen`: generates embeddings (stub). No quality gate (v1-style).
//!
//! - `final_review`: always blocks for human review (`ReviewPolicy::Always`).
//!
//! ## Running
//!
//! ```bash
//! cargo run --example quality_pipeline
//! ```

use std::any::Any;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use treadle::{
    Artefact, CriterionResult, ExhaustedAction, MemoryStateStore, QualityContext,
    QualityFeedback, QualityGate, QualityVerdict, Result, RetryBudget, ReviewOutcome,
    ReviewPolicy, Stage, StageContext, StageOutcome, StageOutput, StateStore, WorkItem, Workflow,
    WorkflowEvent,
};

// ===========================================================================
// Work item
// ===========================================================================

/// A document to be processed through the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Document {
    id: String,
    title: String,
    /// In a real application this would be a file path or blob reference.
    /// Here we use a stub string.
    pdf_content: String,
}

impl WorkItem for Document {
    fn id(&self) -> &str {
        &self.id
    }
}

// ===========================================================================
// Artefact types
// ===========================================================================

/// Markdown output from PDF conversion.
#[derive(Debug, Clone)]
struct MarkdownOutput {
    text: String,
    table_count: usize,
    word_count: usize,
}

impl Artefact for MarkdownOutput {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "text_length": self.text.len(),
            "table_count": self.table_count,
            "word_count": self.word_count,
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Concepts extracted from a document.
#[derive(Debug, Clone)]
struct ConceptGraph {
    concepts: Vec<String>,
    relationships: Vec<(String, String)>,
}

impl Artefact for ConceptGraph {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "concept_count": self.concepts.len(),
            "relationship_count": self.relationships.len(),
            "concepts": self.concepts,
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ===========================================================================
// Stage: PDF to Markdown
// ===========================================================================

/// Simulates PDF-to-markdown conversion.
///
/// On early attempts the output is low quality (no tables, few words).
/// On later attempts (simulating adaptation to feedback) the output improves.
#[derive(Debug)]
struct PdfToMarkdown {
    call_count: Arc<AtomicU32>,
}

impl PdfToMarkdown {
    fn new() -> Self {
        Self {
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }
}

#[async_trait]
impl Stage for PdfToMarkdown {
    fn name(&self) -> &str {
        "pdf_to_markdown"
    }

    async fn execute(
        &self,
        item: &dyn WorkItem,
        ctx: &StageContext,
    ) -> Result<StageOutput> {
        let n = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
        println!(
            "  [pdf_to_markdown] Attempt {} for item '{}'",
            n,
            item.id()
        );

        // If we have feedback from a previous attempt, acknowledge it.
        if let Some(ref feedback) = ctx.feedback {
            println!("  [pdf_to_markdown] Received feedback: {feedback}");
            println!("  [pdf_to_markdown] Adapting extraction strategy ...");
        }

        // Simulate improving quality across attempts.
        let (text, table_count) = match n {
            1 => {
                // First attempt: poor quality, no tables.
                ("Short text without tables.".to_string(), 0)
            }
            2 => {
                // Second attempt: better, but still no tables.
                let text = "This is a longer document about quantum computing. \
                    It discusses qubits, superposition, entanglement, \
                    and quantum error correction. The field has seen \
                    significant advances in recent years."
                    .to_string();
                (text, 0)
            }
            _ => {
                // Third attempt: full quality with tables.
                let text = "# Quantum Computing Overview\n\n\
                    This document provides a comprehensive overview of quantum computing.\n\n\
                    ## Key Concepts\n\n\
                    Quantum computing leverages quantum mechanical phenomena including \
                    superposition, entanglement, and interference to perform computations.\n\n\
                    ## Performance Comparison\n\n\
                    | Algorithm | Classical | Quantum |\n\
                    |-----------|-----------|----------|\n\
                    | Factoring | Exponential | Polynomial |\n\
                    | Search | O(N) | O(sqrt(N)) |\n\n\
                    ## Applications\n\n\
                    Quantum computers show promise in cryptography, drug discovery, \
                    materials science, and optimisation problems."
                    .to_string();
                (text, 1)
            }
        };

        let word_count = text.split_whitespace().count();
        let artefact = MarkdownOutput {
            text,
            table_count,
            word_count,
        };

        Ok(StageOutput::completed_with_summary(
            artefact,
            format!("{word_count} words, {table_count} tables"),
        ))
    }
}

// ===========================================================================
// Quality gate: Markdown quality check
// ===========================================================================

/// Checks that the markdown output has tables and a minimum word count.
#[derive(Debug)]
struct MarkdownQualityCheck {
    require_tables: bool,
    min_word_count: usize,
}

#[async_trait]
impl QualityGate for MarkdownQualityCheck {
    async fn evaluate(
        &self,
        _item: &dyn WorkItem,
        _stage: &str,
        output: &StageOutput,
        _ctx: &QualityContext,
    ) -> Result<QualityVerdict> {
        let md = output
            .artefacts
            .as_ref()
            .and_then(|a| a.as_any().downcast_ref::<MarkdownOutput>());

        let Some(md) = md else {
            return Ok(QualityVerdict::Uncertain {
                reason: "Expected MarkdownOutput artefact but found something else".into(),
            });
        };

        let mut failed = Vec::new();

        if md.word_count < self.min_word_count {
            failed.push(CriterionResult {
                name: "word_count".into(),
                expected: format!(">= {}", self.min_word_count),
                actual: format!("{}", md.word_count),
                passed: false,
            });
        }

        if self.require_tables && md.table_count == 0 {
            failed.push(CriterionResult {
                name: "table_preservation".into(),
                expected: "At least 1 table".into(),
                actual: "0 tables found".into(),
                passed: false,
            });
        }

        if failed.is_empty() {
            println!("  [MarkdownQualityCheck] ACCEPTED");
            Ok(QualityVerdict::Accepted)
        } else {
            let summary = format!("{} criteria not met", failed.len());
            println!("  [MarkdownQualityCheck] REJECTED: {summary}");
            Ok(QualityVerdict::Rejected {
                feedback: QualityFeedback {
                    summary,
                    failed_criteria: failed,
                    guidance: Some(serde_json::json!({
                        "hint": "Try OCR-based extraction for table recovery"
                    })),
                },
            })
        }
    }
}

// ===========================================================================
// Stage: Concept Extractor
// ===========================================================================

/// Extracts concepts from upstream markdown. Uses `upstream_artefacts`
/// to access the markdown output from `pdf_to_markdown`.
#[derive(Debug)]
struct ConceptExtractor;

#[async_trait]
impl Stage for ConceptExtractor {
    fn name(&self) -> &str {
        "concept_extractor"
    }

    async fn execute(
        &self,
        item: &dyn WorkItem,
        ctx: &StageContext,
    ) -> Result<StageOutput> {
        println!(
            "  [concept_extractor] Processing item '{}'",
            item.id()
        );

        // Access the upstream markdown artefact.
        let upstream_json = ctx
            .upstream_artefacts
            .get("pdf_to_markdown")
            .and_then(|a| a.to_json());

        if let Some(ref json) = upstream_json {
            println!(
                "  [concept_extractor] Upstream markdown: {} words",
                json.get("word_count").and_then(|v| v.as_u64()).unwrap_or(0)
            );
        }

        // Produce a concept graph (stub).
        let graph = ConceptGraph {
            concepts: vec![
                "quantum computing".into(),
                "superposition".into(),
                "entanglement".into(),
                "qubits".into(),
            ],
            relationships: vec![
                ("quantum computing".into(), "superposition".into()),
                ("quantum computing".into(), "entanglement".into()),
                ("quantum computing".into(), "qubits".into()),
            ],
        };

        Ok(StageOutput::completed_with_summary(
            graph,
            "Extracted 4 concepts, 3 relationships",
        ))
    }
}

// ===========================================================================
// Quality gate: Concept graph validator
// ===========================================================================

/// Requires at least `min_concepts` in the concept graph.
#[derive(Debug)]
struct ConceptGraphValidator {
    min_concepts: usize,
}

#[async_trait]
impl QualityGate for ConceptGraphValidator {
    async fn evaluate(
        &self,
        _item: &dyn WorkItem,
        _stage: &str,
        output: &StageOutput,
        _ctx: &QualityContext,
    ) -> Result<QualityVerdict> {
        let graph = output
            .artefacts
            .as_ref()
            .and_then(|a| a.as_any().downcast_ref::<ConceptGraph>());

        let Some(graph) = graph else {
            return Ok(QualityVerdict::Uncertain {
                reason: "Expected ConceptGraph artefact".into(),
            });
        };

        if graph.concepts.len() >= self.min_concepts {
            println!(
                "  [ConceptGraphValidator] ACCEPTED ({} concepts)",
                graph.concepts.len()
            );
            Ok(QualityVerdict::Accepted)
        } else {
            let summary = format!(
                "Only {} concepts found, need at least {}",
                graph.concepts.len(),
                self.min_concepts
            );
            println!("  [ConceptGraphValidator] REJECTED: {summary}");
            Ok(QualityVerdict::Rejected {
                feedback: QualityFeedback {
                    summary,
                    failed_criteria: vec![CriterionResult {
                        name: "min_concepts".into(),
                        expected: format!(">= {}", self.min_concepts),
                        actual: format!("{}", graph.concepts.len()),
                        passed: false,
                    }],
                    guidance: None,
                },
            })
        }
    }
}

// ===========================================================================
// Stage: Embedding Generator (v1-style, no quality gate)
// ===========================================================================

#[derive(Debug)]
struct EmbeddingGenerator;

#[async_trait]
impl Stage for EmbeddingGenerator {
    fn name(&self) -> &str {
        "embedding_gen"
    }

    async fn execute(
        &self,
        item: &dyn WorkItem,
        _ctx: &StageContext,
    ) -> Result<StageOutput> {
        println!("  [embedding_gen] Generating embeddings for '{}'", item.id());
        // v1-style: no artefacts, just completes.
        Ok(StageOutcome::Complete.into())
    }
}

// ===========================================================================
// Stage: Final Review (always blocks for human review)
// ===========================================================================

#[derive(Debug)]
struct FinalReviewStage;

#[async_trait]
impl Stage for FinalReviewStage {
    fn name(&self) -> &str {
        "final_review"
    }

    async fn execute(
        &self,
        item: &dyn WorkItem,
        _ctx: &StageContext,
    ) -> Result<StageOutput> {
        println!("  [final_review] Preparing review for '{}'", item.id());
        Ok(StageOutcome::Complete.into())
    }
}

// ===========================================================================
// Event listener (prints events to stdout)
// ===========================================================================

async fn print_events(mut rx: tokio::sync::broadcast::Receiver<WorkflowEvent>) {
    while let Ok(event) = rx.recv().await {
        let stage_info = event
            .stage()
            .map(|s| format!(" [{s}]"))
            .unwrap_or_default();
        match &event {
            WorkflowEvent::StageStarted { .. } => {
                println!("  EVENT: StageStarted{stage_info}");
            }
            WorkflowEvent::StageCompleted { .. } => {
                println!("  EVENT: StageCompleted{stage_info}");
            }
            WorkflowEvent::StageFailed { error, .. } => {
                println!("  EVENT: StageFailed{stage_info}: {error}");
            }
            WorkflowEvent::QualityCheckPassed { attempt, .. } => {
                println!("  EVENT: QualityCheckPassed{stage_info} (attempt {attempt})");
            }
            WorkflowEvent::QualityCheckFailed {
                attempt,
                feedback_summary,
                ..
            } => {
                println!(
                    "  EVENT: QualityCheckFailed{stage_info} (attempt {attempt}): {feedback_summary}"
                );
            }
            WorkflowEvent::RetryScheduled {
                attempt,
                max_attempts,
                ..
            } => {
                println!(
                    "  EVENT: RetryScheduled{stage_info} (attempt {attempt}/{max_attempts})"
                );
            }
            WorkflowEvent::RetryAttempt {
                attempt,
                max_attempts,
                ..
            } => {
                println!(
                    "  EVENT: RetryAttempt{stage_info} (attempt {attempt}/{max_attempts})"
                );
            }
            WorkflowEvent::Escalated { reason, .. } => {
                println!("  EVENT: Escalated{stage_info}: {reason}");
            }
            WorkflowEvent::ReviewRequired { .. } => {
                println!("  EVENT: ReviewRequired{stage_info}");
            }
            WorkflowEvent::WorkflowCompleted { .. } => {
                println!("  EVENT: WorkflowCompleted");
            }
            _ => {
                println!("  EVENT: {:?}", event);
            }
        }
    }
}

// ===========================================================================
// Main
// ===========================================================================

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    println!("=== Treadle Quality Pipeline Example ===\n");

    // -----------------------------------------------------------------------
    // 1. Build the workflow
    // -----------------------------------------------------------------------
    println!("Building workflow ...\n");

    let workflow = Workflow::builder()
        // Stages
        .stage("pdf_to_markdown", PdfToMarkdown::new())
        .stage("concept_extractor", ConceptExtractor)
        .stage("embedding_gen", EmbeddingGenerator)
        .stage("final_review", FinalReviewStage)
        // Dependencies
        .dependency("concept_extractor", "pdf_to_markdown")
        .dependency("embedding_gen", "pdf_to_markdown")
        .dependency("final_review", "concept_extractor")
        .dependency("final_review", "embedding_gen")
        // Quality gates
        .quality_gate(
            "pdf_to_markdown",
            MarkdownQualityCheck {
                require_tables: true,
                min_word_count: 20,
            },
        )
        .quality_gate(
            "concept_extractor",
            ConceptGraphValidator { min_concepts: 3 },
        )
        // Retry budgets
        .retry_budget(
            "pdf_to_markdown",
            RetryBudget {
                max_attempts: 3,
                delay: None,
                attempt_timeout: Some(Duration::from_secs(2)),
                on_exhausted: ExhaustedAction::Escalate,
            },
        )
        .retry_budget(
            "concept_extractor",
            RetryBudget {
                max_attempts: 2,
                delay: None,
                attempt_timeout: Some(Duration::from_secs(2)),
                on_exhausted: ExhaustedAction::Escalate,
            },
        )
        // Review policies
        .review_policy("final_review", ReviewPolicy::Always)
        .review_policy("pdf_to_markdown", ReviewPolicy::OnEscalation)
        .review_policy("concept_extractor", ReviewPolicy::OnEscalation)
        .build()?;

    println!(
        "Workflow built with {} stages: {:?}\n",
        workflow.stage_count(),
        workflow.stages()
    );

    // -----------------------------------------------------------------------
    // 2. Subscribe to events
    // -----------------------------------------------------------------------
    let event_rx = workflow.subscribe();
    let event_handle = tokio::spawn(print_events(event_rx));

    // -----------------------------------------------------------------------
    // 3. Create a document and a state store
    // -----------------------------------------------------------------------
    let mut store = MemoryStateStore::new();
    let document = Document {
        id: "doc-quantum-2024".into(),
        title: "Introduction to Quantum Computing".into(),
        pdf_content: "(stub PDF bytes)".into(),
    };

    // -----------------------------------------------------------------------
    // 4. Advance the document through the pipeline
    // -----------------------------------------------------------------------
    println!("--- Advancing document through pipeline ---\n");

    workflow.advance(&document, &mut store).await?;

    // Give events a moment to print.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // -----------------------------------------------------------------------
    // 5. Check pipeline status
    // -----------------------------------------------------------------------
    println!("\n--- Pipeline status after first advance ---\n");

    for stage_name in workflow.stages() {
        let state = store.get_stage_state(&document.id, stage_name).await?;
        let status = state
            .as_ref()
            .map(|s| format!("{:?}", s.status))
            .unwrap_or_else(|| "Not started".into());
        println!("  {stage_name}: {status}");
    }

    // -----------------------------------------------------------------------
    // 6. Check attempt history for pdf_to_markdown
    // -----------------------------------------------------------------------
    let attempts = store
        .get_attempts(&document.id, "pdf_to_markdown")
        .await?;
    println!(
        "\n--- pdf_to_markdown attempt history: {} attempts ---\n",
        attempts.len()
    );
    for attempt in &attempts {
        println!(
            "  Attempt {}: summary={:?}, verdict={:?}",
            attempt.attempt, attempt.output_summary, attempt.quality_verdict
        );
    }

    // -----------------------------------------------------------------------
    // 7. Resolve the final review
    // -----------------------------------------------------------------------
    println!("\n--- Resolving final review (Approve) ---\n");

    workflow
        .resolve_review(
            &document.id,
            "final_review",
            ReviewOutcome::Approve,
            &mut store,
        )
        .await?;

    // Advance again to complete any stages unblocked by the review.
    workflow.advance(&document, &mut store).await?;

    // Give events a moment to print.
    tokio::time::sleep(Duration::from_millis(50)).await;

    // -----------------------------------------------------------------------
    // 8. Final status
    // -----------------------------------------------------------------------
    println!("\n--- Final pipeline status ---\n");

    for stage_name in workflow.stages() {
        let state = store.get_stage_state(&document.id, stage_name).await?;
        let status = state
            .as_ref()
            .map(|s| format!("{:?}", s.status))
            .unwrap_or_else(|| "Not started".into());
        println!("  {stage_name}: {status}");
    }

    let complete = workflow.is_complete(&document.id, &store).await?;
    println!("\nWorkflow complete: {complete}");

    // Clean up the event listener.
    event_handle.abort();

    println!("\n=== Done ===");
    Ok(())
}
```

### Design Decisions

- **Crate docs** are expanded to explain the v2 work/judgement/policy separation and list key new types.
- **The example uses stub implementations** that produce fake output but demonstrate all v2 features: quality gates with typed downcasting, retry budgets, review policies, event subscriptions, attempt history, and `resolve_review`.
- **`MarkdownOutput` and `ConceptGraph` are custom `Artefact` implementations** showing both `to_json()` for persistence and `as_any()` for typed downcasting in quality gates.
- **The `PdfToMarkdown` stage simulates improving quality** across attempts (poor on attempt 1, better on attempt 2, good on attempt 3), matching the real-world pattern of AI agents adapting to feedback.
- **The event listener** runs in a separate task and prints human-readable event descriptions, demonstrating event subscription for monitoring.
- **The example is fully runnable** with `cargo run --example quality_pipeline`.

### Verification Commands

```bash
# Verify the example compiles
cargo build --example quality_pipeline

# Run the example
cargo run --example quality_pipeline

# Verify crate docs build
cargo doc --no-deps

# Run all tests including integration tests
cargo test --all-targets

# Lint everything
cargo clippy --all-targets
```

---

## Phase 11 Completion Checklist

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes all tests (including all v1 and v2 tests)
- [ ] `cargo test --test mixed_pipeline` passes
- [ ] `cargo test --test approve_with_edits` passes
- [ ] `cargo test --test attempt_timeout` passes
- [ ] `cargo build --example quality_pipeline` compiles
- [ ] `cargo run --example quality_pipeline` runs and demonstrates all v2 features
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo doc --no-deps` builds without warnings
- [ ] Crate-level docs updated with v2 overview and new type catalogue
- [ ] All existing v1 tests still pass
- [ ] Mixed pipeline test validates v1/v2 coexistence in one workflow
- [ ] ApproveWithEdits test validates artefact replacement flows to downstream stages
- [ ] Timeout tests validate deterministic timeout behaviour (paused time)
- [ ] Event ordering verified in integration tests
- [ ] Attempt history verified in integration tests

### Public API Demonstrated in Phase 11

```rust
// Types exercised in integration tests and the example
treadle::Artefact                // Trait: custom artefact types
treadle::StageOutput             // completed_with, completed_with_summary, From<StageOutcome>
treadle::QualityGate             // Trait: evaluate()
treadle::QualityVerdict          // Accepted, Rejected, Uncertain
treadle::QualityFeedback         // summary, failed_criteria, guidance
treadle::CriterionResult         // name, expected, actual, passed
treadle::QualityContext          // attempt, max_attempts, previous_attempts
treadle::RetryBudget             // max_attempts, delay, attempt_timeout, on_exhausted
treadle::ExhaustedAction         // Fail, Escalate
treadle::ReviewPolicy            // Never, Always, OnEscalation, OnUncertain, OnEscalationOrUncertain
treadle::ReviewOutcome           // Approve, Reject, ApproveWithEdits
treadle::AttemptRecord           // attempt, started_at, completed_at, output_summary, ...
treadle::WorkflowEvent           // QualityCheckPassed, QualityCheckFailed, RetryScheduled, etc.
treadle::Workflow::resolve_review  // Method for resolving human reviews
treadle::Workflow::subscribe       // Method for event subscription
treadle::StateStore::get_attempts  // Method for retrieving attempt history
treadle::MemoryStateStore          // In-memory store used in tests and example
```

---

## Notes on Rust Guidelines Applied

### Anti-Patterns Avoided
- **AP-01:** No boolean arguments anywhere -- quality gate decisions use typed enums (`QualityVerdict`, `ReviewOutcome`)
- **AP-03:** All stage and gate implementations are `Send + Sync` for async safety
- **AP-09:** No `unwrap()` in library code; test code uses `unwrap()` only where failure would indicate a genuine test bug
- **AP-12:** No unnecessary clones -- artefacts are consumed or borrowed, never copied unnecessarily
- **AP-15:** No `#[allow(unused)]` -- all code is exercised

### Core Idioms Applied
- **ID-04:** `impl Into<String>` for builder methods and constructors
- **ID-09:** `new()` as canonical constructor for stage and gate types
- **ID-11:** `Serialize`/`Deserialize` on work items and `AttemptRecord` for persistence
- **ID-35:** Builder methods returning `Self` on `StageOutput`

### Testing Patterns
- **TS-01:** Test names follow `test_<function>_<scenario>_<expectation>` convention
- **TS-04:** Paused-time tests (`start_paused = true`) for deterministic timeout testing
- **TS-07:** Integration tests in `tests/` directory, unit tests in `#[cfg(test)]` modules
- **TS-09:** Shared test helpers (work items, stages) defined at the top of each test file

### Error Handling
- **EH-07:** Timeout errors recorded in attempt history with descriptive summaries
- **EH-10:** `Result` propagated through all async boundaries; errors surfaced to callers

### Concurrency
- `tokio::time::timeout` for attempt timeouts
- `tokio::sync::broadcast` for event distribution
- `Arc<AtomicU32>` for call counters in tests (lock-free)
- `Arc<Mutex<...>>` for shared test state capture
- Paused-time tests for deterministic timeout behaviour
