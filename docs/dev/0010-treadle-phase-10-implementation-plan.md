# Treadle Phase 10 Implementation Plan

## Phase 10: Quality Gate Integration

**Goal:** Replace the no-op quality gate stub introduced in Phase 9 with actual `QualityGate::evaluate` calls. The judgement step now runs a real quality gate (when one is attached to a stage), and `QualityVerdict` drives the retry/escalate decision. `ReviewPolicy` variants are fully applied. New events (`QualityCheckPassed`, `QualityCheckFailed`, `Escalated`) provide observability into the quality evaluation lifecycle.

**Prerequisites:**
- Phases 1--9 completed
- `Artefact` trait, `StageOutput`, `Stage::execute` returning `StageOutput` (Phase 6)
- `QualityGate` trait, `QualityVerdict`, `QualityFeedback`, `CriterionResult`, `QualityContext` (Phase 7)
- `RetryBudget`, `ExhaustedAction`, `ReviewPolicy`, `ReviewOutcome`, `resolve_review` (Phase 8)
- Retry loop in `advance()` with no-op quality gate stub, `AttemptRecord`, attempt history, `attempt_timeout`, `RetryScheduled`/`RetryAttempt` events (Phase 9)
- `cargo build` and `cargo test` pass

---

## Milestone 10.1 -- New Events: QualityCheckPassed, QualityCheckFailed, Escalated

### Files to Modify
- `src/event.rs` (add new event variants)

### Implementation Details

#### 10.1.1 Add Event Variants

Add to `WorkflowEvent` in `src/event.rs`:

```rust
/// A quality gate accepted the stage output.
QualityCheckPassed {
    /// The work item's identifier (stringified).
    item_id: String,
    /// The stage name.
    stage: String,
    /// The attempt number that passed.
    attempt: u32,
},

/// A quality gate rejected the stage output.
QualityCheckFailed {
    /// The work item's identifier (stringified).
    item_id: String,
    /// The stage name.
    stage: String,
    /// The attempt number that failed.
    attempt: u32,
    /// A human-readable summary of the quality feedback.
    feedback_summary: String,
},

/// A stage has been escalated to human review.
///
/// Escalation occurs when:
/// - The retry budget is exhausted and `ExhaustedAction::Escalate` is set
/// - The quality gate returns `Uncertain`
/// - The `ReviewPolicy` requires review for the given outcome
Escalated {
    /// The work item's identifier (stringified).
    item_id: String,
    /// The stage name.
    stage: String,
    /// The reason for escalation.
    reason: String,
},
```

#### 10.1.2 Update `item_id()` and `stage()` Accessors

Update the `item_id()` match arm to include the new variants:

```rust
pub fn item_id(&self) -> &str {
    match self {
        Self::StageStarted { item_id, .. }
        | Self::StageCompleted { item_id, .. }
        | Self::StageFailed { item_id, .. }
        | Self::ReviewRequired { item_id, .. }
        | Self::StageSkipped { item_id, .. }
        | Self::StageRetried { item_id, .. }
        | Self::RetryScheduled { item_id, .. }
        | Self::RetryAttempt { item_id, .. }
        | Self::QualityCheckPassed { item_id, .. }
        | Self::QualityCheckFailed { item_id, .. }
        | Self::Escalated { item_id, .. }
        | Self::WorkflowCompleted { item_id } => item_id,
    }
}

pub fn stage(&self) -> Option<&str> {
    match self {
        Self::StageStarted { stage, .. }
        | Self::StageCompleted { stage, .. }
        | Self::StageFailed { stage, .. }
        | Self::ReviewRequired { stage, .. }
        | Self::StageSkipped { stage, .. }
        | Self::StageRetried { stage, .. }
        | Self::RetryScheduled { stage, .. }
        | Self::RetryAttempt { stage, .. }
        | Self::QualityCheckPassed { stage, .. }
        | Self::QualityCheckFailed { stage, .. }
        | Self::Escalated { stage, .. } => Some(stage),
        Self::WorkflowCompleted { .. } => None,
    }
}
```

### Design Decisions

- **`feedback_summary` is a `String`, not a full `QualityFeedback`**: Events should be lightweight and `Clone`-able for the broadcast channel. The full feedback is available in the `AttemptRecord`; the event carries only a human-readable summary for logging and real-time monitoring.
- **`Escalated` covers all escalation paths**: Whether escalation comes from budget exhaustion, an `Uncertain` verdict, or a `ReviewPolicy`, the same event is emitted. The `reason` field distinguishes the cause.
- **Events are `#[non_exhaustive]`**: Already the case from v1; new variants do not break downstream consumers.

### Tests

```rust
#[test]
fn test_quality_check_passed_event() {
    let event = WorkflowEvent::QualityCheckPassed {
        item_id: "item-1".into(),
        stage: "pdf_to_md".into(),
        attempt: 1,
    };
    assert_eq!(event.item_id(), "item-1");
    assert_eq!(event.stage(), Some("pdf_to_md"));
    assert!(!event.is_error());
}

#[test]
fn test_quality_check_failed_event() {
    let event = WorkflowEvent::QualityCheckFailed {
        item_id: "item-1".into(),
        stage: "pdf_to_md".into(),
        attempt: 2,
        feedback_summary: "Tables lost in conversion".into(),
    };
    assert_eq!(event.item_id(), "item-1");
    assert_eq!(event.stage(), Some("pdf_to_md"));
    let debug = format!("{:?}", event);
    assert!(debug.contains("QualityCheckFailed"));
    assert!(debug.contains("Tables lost"));
}

#[test]
fn test_quality_check_failed_event_is_not_error() {
    // QualityCheckFailed is a quality gate rejection, not a stage
    // execution error. The stage ran successfully but produced
    // inadequate output.
    let event = WorkflowEvent::QualityCheckFailed {
        item_id: "item-1".into(),
        stage: "pdf_to_md".into(),
        attempt: 1,
        feedback_summary: "Word count too low".into(),
    };
    assert!(!event.is_error());
}

#[test]
fn test_escalated_event() {
    let event = WorkflowEvent::Escalated {
        item_id: "item-1".into(),
        stage: "pdf_to_md".into(),
        reason: "Retry budget exhausted after 3 attempts".into(),
    };
    assert_eq!(event.item_id(), "item-1");
    assert_eq!(event.stage(), Some("pdf_to_md"));
    let debug = format!("{:?}", event);
    assert!(debug.contains("Escalated"));
    assert!(debug.contains("Retry budget exhausted"));
}

#[test]
fn test_escalated_event_clone() {
    let event = WorkflowEvent::Escalated {
        item_id: "item-1".into(),
        stage: "extract".into(),
        reason: "Uncertain quality verdict".into(),
    };
    let cloned = event.clone();
    assert_eq!(event.item_id(), cloned.item_id());
    assert_eq!(event.stage(), cloned.stage());
}

#[test]
fn test_quality_events_are_not_completion() {
    let passed = WorkflowEvent::QualityCheckPassed {
        item_id: "x".into(),
        stage: "s".into(),
        attempt: 1,
    };
    let failed = WorkflowEvent::QualityCheckFailed {
        item_id: "x".into(),
        stage: "s".into(),
        attempt: 1,
        feedback_summary: "bad".into(),
    };
    let escalated = WorkflowEvent::Escalated {
        item_id: "x".into(),
        stage: "s".into(),
        reason: "r".into(),
    };
    assert!(!passed.is_completion());
    assert!(!failed.is_completion());
    assert!(!escalated.is_completion());
}
```

### Verification Commands

```bash
cargo build
cargo test event
cargo clippy
```

---

## Milestone 10.2 -- Wire Quality Gate Evaluation into Retry Loop

### Files to Modify
- `src/workflow.rs` (modify `execute_stage_with_retry` method)

### Implementation Details

This is the core milestone. The no-op quality gate stub from Phase 9:

```rust
// === QUALITY GATE STUB: always Accepted ===
// In Phase 10, this will call the actual quality gate.
let verdict = QualityVerdict::Accepted;
```

is replaced with actual quality gate evaluation. The full retry loop is revised to handle all three verdict types and all five `ReviewPolicy` variants.

#### 10.2.1 Quality Gate Lookup

The `Workflow` struct already stores quality gates per stage (added in Phase 7 via `WorkflowBuilder::quality_gate()`). Add a helper method to look up the gate:

```rust
/// Returns the quality gate for a stage, if one is attached.
pub(crate) fn get_quality_gate(
    &self,
    stage_name: &str,
) -> Option<&Arc<dyn QualityGate<W>>> {
    self.quality_gates.get(stage_name)
}
```

Where `quality_gates: HashMap<String, Arc<dyn QualityGate<W>>>` was added to `Workflow` in Phase 7.

#### 10.2.2 Build QualityContext

Add a helper to construct the `QualityContext` from the current state:

```rust
/// Builds a [`QualityContext`] for a quality gate evaluation.
fn build_quality_context(
    &self,
    item: &W,
    stage_name: &str,
    attempt: u32,
    previous_attempts: &[AttemptRecord],
) -> QualityContext {
    let budget = self.get_retry_budget(stage_name);
    QualityContext {
        attempt,
        max_attempts: budget.max_attempts,
        previous_attempts: previous_attempts.to_vec(),
        stage_name: stage_name.to_string(),
        item_id: item.id().to_string(),
    }
}
```

#### 10.2.3 ReviewPolicy Resolution Helper

Add a helper that determines whether a given `ReviewPolicy` requires review for a specific situation:

```rust
/// Determines whether the review policy requires human review.
///
/// # Arguments
///
/// * `policy` - The review policy for this stage.
/// * `is_exhausted` - Whether the retry budget was exhausted.
/// * `is_uncertain` - Whether the quality verdict was `Uncertain`.
fn requires_review(
    policy: &ReviewPolicy,
    is_exhausted: bool,
    is_uncertain: bool,
) -> bool {
    match policy {
        ReviewPolicy::Never => false,
        ReviewPolicy::Always => true,
        ReviewPolicy::OnEscalation => is_exhausted,
        ReviewPolicy::OnUncertain => is_uncertain,
        ReviewPolicy::OnEscalationOrUncertain => is_exhausted || is_uncertain,
    }
}
```

#### 10.2.4 Revised `execute_stage_with_retry`

The complete revised method, replacing the Phase 9 stub:

```rust
async fn execute_stage_with_retry(
    &self,
    item: &W,
    stage_name: &str,
    store: &dyn StateStore<W>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
) -> Result<()> {
    let stage = self.get_stage(stage_name)?;
    let budget = self.get_retry_budget(stage_name);
    let review_policy = self.get_review_policy(stage_name);
    let quality_gate = self.get_quality_gate(stage_name);

    let mut attempt: u32 = 1;
    let mut feedback: Option<QualityFeedback> = None;

    loop {
        // Emit RetryAttempt event for retries (attempt > 1)
        if attempt > 1 {
            let _ = event_tx.send(WorkflowEvent::RetryAttempt {
                item_id: item.id().to_string(),
                stage: stage_name.to_string(),
                attempt,
                max_attempts: budget.max_attempts,
                feedback_summary: feedback.as_ref().map(|f| f.summary.clone()),
            });
        }

        // Build context with attempt number and previous feedback
        let mut ctx = StageContext::new(item.id(), stage_name)
            .with_attempt(attempt);
        if let Some(ref fb) = feedback {
            ctx = ctx.with_feedback(serde_json::to_value(fb).unwrap_or_default());
        }
        // Populate upstream artefacts from completed dependencies
        for dep_name in self.dependencies(stage_name)? {
            if let Some(json) = store.get_artefact_summary(
                item.id(), dep_name,
            ).await? {
                ctx = ctx.with_upstream_artefact(
                    dep_name,
                    Box::new(json) as Box<dyn Artefact>,
                );
            }
        }

        // Mark stage as running
        store.set_status(
            item.id(), stage_name, StageStatus::running(),
        ).await?;

        // Execute with optional timeout
        let exec_result = if let Some(timeout_duration) = budget.attempt_timeout {
            match tokio::time::timeout(
                timeout_duration,
                stage.execute(item, &ctx),
            ).await {
                Ok(result) => result,
                Err(_elapsed) => {
                    // Timeout: treat as a failed attempt
                    let record = AttemptRecord::start(attempt)
                        .with_output_summary("Attempt timed out")
                        .complete();
                    store.record_attempt(
                        item.id(), stage_name, record,
                    ).await?;

                    // Check if we can retry
                    if attempt < budget.max_attempts {
                        attempt += 1;
                        feedback = Some(QualityFeedback {
                            summary: format!(
                                "Attempt timed out after {}ms",
                                timeout_duration.as_millis()
                            ),
                            failed_criteria: vec![],
                            guidance: None,
                        });
                        let _ = event_tx.send(WorkflowEvent::RetryScheduled {
                            item_id: item.id().to_string(),
                            stage: stage_name.to_string(),
                            attempt,
                            max_attempts: budget.max_attempts,
                        });
                        if let Some(delay) = budget.delay {
                            tokio::time::sleep(delay).await;
                        }
                        continue;
                    }
                    // Exhausted
                    return self.handle_exhausted(
                        item, stage_name, &budget, &review_policy,
                        store, event_tx,
                    ).await;
                }
            }
        } else {
            stage.execute(item, &ctx).await
        };

        // Handle execution errors
        let output = match exec_result {
            Ok(output) => output,
            Err(e) => {
                let record = AttemptRecord::start(attempt)
                    .with_output_summary(format!("Error: {}", e))
                    .complete();
                store.record_attempt(
                    item.id(), stage_name, record,
                ).await?;
                store.set_status(
                    item.id(),
                    stage_name,
                    StageStatus::failed(e.to_string()),
                ).await?;
                return Err(e);
            }
        };

        // Store artefact summary
        let artefact_json = output.artefacts.as_ref()
            .and_then(|a| a.to_json());
        store.store_artefact_summary(
            item.id(), stage_name, artefact_json.clone(),
        ).await?;

        // === QUALITY GATE EVALUATION ===
        let previous_attempts = store.get_attempts(
            item.id(), stage_name,
        ).await?;

        let verdict = if let Some(gate) = quality_gate {
            let quality_ctx = self.build_quality_context(
                item, stage_name, attempt, &previous_attempts,
            );
            gate.evaluate(item, stage_name, &output, &quality_ctx).await?
        } else {
            // No quality gate attached: behave like v1 (always accepted)
            QualityVerdict::Accepted
        };

        // Record the attempt with its verdict
        let mut record = AttemptRecord::start(attempt)
            .with_artefacts(artefact_json);
        if let Some(ref summary) = output.summary {
            record = record.with_output_summary(summary.clone());
        }
        record = record.with_verdict(&verdict);
        let record = record.complete();
        store.record_attempt(
            item.id(), stage_name, record,
        ).await?;

        // Handle verdict
        match verdict {
            QualityVerdict::Accepted => {
                // Emit quality check passed event
                let _ = event_tx.send(WorkflowEvent::QualityCheckPassed {
                    item_id: item.id().to_string(),
                    stage: stage_name.to_string(),
                    attempt,
                });

                // Apply review policy for the accepted case
                if Self::requires_review(
                    &review_policy,
                    /* is_exhausted */ false,
                    /* is_uncertain */ false,
                ) {
                    // Only ReviewPolicy::Always reaches here
                    store.set_status(
                        item.id(),
                        stage_name,
                        StageStatus::awaiting_review(),
                    ).await?;
                    let _ = event_tx.send(WorkflowEvent::Escalated {
                        item_id: item.id().to_string(),
                        stage: stage_name.to_string(),
                        reason: "ReviewPolicy::Always requires human review"
                            .to_string(),
                    });
                    return Ok(());
                }

                // No review required: stage completes
                store.set_status(
                    item.id(),
                    stage_name,
                    StageStatus::completed(),
                ).await?;
                let _ = event_tx.send(WorkflowEvent::StageCompleted {
                    item_id: item.id().to_string(),
                    stage: stage_name.to_string(),
                });
                return Ok(());
            }

            QualityVerdict::Rejected { feedback: fb } => {
                // Emit quality check failed event
                let _ = event_tx.send(WorkflowEvent::QualityCheckFailed {
                    item_id: item.id().to_string(),
                    stage: stage_name.to_string(),
                    attempt,
                    feedback_summary: fb.summary.clone(),
                });

                // Check if retries remain
                if attempt < budget.max_attempts {
                    attempt += 1;
                    feedback = Some(fb);
                    let _ = event_tx.send(WorkflowEvent::RetryScheduled {
                        item_id: item.id().to_string(),
                        stage: stage_name.to_string(),
                        attempt,
                        max_attempts: budget.max_attempts,
                    });
                    if let Some(delay) = budget.delay {
                        tokio::time::sleep(delay).await;
                    }
                    continue;
                }

                // Retry budget exhausted
                return self.handle_exhausted(
                    item, stage_name, &budget, &review_policy,
                    store, event_tx,
                ).await;
            }

            QualityVerdict::Uncertain { reason } => {
                // Uncertain: escalate to review regardless of retry budget
                let _ = event_tx.send(WorkflowEvent::Escalated {
                    item_id: item.id().to_string(),
                    stage: stage_name.to_string(),
                    reason: reason.clone(),
                });

                store.set_status(
                    item.id(),
                    stage_name,
                    StageStatus::awaiting_review(),
                ).await?;
                return Ok(());
            }
        }
    }
}
```

#### 10.2.5 Revised `handle_exhausted` Helper

The `handle_exhausted` method from Phase 9 is updated to accept `review_policy` and to apply it when determining the exhausted behaviour:

```rust
async fn handle_exhausted(
    &self,
    item: &W,
    stage_name: &str,
    budget: &RetryBudget,
    review_policy: &ReviewPolicy,
    store: &dyn StateStore<W>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
) -> Result<()> {
    match budget.on_exhausted {
        ExhaustedAction::Fail => {
            // Check if review policy overrides the fail
            if Self::requires_review(
                review_policy,
                /* is_exhausted */ true,
                /* is_uncertain */ false,
            ) {
                store.set_status(
                    item.id(),
                    stage_name,
                    StageStatus::awaiting_review(),
                ).await?;
                let _ = event_tx.send(WorkflowEvent::Escalated {
                    item_id: item.id().to_string(),
                    stage: stage_name.to_string(),
                    reason: "Retry budget exhausted; review policy \
                             requires human review".to_string(),
                });
                return Ok(());
            }

            store.set_status(
                item.id(),
                stage_name,
                StageStatus::failed("Retry budget exhausted"),
            ).await?;
            let _ = event_tx.send(WorkflowEvent::StageFailed {
                item_id: item.id().to_string(),
                stage: stage_name.to_string(),
                error: "Retry budget exhausted".to_string(),
            });
            Ok(())
        }

        ExhaustedAction::Escalate => {
            store.set_status(
                item.id(),
                stage_name,
                StageStatus::awaiting_review(),
            ).await?;
            let _ = event_tx.send(WorkflowEvent::Escalated {
                item_id: item.id().to_string(),
                stage: stage_name.to_string(),
                reason: "Retry budget exhausted".to_string(),
            });
            Ok(())
        }
    }
}
```

### Design Decisions

- **No quality gate = v1 behaviour**: When no quality gate is attached to a stage, the verdict is always `Accepted`. This preserves complete backward compatibility: v1 stages with no quality gate, no retry budget, and `ReviewPolicy::Never` behave identically to v1.
- **`QualityContext` includes previous attempts**: The gate has full visibility into the attempt history, enabling intelligent decisions such as "the last two attempts both failed on table extraction, escalate now rather than retrying with the same strategy."
- **`ReviewPolicy` is consulted on both accepted and exhausted paths**: `ReviewPolicy::Always` blocks even when quality passes. `ReviewPolicy::OnEscalation` blocks when the budget is exhausted, even if `ExhaustedAction::Fail` is set (the review policy overrides the fail action). This gives operators fine-grained control.
- **`Uncertain` always escalates**: The design document specifies that `Uncertain` escalates "regardless of retry budget." This means even if retries remain, an `Uncertain` verdict immediately escalates. The rationale: if the gate cannot determine quality, retrying will not help -- human judgement is required.
- **`requires_review` is a pure function**: It takes the policy and two booleans, making it trivially testable without constructing a full workflow.
- **Events are emitted before status updates where possible**: This ensures subscribers see the event before the state store reflects the change, matching the chronological order of operations.

### Tests

```rust
#[test]
fn test_requires_review_never() {
    assert!(!Workflow::requires_review(&ReviewPolicy::Never, false, false));
    assert!(!Workflow::requires_review(&ReviewPolicy::Never, true, false));
    assert!(!Workflow::requires_review(&ReviewPolicy::Never, false, true));
    assert!(!Workflow::requires_review(&ReviewPolicy::Never, true, true));
}

#[test]
fn test_requires_review_always() {
    assert!(Workflow::requires_review(&ReviewPolicy::Always, false, false));
    assert!(Workflow::requires_review(&ReviewPolicy::Always, true, false));
    assert!(Workflow::requires_review(&ReviewPolicy::Always, false, true));
    assert!(Workflow::requires_review(&ReviewPolicy::Always, true, true));
}

#[test]
fn test_requires_review_on_escalation() {
    assert!(!Workflow::requires_review(&ReviewPolicy::OnEscalation, false, false));
    assert!(Workflow::requires_review(&ReviewPolicy::OnEscalation, true, false));
    assert!(!Workflow::requires_review(&ReviewPolicy::OnEscalation, false, true));
    assert!(Workflow::requires_review(&ReviewPolicy::OnEscalation, true, true));
}

#[test]
fn test_requires_review_on_uncertain() {
    assert!(!Workflow::requires_review(&ReviewPolicy::OnUncertain, false, false));
    assert!(!Workflow::requires_review(&ReviewPolicy::OnUncertain, true, false));
    assert!(Workflow::requires_review(&ReviewPolicy::OnUncertain, false, true));
    assert!(Workflow::requires_review(&ReviewPolicy::OnUncertain, true, true));
}

#[test]
fn test_requires_review_on_escalation_or_uncertain() {
    assert!(!Workflow::requires_review(
        &ReviewPolicy::OnEscalationOrUncertain, false, false,
    ));
    assert!(Workflow::requires_review(
        &ReviewPolicy::OnEscalationOrUncertain, true, false,
    ));
    assert!(Workflow::requires_review(
        &ReviewPolicy::OnEscalationOrUncertain, false, true,
    ));
    assert!(Workflow::requires_review(
        &ReviewPolicy::OnEscalationOrUncertain, true, true,
    ));
}
```

### Verification Commands

```bash
cargo build
cargo test workflow
cargo clippy
```

---

## Milestone 10.3 -- End-to-End Tests

### Files to Create/Modify
- `tests/quality_gate_integration.rs` (new integration test file)

### Implementation Details

#### 10.3.1 Test Gate Implementations

These gates are defined at the top of the integration test file. They implement `QualityGate<TestItem>` and provide controlled behaviour for testing:

```rust
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

use treadle::{
    Artefact, AttemptRecord, CriterionResult, ExhaustedAction, QualityContext,
    QualityFeedback, QualityGate, QualityVerdict, Result, RetryBudget,
    ReviewPolicy, Stage, StageContext, StageOutcome, StageOutput, StateStore,
    WorkItem, Workflow, WorkflowEvent, MemoryStateStore,
};

// --- Test work item ---

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TestItem {
    id: String,
}

impl TestItem {
    fn new(id: impl Into<String>) -> Self {
        Self { id: id.into() }
    }
}

impl WorkItem for TestItem {
    fn id(&self) -> &str {
        &self.id
    }
}

// --- Test stage ---

/// A stage that always completes with a JSON artefact.
#[derive(Debug)]
struct SimpleStage;

#[async_trait]
impl Stage<TestItem> for SimpleStage {
    fn name(&self) -> &str {
        "simple"
    }

    async fn execute(
        &self,
        _item: &TestItem,
        _ctx: &StageContext,
    ) -> Result<StageOutput> {
        Ok(StageOutput::completed_with(
            serde_json::json!({"result": "ok"}),
        ))
    }
}

/// A stage that records which attempt numbers it sees.
#[derive(Debug)]
struct AttemptAwareStage {
    attempts_seen: Arc<Mutex<Vec<u32>>>,
}

#[async_trait]
impl Stage<TestItem> for AttemptAwareStage {
    fn name(&self) -> &str {
        "attempt_aware"
    }

    async fn execute(
        &self,
        _item: &TestItem,
        ctx: &StageContext,
    ) -> Result<StageOutput> {
        self.attempts_seen.lock().await.push(ctx.attempt);
        Ok(StageOutput::completed_with(
            serde_json::json!({"attempt": ctx.attempt}),
        ))
    }
}

// --- Test quality gates ---

/// A quality gate that always accepts.
struct AlwaysAcceptGate;

#[async_trait]
impl QualityGate<TestItem> for AlwaysAcceptGate {
    async fn evaluate(
        &self,
        _item: &TestItem,
        _stage: &str,
        _output: &StageOutput,
        _ctx: &QualityContext,
    ) -> Result<QualityVerdict> {
        Ok(QualityVerdict::Accepted)
    }
}

/// A quality gate that always rejects with configurable feedback.
struct AlwaysRejectGate {
    feedback: QualityFeedback,
}

#[async_trait]
impl QualityGate<TestItem> for AlwaysRejectGate {
    async fn evaluate(
        &self,
        _item: &TestItem,
        _stage: &str,
        _output: &StageOutput,
        _ctx: &QualityContext,
    ) -> Result<QualityVerdict> {
        Ok(QualityVerdict::Rejected {
            feedback: self.feedback.clone(),
        })
    }
}

/// A quality gate that rejects until a specific attempt number,
/// then accepts.
struct AcceptOnAttemptGate {
    accept_on: u32,
}

#[async_trait]
impl QualityGate<TestItem> for AcceptOnAttemptGate {
    async fn evaluate(
        &self,
        _item: &TestItem,
        _stage: &str,
        _output: &StageOutput,
        ctx: &QualityContext,
    ) -> Result<QualityVerdict> {
        if ctx.attempt >= self.accept_on {
            Ok(QualityVerdict::Accepted)
        } else {
            Ok(QualityVerdict::Rejected {
                feedback: QualityFeedback {
                    summary: format!(
                        "Not ready yet (attempt {}/{})",
                        ctx.attempt, self.accept_on
                    ),
                    failed_criteria: vec![CriterionResult {
                        name: "readiness".to_string(),
                        expected: format!("attempt >= {}", self.accept_on),
                        actual: format!("attempt {}", ctx.attempt),
                        passed: false,
                    }],
                    guidance: None,
                },
            })
        }
    }
}

/// A quality gate that always returns Uncertain.
struct UncertainGate {
    reason: String,
}

#[async_trait]
impl QualityGate<TestItem> for UncertainGate {
    async fn evaluate(
        &self,
        _item: &TestItem,
        _stage: &str,
        _output: &StageOutput,
        _ctx: &QualityContext,
    ) -> Result<QualityVerdict> {
        Ok(QualityVerdict::Uncertain {
            reason: self.reason.clone(),
        })
    }
}
```

#### 10.3.2 Test 1: Quality Gate Accepts on First Attempt

```rust
#[tokio::test]
async fn test_quality_gate_accepts_on_first_attempt() {
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysAcceptGate)
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::Completed);

    let attempts = store.get_attempts(item.id(), "a").await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].attempt, 1);
    assert!(matches!(
        attempts[0].quality_verdict,
        Some(SerializableVerdict::Accepted)
    ));
}
```

#### 10.3.3 Test 2: Quality Gate Rejects Then Accepts

```rust
#[tokio::test]
async fn test_quality_gate_rejects_then_accepts() {
    let attempts_seen = Arc::new(Mutex::new(Vec::new()));

    let workflow = Workflow::builder()
        .stage("a", AttemptAwareStage {
            attempts_seen: attempts_seen.clone(),
        })
        .quality_gate("a", AcceptOnAttemptGate { accept_on: 2 })
        .retry_budget("a", RetryBudget {
            max_attempts: 3,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        })
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    // Stage should complete on attempt 2
    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::Completed);

    // Verify two attempts were recorded
    let attempts = store.get_attempts(item.id(), "a").await.unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].attempt, 1);
    assert!(matches!(
        attempts[0].quality_verdict,
        Some(SerializableVerdict::Rejected)
    ));
    assert_eq!(attempts[1].attempt, 2);
    assert!(matches!(
        attempts[1].quality_verdict,
        Some(SerializableVerdict::Accepted)
    ));

    // Verify the stage saw both attempts
    let seen = attempts_seen.lock().await;
    assert_eq!(*seen, vec![1, 2]);
}
```

#### 10.3.4 Test 3: Quality Gate Exhausts and Fails

```rust
#[tokio::test]
async fn test_quality_gate_exhausts_and_fails() {
    let reject_feedback = QualityFeedback {
        summary: "Tables missing from output".to_string(),
        failed_criteria: vec![CriterionResult {
            name: "table_preservation".to_string(),
            expected: "Tables present".to_string(),
            actual: "No tables found".to_string(),
            passed: false,
        }],
        guidance: Some(serde_json::json!({
            "hint": "Try OCR-based extraction"
        })),
    };

    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysRejectGate {
            feedback: reject_feedback,
        })
        .retry_budget("a", RetryBudget {
            max_attempts: 3,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        })
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    // Stage should be failed
    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::Failed);

    // All 3 attempts should be recorded, all rejected
    let attempts = store.get_attempts(item.id(), "a").await.unwrap();
    assert_eq!(attempts.len(), 3);
    for (i, record) in attempts.iter().enumerate() {
        assert_eq!(record.attempt, (i as u32) + 1);
        assert!(matches!(
            record.quality_verdict,
            Some(SerializableVerdict::Rejected)
        ));
    }
}
```

#### 10.3.5 Test 4: Quality Gate Exhausts and Escalates

```rust
#[tokio::test]
async fn test_quality_gate_exhausts_and_escalates() {
    let reject_feedback = QualityFeedback {
        summary: "Insufficient quality".to_string(),
        failed_criteria: vec![],
        guidance: None,
    };

    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysRejectGate {
            feedback: reject_feedback,
        })
        .retry_budget("a", RetryBudget {
            max_attempts: 3,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Escalate,
        })
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    // Stage should be awaiting review (escalated)
    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::AwaitingReview);

    // All 3 attempts should be recorded
    let attempts = store.get_attempts(item.id(), "a").await.unwrap();
    assert_eq!(attempts.len(), 3);
}
```

#### 10.3.6 Test 5: Quality Gate Uncertain Escalates

```rust
#[tokio::test]
async fn test_quality_gate_uncertain_escalates() {
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", UncertainGate {
            reason: "Cannot assess table quality automatically".to_string(),
        })
        .retry_budget("a", RetryBudget {
            max_attempts: 5,  // Plenty of retries -- should not be used
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        })
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    // Stage should immediately escalate on attempt 1
    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::AwaitingReview);

    // Only 1 attempt should be recorded (no retries)
    let attempts = store.get_attempts(item.id(), "a").await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert!(matches!(
        attempts[0].quality_verdict,
        Some(SerializableVerdict::Uncertain { .. })
    ));
}
```

#### 10.3.7 Test 6: ReviewPolicy::Always with Quality Gate

```rust
#[tokio::test]
async fn test_review_policy_always_with_quality_gate() {
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysAcceptGate)
        .review_policy("a", ReviewPolicy::Always)
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    // Quality gate accepted, but ReviewPolicy::Always blocks for review
    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::AwaitingReview);

    // Only 1 attempt, and it was accepted
    let attempts = store.get_attempts(item.id(), "a").await.unwrap();
    assert_eq!(attempts.len(), 1);
    assert!(matches!(
        attempts[0].quality_verdict,
        Some(SerializableVerdict::Accepted)
    ));
}
```

#### 10.3.8 Test 7: ReviewPolicy::OnEscalation Only Triggers on Exhaust

```rust
#[tokio::test]
async fn test_review_policy_on_escalation_only_triggers_on_exhaust() {
    // Scenario A: quality passes -> completes normally (no review)
    let workflow_pass = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysAcceptGate)
        .review_policy("a", ReviewPolicy::OnEscalation)
        .build()
        .unwrap();

    let store_pass = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow_pass.advance(&item, &store_pass).await.unwrap();

    let status = store_pass.get_status(item.id(), "a")
        .await.unwrap().unwrap();
    assert_eq!(status.state, StageState::Completed);

    // Scenario B: quality exhausts -> AwaitingReview even with
    // ExhaustedAction::Fail, because ReviewPolicy::OnEscalation overrides
    let reject_feedback = QualityFeedback {
        summary: "bad".to_string(),
        failed_criteria: vec![],
        guidance: None,
    };

    let workflow_fail = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysRejectGate {
            feedback: reject_feedback,
        })
        .retry_budget("a", RetryBudget {
            max_attempts: 2,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        })
        .review_policy("a", ReviewPolicy::OnEscalation)
        .build()
        .unwrap();

    let store_fail = MemoryStateStore::<TestItem>::new();
    let item2 = TestItem::new("item-2");

    workflow_fail.advance(&item2, &store_fail).await.unwrap();

    let status = store_fail.get_status(item2.id(), "a")
        .await.unwrap().unwrap();
    assert_eq!(status.state, StageState::AwaitingReview);
}
```

#### 10.3.9 Test 8: Feedback Threaded to Stage Context

```rust
#[tokio::test]
async fn test_feedback_threaded_to_stage_context() {
    let feedback_seen = Arc::new(Mutex::new(Vec::new()));
    let feedback_capture = feedback_seen.clone();

    /// A stage that captures the feedback from its context.
    #[derive(Debug)]
    struct FeedbackCapturingStage {
        feedback_seen: Arc<Mutex<Vec<Option<serde_json::Value>>>>,
    }

    #[async_trait]
    impl Stage<TestItem> for FeedbackCapturingStage {
        fn name(&self) -> &str {
            "capturing"
        }

        async fn execute(
            &self,
            _item: &TestItem,
            ctx: &StageContext,
        ) -> Result<StageOutput> {
            self.feedback_seen.lock().await.push(ctx.feedback.clone());
            Ok(StageOutput::completed_with(
                serde_json::json!({"attempt": ctx.attempt}),
            ))
        }
    }

    let workflow = Workflow::builder()
        .stage("a", FeedbackCapturingStage {
            feedback_seen: feedback_capture,
        })
        .quality_gate("a", AcceptOnAttemptGate { accept_on: 3 })
        .retry_budget("a", RetryBudget {
            max_attempts: 3,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        })
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    let seen = feedback_seen.lock().await;
    assert_eq!(seen.len(), 3);

    // Attempt 1: no feedback (first attempt)
    assert!(seen[0].is_none());

    // Attempt 2: feedback from the gate's rejection of attempt 1
    assert!(seen[1].is_some());
    let fb1 = seen[1].as_ref().unwrap();
    assert!(fb1["summary"].as_str().unwrap().contains("attempt 1"));

    // Attempt 3: feedback from the gate's rejection of attempt 2
    assert!(seen[2].is_some());
    let fb2 = seen[2].as_ref().unwrap();
    assert!(fb2["summary"].as_str().unwrap().contains("attempt 2"));
}
```

#### 10.3.10 Test 9: Events Emitted Correctly

```rust
#[tokio::test]
async fn test_events_emitted_correctly() {
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AcceptOnAttemptGate { accept_on: 2 })
        .retry_budget("a", RetryBudget {
            max_attempts: 3,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        })
        .build()
        .unwrap();

    let mut receiver = workflow.subscribe();
    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    // Collect all events
    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }

    // Expected sequence:
    // 1. StageStarted (attempt 1 implicit)
    // 2. QualityCheckFailed (attempt 1 rejected)
    // 3. RetryScheduled (attempt 2 scheduled)
    // 4. RetryAttempt (attempt 2 starting)
    // 5. QualityCheckPassed (attempt 2 accepted)
    // 6. StageCompleted

    // Verify QualityCheckFailed was emitted
    let failed_events: Vec<_> = events.iter().filter(|e| {
        matches!(e, WorkflowEvent::QualityCheckFailed { .. })
    }).collect();
    assert_eq!(failed_events.len(), 1);
    if let WorkflowEvent::QualityCheckFailed {
        attempt, feedback_summary, ..
    } = &failed_events[0] {
        assert_eq!(*attempt, 1);
        assert!(feedback_summary.contains("attempt 1"));
    }

    // Verify QualityCheckPassed was emitted
    let passed_events: Vec<_> = events.iter().filter(|e| {
        matches!(e, WorkflowEvent::QualityCheckPassed { .. })
    }).collect();
    assert_eq!(passed_events.len(), 1);
    if let WorkflowEvent::QualityCheckPassed { attempt, .. } = &passed_events[0] {
        assert_eq!(*attempt, 2);
    }

    // Verify RetryScheduled was emitted
    let retry_events: Vec<_> = events.iter().filter(|e| {
        matches!(e, WorkflowEvent::RetryScheduled { .. })
    }).collect();
    assert_eq!(retry_events.len(), 1);

    // Verify RetryAttempt was emitted
    let attempt_events: Vec<_> = events.iter().filter(|e| {
        matches!(e, WorkflowEvent::RetryAttempt { .. })
    }).collect();
    assert_eq!(attempt_events.len(), 1);
    if let WorkflowEvent::RetryAttempt {
        attempt, feedback_summary, ..
    } = &attempt_events[0] {
        assert_eq!(*attempt, 2);
        assert!(feedback_summary.is_some());
    }

    // Verify StageCompleted was emitted
    assert!(events.iter().any(|e| {
        matches!(e, WorkflowEvent::StageCompleted { stage, .. } if stage == "a")
    }));
}

#[tokio::test]
async fn test_escalated_event_emitted_on_uncertain() {
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", UncertainGate {
            reason: "Cannot determine quality".to_string(),
        })
        .build()
        .unwrap();

    let mut receiver = workflow.subscribe();
    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }

    let escalated: Vec<_> = events.iter().filter(|e| {
        matches!(e, WorkflowEvent::Escalated { .. })
    }).collect();
    assert_eq!(escalated.len(), 1);
    if let WorkflowEvent::Escalated { reason, .. } = &escalated[0] {
        assert_eq!(reason, "Cannot determine quality");
    }
}

#[tokio::test]
async fn test_escalated_event_emitted_on_exhaust() {
    let reject_feedback = QualityFeedback {
        summary: "bad".to_string(),
        failed_criteria: vec![],
        guidance: None,
    };

    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysRejectGate {
            feedback: reject_feedback,
        })
        .retry_budget("a", RetryBudget {
            max_attempts: 2,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Escalate,
        })
        .build()
        .unwrap();

    let mut receiver = workflow.subscribe();
    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    let mut events = Vec::new();
    while let Ok(event) = receiver.try_recv() {
        events.push(event);
    }

    // Should have 2 QualityCheckFailed events + 1 Escalated event
    let failed_count = events.iter()
        .filter(|e| matches!(e, WorkflowEvent::QualityCheckFailed { .. }))
        .count();
    assert_eq!(failed_count, 2);

    let escalated: Vec<_> = events.iter().filter(|e| {
        matches!(e, WorkflowEvent::Escalated { .. })
    }).collect();
    assert_eq!(escalated.len(), 1);
    if let WorkflowEvent::Escalated { reason, .. } = &escalated[0] {
        assert!(reason.contains("exhausted"));
    }
}
```

#### 10.3.11 Test 10: Stage Without Gate Behaves Like v1

```rust
#[tokio::test]
async fn test_stage_without_gate_behaves_like_v1() {
    // No quality gate, no retry budget, no review policy
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    // Stage should complete normally
    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::Completed);

    // Exactly 1 attempt recorded
    let attempts = store.get_attempts(item.id(), "a").await.unwrap();
    assert_eq!(attempts.len(), 1);

    // Verdict should be Accepted (implicit, no gate)
    assert!(matches!(
        attempts[0].quality_verdict,
        Some(SerializableVerdict::Accepted)
    ));
}
```

#### 10.3.12 Additional Edge Case Tests

```rust
#[tokio::test]
async fn test_quality_gate_with_multi_stage_pipeline() {
    // Verify that quality gates on one stage do not affect other stages
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .stage("b", SimpleStage)
        .dependency("b", "a")
        .quality_gate("a", AlwaysAcceptGate)
        // b has no quality gate
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    let status_a = store.get_status(item.id(), "a").await.unwrap().unwrap();
    let status_b = store.get_status(item.id(), "b").await.unwrap().unwrap();
    assert_eq!(status_a.state, StageState::Completed);
    assert_eq!(status_b.state, StageState::Completed);
}

#[tokio::test]
async fn test_quality_gate_retry_budget_default_is_one_attempt() {
    // Default retry budget: max_attempts=1, on_exhausted=Fail
    // If the gate rejects, the stage should fail immediately (no retry)
    let reject_feedback = QualityFeedback {
        summary: "rejected".to_string(),
        failed_criteria: vec![],
        guidance: None,
    };

    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysRejectGate {
            feedback: reject_feedback,
        })
        // No retry_budget call: uses default (max_attempts=1, Fail)
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::Failed);

    // Only 1 attempt
    let attempts = store.get_attempts(item.id(), "a").await.unwrap();
    assert_eq!(attempts.len(), 1);
}

#[tokio::test]
async fn test_review_policy_on_uncertain_with_accepted_verdict() {
    // ReviewPolicy::OnUncertain should NOT block when the verdict is Accepted
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysAcceptGate)
        .review_policy("a", ReviewPolicy::OnUncertain)
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::Completed);
}

#[tokio::test]
async fn test_review_policy_on_uncertain_blocks_on_uncertain() {
    // ReviewPolicy::OnUncertain SHOULD block when the verdict is Uncertain
    let workflow = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", UncertainGate {
            reason: "Ambiguous output".to_string(),
        })
        .review_policy("a", ReviewPolicy::OnUncertain)
        .build()
        .unwrap();

    let store = MemoryStateStore::<TestItem>::new();
    let item = TestItem::new("item-1");

    workflow.advance(&item, &store).await.unwrap();

    // Uncertain always escalates to review, and OnUncertain agrees
    let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
    assert_eq!(status.state, StageState::AwaitingReview);
}

#[tokio::test]
async fn test_review_policy_on_escalation_or_uncertain_covers_both() {
    // Scenario A: exhaustion -> AwaitingReview
    let reject_feedback = QualityFeedback {
        summary: "bad".to_string(),
        failed_criteria: vec![],
        guidance: None,
    };

    let workflow_exhaust = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", AlwaysRejectGate {
            feedback: reject_feedback,
        })
        .retry_budget("a", RetryBudget {
            max_attempts: 1,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        })
        .review_policy("a", ReviewPolicy::OnEscalationOrUncertain)
        .build()
        .unwrap();

    let store_a = MemoryStateStore::<TestItem>::new();
    let item_a = TestItem::new("item-a");

    workflow_exhaust.advance(&item_a, &store_a).await.unwrap();

    let status_a = store_a.get_status(item_a.id(), "a")
        .await.unwrap().unwrap();
    assert_eq!(status_a.state, StageState::AwaitingReview);

    // Scenario B: uncertain -> AwaitingReview
    let workflow_uncertain = Workflow::builder()
        .stage("a", SimpleStage)
        .quality_gate("a", UncertainGate {
            reason: "unsure".to_string(),
        })
        .review_policy("a", ReviewPolicy::OnEscalationOrUncertain)
        .build()
        .unwrap();

    let store_b = MemoryStateStore::<TestItem>::new();
    let item_b = TestItem::new("item-b");

    workflow_uncertain.advance(&item_b, &store_b).await.unwrap();

    let status_b = store_b.get_status(item_b.id(), "a")
        .await.unwrap().unwrap();
    assert_eq!(status_b.state, StageState::AwaitingReview);
}
```

### Verification Commands

```bash
cargo build --all-features
cargo test --all-features
cargo test --test quality_gate_integration
cargo clippy --all-features
```

---

## Phase 10 Completion Checklist

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes all tests (including all v1 and prior v2 phase tests)
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `QualityCheckPassed`, `QualityCheckFailed`, `Escalated` events added to `WorkflowEvent`
- [ ] `item_id()` and `stage()` accessors updated for new event variants
- [ ] No-op quality gate stub replaced with real `QualityGate::evaluate` calls
- [ ] `QualityVerdict::Accepted` path emits `QualityCheckPassed`, applies `ReviewPolicy`
- [ ] `QualityVerdict::Rejected` path emits `QualityCheckFailed`, retries or exhausts
- [ ] `QualityVerdict::Uncertain` path emits `Escalated`, immediately escalates to review
- [ ] `ReviewPolicy::Never` allows completion without review
- [ ] `ReviewPolicy::Always` blocks for review even when quality passes
- [ ] `ReviewPolicy::OnEscalation` blocks for review only when budget exhausted
- [ ] `ReviewPolicy::OnUncertain` blocks for review only on uncertain verdict
- [ ] `ReviewPolicy::OnEscalationOrUncertain` blocks on either condition
- [ ] `handle_exhausted` respects `ReviewPolicy` override of `ExhaustedAction::Fail`
- [ ] `QualityContext` correctly populated with attempt history
- [ ] Feedback threaded from quality gate rejection to `StageContext.feedback` on retry
- [ ] Stages without quality gates behave identically to v1
- [ ] Default retry budget (max_attempts=1, Fail) works correctly with quality gates
- [ ] End-to-end integration tests all pass
- [ ] All existing tests still pass

### Public API After Phase 10

```rust
// New in Phase 10
treadle::WorkflowEvent::QualityCheckPassed   // Quality gate accepted output
treadle::WorkflowEvent::QualityCheckFailed   // Quality gate rejected output
treadle::WorkflowEvent::Escalated            // Stage escalated to human review

// Modified in Phase 10
treadle::Workflow                             // execute_stage_with_retry now calls real gates
// (internal change; public API signature unchanged)

// Unchanged from prior phases (used by Phase 10)
treadle::QualityGate                         // Phase 7
treadle::QualityVerdict                      // Phase 7
treadle::QualityFeedback                     // Phase 7
treadle::QualityContext                      // Phase 7, updated Phase 9
treadle::RetryBudget                         // Phase 8
treadle::ExhaustedAction                     // Phase 8
treadle::ReviewPolicy                        // Phase 8
treadle::AttemptRecord                       // Phase 9
treadle::SerializableVerdict                 // Phase 9
```

---

## Notes on Rust Guidelines Applied

### Anti-Patterns Avoided
- **AP-01**: No boolean arguments in public API. `ReviewPolicy` uses named enum variants rather than boolean fields. The internal `requires_review` helper uses two booleans, but it is `pub(crate)` and tested exhaustively.
- **AP-03**: All types involved in async boundaries (`QualityGate`, `StageOutput`, `QualityContext`) are `Send + Sync`.
- **AP-09**: No `unwrap()` in library code. `serde_json::to_value(fb).unwrap_or_default()` uses a safe fallback. Event sends use `let _ = event_tx.send(...)` to ignore channel errors.
- **AP-12**: No unnecessary clones. `QualityFeedback` is cloned only when threading feedback to the next attempt (necessary because the verdict consumes the feedback). Event fields are `String` values created from `to_string()`, avoiding cloning large structures.

### Core Idioms Applied
- **ID-01**: `WorkflowEvent` is `#[non_exhaustive]`, allowing new variants without breaking downstream.
- **ID-04**: Builder pattern (`StageContext::new().with_attempt().with_feedback()`) used for context construction.
- **ID-11**: `Serialize`/`Deserialize` on `AttemptRecord` for persistence.
- **ID-35**: Builder methods consuming and returning `Self`.

### Error Handling
- **EH-07**: Quality gate evaluation errors (`gate.evaluate()` returning `Err`) propagate up via `?`. This is intentional: a gate that fails to evaluate (as opposed to returning `Rejected`) is a system error, not a quality judgement.
- Timeout is treated as a retryable failure with synthetic feedback, not a hard error.
- `ExhaustedAction::Fail` can be overridden by `ReviewPolicy::OnEscalation`, preventing data loss when operators want both fail semantics and escalation review.

### Concurrency
- Quality gate evaluation happens within the retry loop, which processes one item at a time through a stage. Different items may be evaluated concurrently (as documented on the `QualityGate` trait).
- `tokio::time::timeout` wraps stage execution only, not quality gate evaluation. If a quality gate hangs, that is a separate concern (addressed by the `attempt_timeout` wrapping the entire attempt in a future phase if needed).

### Testing Strategy
- Test gates (`AlwaysAcceptGate`, `AlwaysRejectGate`, `AcceptOnAttemptGate`, `UncertainGate`) provide deterministic, controllable behaviour.
- Tests cover all verdict types (Accepted, Rejected, Uncertain), all `ReviewPolicy` variants, budget exhaustion with both `ExhaustedAction` variants, feedback threading, event emission, and v1 backward compatibility.
- `Arc<Mutex<Vec<_>>>` captures state across async boundaries in tests without introducing flakiness.
- Integration tests are in a separate file (`tests/quality_gate_integration.rs`) to exercise the public API.
