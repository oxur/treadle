# Treadle Phase 9 Implementation Plan

## Phase 9: Retry Loop Machinery

**Goal:** Implement the retry loop within `advance()` — attempt counters, feedback threading, attempt timeout, and attempt history persistence. Quality gate evaluation is a **no-op pass-through** in this phase; the retry machinery operates but the judgement step always returns `Accepted`. This isolates testing of the retry/escalation mechanics from quality evaluation logic.

**Prerequisites:**
- Phases 1–8 completed
- `QualityGate` trait, `QualityVerdict`, `QualityFeedback`, `QualityContext` defined (Phase 7)
- `RetryBudget`, `ExhaustedAction`, `ReviewPolicy`, `ReviewOutcome` defined (Phase 8)
- `cargo build` and `cargo test` pass

---

## Milestone 9.1 — AttemptRecord and Attempt History in StateStore

### Files to Create/Modify
- `src/attempt.rs` (new)
- `src/state_store/mod.rs` (extend trait)
- `src/state_store/memory.rs` (implement)
- `src/state_store/sqlite.rs` (implement)
- `src/lib.rs` (add module, re-exports)

### Implementation Details

#### 9.1.1 Create `src/attempt.rs`

```rust
//! Attempt history types for retry tracking.
//!
//! Each time a stage executes (including retries), an [`AttemptRecord`] is
//! created capturing the output, quality verdict, and feedback. This history
//! is available to quality gates (via [`QualityContext`]) and to human
//! reviewers when a stage escalates.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::quality::{QualityFeedback, QualityVerdict};

/// A record of a single execution attempt for a stage.
///
/// Attempt records are persisted in the [`StateStore`](crate::StateStore)
/// and made available to quality gates and human reviewers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptRecord {
    /// Which attempt this was (1-indexed).
    pub attempt: u32,

    /// When this attempt started.
    pub started_at: DateTime<Utc>,

    /// When this attempt completed.
    pub completed_at: DateTime<Utc>,

    /// Human-readable summary of the stage output.
    pub output_summary: Option<String>,

    /// Serialisable summary of artefacts via `Artefact::to_json()`.
    ///
    /// This stores metadata/summaries only, never the artefact payload itself.
    pub artefacts: Option<serde_json::Value>,

    /// The quality verdict for this attempt, if a quality gate was run.
    pub quality_verdict: Option<SerializableVerdict>,

    /// Feedback from the quality gate, if the verdict was `Rejected`.
    pub feedback: Option<QualityFeedback>,
}

/// A serialisable representation of [`QualityVerdict`].
///
/// `QualityVerdict` itself contains `QualityFeedback` which is already
/// serialisable, but we need a flat representation for storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SerializableVerdict {
    /// Quality gate accepted the output.
    Accepted,
    /// Quality gate rejected the output.
    Rejected,
    /// Quality gate could not determine quality.
    Uncertain { reason: String },
}

impl From<&QualityVerdict> for SerializableVerdict {
    fn from(verdict: &QualityVerdict) -> Self {
        match verdict {
            QualityVerdict::Accepted => SerializableVerdict::Accepted,
            QualityVerdict::Rejected { .. } => SerializableVerdict::Rejected,
            QualityVerdict::Uncertain { reason } => SerializableVerdict::Uncertain {
                reason: reason.clone(),
            },
        }
    }
}

impl AttemptRecord {
    /// Creates a new attempt record with the given attempt number and
    /// the current time as `started_at`. Call [`complete`](Self::complete)
    /// when the attempt finishes.
    pub fn start(attempt: u32) -> Self {
        let now = Utc::now();
        Self {
            attempt,
            started_at: now,
            completed_at: now, // placeholder; updated by complete()
            output_summary: None,
            artefacts: None,
            quality_verdict: None,
            feedback: None,
        }
    }

    /// Marks this attempt as completed, recording the completion time.
    pub fn complete(mut self) -> Self {
        self.completed_at = Utc::now();
        self
    }

    /// Sets the output summary.
    pub fn with_output_summary(mut self, summary: impl Into<String>) -> Self {
        self.output_summary = Some(summary.into());
        self
    }

    /// Sets the artefact JSON summary.
    pub fn with_artefacts(mut self, artefacts: Option<serde_json::Value>) -> Self {
        self.artefacts = artefacts;
        self
    }

    /// Sets the quality verdict and feedback.
    pub fn with_verdict(mut self, verdict: &QualityVerdict) -> Self {
        self.quality_verdict = Some(SerializableVerdict::from(verdict));
        if let QualityVerdict::Rejected { feedback } = verdict {
            self.feedback = Some(feedback.clone());
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attempt_record_start() {
        let record = AttemptRecord::start(1);
        assert_eq!(record.attempt, 1);
        assert!(record.output_summary.is_none());
        assert!(record.artefacts.is_none());
        assert!(record.quality_verdict.is_none());
        assert!(record.feedback.is_none());
    }

    #[test]
    fn test_attempt_record_builder() {
        let record = AttemptRecord::start(2)
            .with_output_summary("Extracted 42 entities")
            .with_artefacts(Some(serde_json::json!({"count": 42})))
            .complete();

        assert_eq!(record.attempt, 2);
        assert_eq!(record.output_summary, Some("Extracted 42 entities".to_string()));
        assert!(record.artefacts.is_some());
        assert!(record.completed_at >= record.started_at);
    }

    #[test]
    fn test_attempt_record_with_accepted_verdict() {
        let record = AttemptRecord::start(1)
            .with_verdict(&QualityVerdict::Accepted)
            .complete();

        assert!(matches!(
            record.quality_verdict,
            Some(SerializableVerdict::Accepted)
        ));
        assert!(record.feedback.is_none());
    }

    #[test]
    fn test_attempt_record_with_rejected_verdict() {
        let feedback = QualityFeedback {
            summary: "Tables missing".to_string(),
            failed_criteria: vec![],
            guidance: None,
        };
        let verdict = QualityVerdict::Rejected {
            feedback: feedback.clone(),
        };
        let record = AttemptRecord::start(1).with_verdict(&verdict).complete();

        assert!(matches!(
            record.quality_verdict,
            Some(SerializableVerdict::Rejected)
        ));
        assert_eq!(record.feedback.as_ref().unwrap().summary, "Tables missing");
    }

    #[test]
    fn test_attempt_record_with_uncertain_verdict() {
        let verdict = QualityVerdict::Uncertain {
            reason: "Cannot assess automatically".to_string(),
        };
        let record = AttemptRecord::start(1).with_verdict(&verdict).complete();

        match &record.quality_verdict {
            Some(SerializableVerdict::Uncertain { reason }) => {
                assert_eq!(reason, "Cannot assess automatically");
            }
            _ => panic!("Expected Uncertain verdict"),
        }
    }

    #[test]
    fn test_attempt_record_serde_roundtrip() {
        let record = AttemptRecord::start(3)
            .with_output_summary("test")
            .with_artefacts(Some(serde_json::json!({"key": "value"})))
            .complete();

        let json = serde_json::to_string(&record).unwrap();
        let parsed: AttemptRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.attempt, 3);
        assert_eq!(parsed.output_summary, Some("test".to_string()));
    }
}
```

#### 9.1.2 Extend `StateStore` Trait

Add to `src/state_store/mod.rs`:

```rust
use crate::attempt::AttemptRecord;

// Add to the StateStore trait:

/// Records an execution attempt for a stage.
///
/// Attempts are appended to the history; they are never overwritten.
async fn record_attempt(
    &self,
    item_id: &W::Id,
    stage: &str,
    attempt: AttemptRecord,
) -> Result<()>;

/// Retrieves all execution attempts for a stage, ordered by attempt number.
async fn get_attempts(
    &self,
    item_id: &W::Id,
    stage: &str,
) -> Result<Vec<AttemptRecord>>;
```

#### 9.1.3 Implement in MemoryStateStore

Add to `Storage` struct:

```rust
/// Attempt history, keyed by (item_id, stage).
attempts: HashMap<StageKey, Vec<AttemptRecord>>,
```

Implementation:

```rust
async fn record_attempt(
    &self,
    item_id: &W::Id,
    stage: &str,
    attempt: AttemptRecord,
) -> Result<()> {
    let mut storage = self.storage.write().await;
    let key = StageKey::new(item_id, stage);
    storage.attempts.entry(key).or_default().push(attempt);
    Ok(())
}

async fn get_attempts(
    &self,
    item_id: &W::Id,
    stage: &str,
) -> Result<Vec<AttemptRecord>> {
    let storage = self.storage.read().await;
    let key = StageKey::new(item_id, stage);
    Ok(storage.attempts.get(&key).cloned().unwrap_or_default())
}
```

#### 9.1.4 Implement in SqliteStateStore

New table (schema version 3):

```sql
CREATE TABLE IF NOT EXISTS attempt_records (
    item_id TEXT NOT NULL,
    stage TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    started_at TEXT NOT NULL,
    completed_at TEXT NOT NULL,
    output_summary TEXT,
    artefacts TEXT,
    quality_verdict TEXT,
    feedback TEXT,
    PRIMARY KEY (item_id, stage, attempt)
);
```

Store `quality_verdict` and `feedback` as JSON strings.

#### 9.1.5 Tests for Attempt History

```rust
#[tokio::test]
async fn test_record_and_get_attempts() {
    let store = TestStore::new();
    let item_id = "item-1".to_string();

    let attempt1 = AttemptRecord::start(1)
        .with_output_summary("First try")
        .complete();
    let attempt2 = AttemptRecord::start(2)
        .with_output_summary("Second try")
        .complete();

    store.record_attempt(&item_id, "stage_a", attempt1).await.unwrap();
    store.record_attempt(&item_id, "stage_a", attempt2).await.unwrap();

    let attempts = store.get_attempts(&item_id, "stage_a").await.unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].attempt, 1);
    assert_eq!(attempts[1].attempt, 2);
}

#[tokio::test]
async fn test_get_attempts_empty() {
    let store = TestStore::new();
    let item_id = "item-1".to_string();

    let attempts = store.get_attempts(&item_id, "nonexistent").await.unwrap();
    assert!(attempts.is_empty());
}

#[tokio::test]
async fn test_attempts_isolated_by_stage() {
    let store = TestStore::new();
    let item_id = "item-1".to_string();

    store.record_attempt(&item_id, "stage_a", AttemptRecord::start(1).complete()).await.unwrap();
    store.record_attempt(&item_id, "stage_b", AttemptRecord::start(1).complete()).await.unwrap();

    let a_attempts = store.get_attempts(&item_id, "stage_a").await.unwrap();
    let b_attempts = store.get_attempts(&item_id, "stage_b").await.unwrap();

    assert_eq!(a_attempts.len(), 1);
    assert_eq!(b_attempts.len(), 1);
}
```

### Verification Commands

```bash
cargo build --all-features
cargo test attempt::tests
cargo test state_store
cargo clippy --all-features
```

---

## Milestone 9.2 — New Events: RetryScheduled and RetryAttempt

### Files to Modify
- `src/event.rs` (add new event variants)

### Implementation Details

#### 9.2.1 Add Event Variants

Add to `WorkflowEvent`:

```rust
/// A retry has been scheduled after a quality gate rejection.
RetryScheduled {
    /// The work item's identifier (stringified).
    item_id: String,
    /// The stage name.
    stage: String,
    /// The upcoming attempt number.
    attempt: u32,
    /// Maximum attempts allowed.
    max_attempts: u32,
},

/// A retry attempt is about to begin.
///
/// Emitted immediately before a retry begins, allowing operators
/// to observe progress in real time.
RetryAttempt {
    /// The work item's identifier (stringified).
    item_id: String,
    /// The stage name.
    stage: String,
    /// The attempt number about to start.
    attempt: u32,
    /// Maximum attempts allowed.
    max_attempts: u32,
    /// Summary of the feedback driving this retry.
    feedback_summary: Option<String>,
},
```

#### 9.2.2 Tests

```rust
#[test]
fn test_retry_scheduled_event() {
    let event = WorkflowEvent::RetryScheduled {
        item_id: "item-1".into(),
        stage: "pdf_to_md".into(),
        attempt: 2,
        max_attempts: 3,
    };
    let debug = format!("{:?}", event);
    assert!(debug.contains("RetryScheduled"));
    assert!(debug.contains("pdf_to_md"));
}

#[test]
fn test_retry_attempt_event() {
    let event = WorkflowEvent::RetryAttempt {
        item_id: "item-1".into(),
        stage: "pdf_to_md".into(),
        attempt: 2,
        max_attempts: 3,
        feedback_summary: Some("Tables lost in conversion".into()),
    };
    let debug = format!("{:?}", event);
    assert!(debug.contains("RetryAttempt"));
    assert!(debug.contains("Tables lost"));
}

#[test]
fn test_retry_attempt_event_no_feedback() {
    let event = WorkflowEvent::RetryAttempt {
        item_id: "item-1".into(),
        stage: "pdf_to_md".into(),
        attempt: 2,
        max_attempts: 3,
        feedback_summary: None,
    };
    let debug = format!("{:?}", event);
    assert!(debug.contains("None"));
}
```

### Verification Commands

```bash
cargo build
cargo test event
cargo clippy
```

---

## Milestone 9.3 — Retry Loop in advance() (Quality Gate as No-Op)

### Files to Modify
- `src/workflow.rs` (modify `advance()` method)

### Implementation Details

This is the core milestone. The `advance()` method is modified to implement the retry loop, but quality gate evaluation is stubbed as a no-op that always returns `Accepted`.

#### 9.3.1 Retry Loop Logic

The revised `advance()` for a single stage follows this flow:

```
1. Determine retry budget for stage (default: max_attempts=1)
2. Load attempt history from StateStore
3. Determine current attempt number
4. Loop:
   a. Build StageContext with attempt number and feedback
   b. Emit RetryAttempt event (if attempt > 1)
   c. If attempt_timeout is set, wrap execution in tokio::time::timeout
   d. Execute stage → StageOutput
   e. Record AttemptRecord in StateStore
   f. Run quality gate (STUB: always returns Accepted)
   g. If Accepted → apply ReviewPolicy, break
   h. If Rejected:
      - If attempt < max_attempts → emit RetryScheduled, increment, continue
      - If attempt >= max_attempts → check on_exhausted (Fail or Escalate)
   i. If Uncertain → escalate to review
5. Update stage status
```

#### 9.3.2 Pseudo-Code for the Retry Loop

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

    let mut attempt: u32 = 1;
    let mut feedback: Option<QualityFeedback> = None;

    loop {
        // Emit RetryAttempt event for retries
        if attempt > 1 {
            let _ = event_tx.send(WorkflowEvent::RetryAttempt {
                item_id: item.id().to_string(),
                stage: stage_name.to_string(),
                attempt,
                max_attempts: budget.max_attempts,
                feedback_summary: feedback.as_ref().map(|f| f.summary.clone()),
            });
        }

        // Build context
        let mut ctx = StageContext::new(item.id(), stage_name)
            .with_attempt(attempt);
        if let Some(ref fb) = feedback {
            ctx = ctx.with_feedback(serde_json::to_value(fb).unwrap_or_default());
        }
        // Populate upstream artefacts ...

        // Mark stage as running
        store.set_status(item.id(), stage_name, StageStatus::running()).await?;

        // Execute with optional timeout
        let started_at = Utc::now();
        let exec_result = if let Some(timeout_duration) = budget.attempt_timeout {
            match tokio::time::timeout(timeout_duration, stage.execute(item, &ctx)).await {
                Ok(result) => result,
                Err(_elapsed) => {
                    // Timeout: treat as a failed attempt
                    let record = AttemptRecord::start(attempt)
                        .with_output_summary("Attempt timed out")
                        .complete();
                    store.record_attempt(item.id(), stage_name, record).await?;

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
                    // Exhausted: fail or escalate
                    return self.handle_exhausted(
                        item, stage_name, &budget, store, event_tx,
                    ).await;
                }
            }
        } else {
            stage.execute(item, &ctx).await
        };

        // Handle execution result
        let output = match exec_result {
            Ok(output) => output,
            Err(e) => {
                // Stage execution error: record and handle
                let record = AttemptRecord::start(attempt)
                    .with_output_summary(format!("Error: {}", e))
                    .complete();
                store.record_attempt(item.id(), stage_name, record).await?;
                store.set_status(
                    item.id(),
                    stage_name,
                    StageStatus::failed(e.to_string()),
                ).await?;
                return Err(e);
            }
        };

        // Store artefact summary
        let artefact_json = output.artefacts.as_ref().and_then(|a| a.to_json());
        store.store_artefact_summary(item.id(), stage_name, artefact_json.clone()).await?;

        // Record attempt
        let mut record = AttemptRecord::start(attempt)
            .with_artefacts(artefact_json);
        if let Some(ref summary) = output.summary {
            record = record.with_output_summary(summary.clone());
        }

        // === QUALITY GATE STUB: always Accepted ===
        // In Phase 10, this will call the actual quality gate.
        let verdict = QualityVerdict::Accepted;
        record = record.with_verdict(&verdict);
        let record = record.complete();
        store.record_attempt(item.id(), stage_name, record).await?;

        // Handle verdict (stub always reaches Accepted path)
        match verdict {
            QualityVerdict::Accepted => {
                // Apply review policy
                match review_policy {
                    ReviewPolicy::Always => {
                        store.set_status(
                            item.id(),
                            stage_name,
                            StageStatus::awaiting_review(/* ... */),
                        ).await?;
                        return Ok(());
                    }
                    _ => {
                        store.set_status(
                            item.id(),
                            stage_name,
                            StageStatus::completed(),
                        ).await?;
                        return Ok(());
                    }
                }
            }
            QualityVerdict::Rejected { feedback: fb } => {
                // Retry or exhaust (will be exercised in Phase 10)
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
                return self.handle_exhausted(
                    item, stage_name, &budget, store, event_tx,
                ).await;
            }
            QualityVerdict::Uncertain { reason } => {
                // Escalate regardless of retry budget
                store.set_status(
                    item.id(),
                    stage_name,
                    StageStatus::awaiting_review(/* ... */),
                ).await?;
                let _ = event_tx.send(WorkflowEvent::Escalated {
                    item_id: item.id().to_string(),
                    stage: stage_name.to_string(),
                    reason,
                });
                return Ok(());
            }
        }
    }
}
```

#### 9.3.3 Handle Exhausted Helper

```rust
async fn handle_exhausted(
    &self,
    item: &W,
    stage_name: &str,
    budget: &RetryBudget,
    store: &dyn StateStore<W>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
) -> Result<()> {
    match budget.on_exhausted {
        ExhaustedAction::Fail => {
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
                StageStatus::awaiting_review(/* with attempt history context */),
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

#### 9.3.4 Tests for Retry Loop (with No-Op Gate)

Since the quality gate stub always returns `Accepted`, these tests verify:

1. **Single attempt succeeds** — stage executes once, status is `Completed`.
2. **Attempt counter incremented** — `StageContext.attempt` is 1 on first call.
3. **Artefact summary stored** — after execution, artefact JSON is in the state store.
4. **Attempt record persisted** — after execution, attempt history has one record.
5. **ReviewPolicy::Always blocks** — stage with `Always` policy enters `AwaitingReview`.
6. **Timeout triggers retry** — stage that sleeps longer than `attempt_timeout` retries.
7. **Timeout exhaustion fails** — stage that always times out, with `max_attempts: 2` and `ExhaustedAction::Fail`, ends in `Failed`.
8. **Timeout exhaustion escalates** — same but `ExhaustedAction::Escalate` ends in `AwaitingReview`.
9. **Events emitted** — verify `StageStarted`, `StageCompleted` (or `RetryAttempt`, `RetryScheduled`) events.

```rust
#[cfg(test)]
mod retry_loop_tests {
    use super::*;
    use std::time::Duration;

    // A stage that always completes with artefacts
    struct ArtefactStage;

    #[async_trait]
    impl Stage<TestItem> for ArtefactStage {
        fn name(&self) -> &str { "artefact_stage" }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutput> {
            Ok(StageOutput::completed_with(serde_json::json!({"count": 42})))
        }
    }

    // A stage that tracks attempt numbers
    struct AttemptTrackingStage {
        attempts_seen: Arc<Mutex<Vec<u32>>>,
    }

    #[async_trait]
    impl Stage<TestItem> for AttemptTrackingStage {
        fn name(&self) -> &str { "tracking_stage" }

        async fn execute(&self, _item: &TestItem, ctx: &StageContext) -> Result<StageOutput> {
            self.attempts_seen.lock().await.push(ctx.attempt);
            Ok(StageOutcome::Completed.into())
        }
    }

    // A slow stage for timeout testing
    struct SlowStage {
        delay: Duration,
    }

    #[async_trait]
    impl Stage<TestItem> for SlowStage {
        fn name(&self) -> &str { "slow_stage" }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutput> {
            tokio::time::sleep(self.delay).await;
            Ok(StageOutcome::Completed.into())
        }
    }

    #[tokio::test]
    async fn test_single_attempt_completes() {
        let workflow = Workflow::builder()
            .stage("a", ArtefactStage)
            .build()
            .unwrap();

        let store = MemoryStateStore::<TestItem>::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
        assert_eq!(status.state, StageState::Completed);
    }

    #[tokio::test]
    async fn test_attempt_record_persisted() {
        let workflow = Workflow::builder()
            .stage("a", ArtefactStage)
            .build()
            .unwrap();

        let store = MemoryStateStore::<TestItem>::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let attempts = store.get_attempts(item.id(), "a").await.unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].attempt, 1);
        assert!(attempts[0].artefacts.is_some());
    }

    #[tokio::test]
    async fn test_artefact_summary_stored() {
        let workflow = Workflow::builder()
            .stage("a", ArtefactStage)
            .build()
            .unwrap();

        let store = MemoryStateStore::<TestItem>::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let summary = store.get_artefact_summary(item.id(), "a").await.unwrap();
        assert_eq!(summary, Some(serde_json::json!({"count": 42})));
    }

    #[tokio::test]
    async fn test_review_policy_always_blocks() {
        let workflow = Workflow::builder()
            .stage("a", ArtefactStage)
            .review_policy("a", ReviewPolicy::Always)
            .build()
            .unwrap();

        let store = MemoryStateStore::<TestItem>::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let status = store.get_status(item.id(), "a").await.unwrap().unwrap();
        assert_eq!(status.state, StageState::AwaitingReview);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_timeout_triggers_failure_on_exhaustion() {
        let workflow = Workflow::builder()
            .stage("slow", SlowStage { delay: Duration::from_secs(10) })
            .retry_budget("slow", RetryBudget {
                max_attempts: 1,
                delay: None,
                attempt_timeout: Some(Duration::from_millis(100)),
                on_exhausted: ExhaustedAction::Fail,
            })
            .build()
            .unwrap();

        let store = MemoryStateStore::<TestItem>::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let status = store.get_status(item.id(), "slow").await.unwrap().unwrap();
        assert_eq!(status.state, StageState::Failed);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_timeout_escalates_on_exhaustion() {
        let workflow = Workflow::builder()
            .stage("slow", SlowStage { delay: Duration::from_secs(10) })
            .retry_budget("slow", RetryBudget {
                max_attempts: 1,
                delay: None,
                attempt_timeout: Some(Duration::from_millis(100)),
                on_exhausted: ExhaustedAction::Escalate,
            })
            .build()
            .unwrap();

        let store = MemoryStateStore::<TestItem>::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let status = store.get_status(item.id(), "slow").await.unwrap().unwrap();
        assert_eq!(status.state, StageState::AwaitingReview);
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_timeout_retry_then_succeed() {
        // A stage that times out on attempt 1 but succeeds on attempt 2
        struct ConditionalSlowStage;

        #[async_trait]
        impl Stage<TestItem> for ConditionalSlowStage {
            fn name(&self) -> &str { "conditional" }

            async fn execute(
                &self,
                _item: &TestItem,
                ctx: &StageContext,
            ) -> Result<StageOutput> {
                if ctx.attempt == 1 {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
                Ok(StageOutcome::Completed.into())
            }
        }

        let workflow = Workflow::builder()
            .stage("conditional", ConditionalSlowStage)
            .retry_budget("conditional", RetryBudget {
                max_attempts: 2,
                delay: None,
                attempt_timeout: Some(Duration::from_millis(100)),
                on_exhausted: ExhaustedAction::Fail,
            })
            .build()
            .unwrap();

        let store = MemoryStateStore::<TestItem>::new();
        let item = TestItem::new("item-1");

        workflow.advance(&item, &store).await.unwrap();

        let status = store.get_status(item.id(), "conditional").await.unwrap().unwrap();
        assert_eq!(status.state, StageState::Completed);

        let attempts = store.get_attempts(item.id(), "conditional").await.unwrap();
        assert_eq!(attempts.len(), 2);
    }
}
```

### Verification Commands

```bash
cargo build
cargo test retry_loop_tests
cargo clippy
```

---

## Milestone 9.4 — Update QualityContext with AttemptRecord

### Files to Modify
- `src/quality.rs` (update `QualityContext` to use `AttemptRecord`)

### Implementation Details

Now that `AttemptRecord` is defined, update the `QualityContext` placeholder:

```rust
use crate::attempt::AttemptRecord;

pub struct QualityContext {
    /// Which attempt this quality check is for.
    pub attempt: u32,
    /// Maximum attempts allowed by the retry budget.
    pub max_attempts: u32,
    /// Previous attempt records for this stage, if any.
    pub previous_attempts: Vec<AttemptRecord>,
    /// The stage name being evaluated.
    pub stage_name: String,
    /// The work item ID (stringified).
    pub item_id: String,
}
```

Update constructors and tests accordingly.

### Verification Commands

```bash
cargo build
cargo test quality
cargo clippy
```

---

## Phase 9 Completion Checklist

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes all tests (including all v1 and v2 type tests)
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `AttemptRecord` defined with builder methods and serde
- [ ] `StateStore` extended with `record_attempt` and `get_attempts`
- [ ] Both `MemoryStateStore` and `SqliteStateStore` implement attempt storage
- [ ] `RetryScheduled` and `RetryAttempt` events added
- [ ] `advance()` implements retry loop with attempt counter and feedback threading
- [ ] `attempt_timeout` wraps execution in `tokio::time::timeout`
- [ ] Quality gate stub always returns `Accepted`
- [ ] `ReviewPolicy::Always` correctly blocks for review
- [ ] Timeout scenarios tested (retry, exhaust+fail, exhaust+escalate)
- [ ] `QualityContext` updated with real `AttemptRecord` type
- [ ] All existing tests still pass

### Public API After Phase 9

```rust
// New in Phase 9
treadle::AttemptRecord          // Attempt history record
treadle::SerializableVerdict    // Serialisable quality verdict

// Modified in Phase 9
treadle::StateStore             // +record_attempt, +get_attempts
treadle::WorkflowEvent          // +RetryScheduled, +RetryAttempt
treadle::QualityContext         // previous_attempts now Vec<AttemptRecord>
```

---

## Notes on Rust Guidelines Applied

### Anti-Patterns Avoided
- AP-03: `Send + Sync` on all async-relevant types
- AP-09: No `unwrap()` in library code (timeout errors handled gracefully)
- AP-12: No unnecessary clones (artefact JSON cloned only when storing)

### Core Idioms Applied
- ID-04: Builder pattern on `AttemptRecord` with `with_*` methods
- ID-11: `Serialize`/`Deserialize` on `AttemptRecord` for persistence
- ID-35: Builder methods consuming and returning `Self`

### Error Handling
- EH-07: Stage execution errors recorded in attempt history
- Timeout treated as a retryable failure, not a hard error

### Concurrency
- `tokio::time::timeout` for attempt timeouts
- `tokio::time::sleep` for retry delays
- Paused-time tests (`start_paused = true`) for deterministic timeout testing
