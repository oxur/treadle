---
number: 2
title: "Treadle v2 — Design Document"
author: "the stage"
component: All
tags: [change-me]
created: 2026-02-07
updated: 2026-02-07
state: Under Review
supersedes: null
superseded-by: null
version: 1.0
---

# Treadle v2 — Design Document

**Status:** Draft v2.1
**Context:** Generalising Treadle beyond single-pass DAG execution to support AI-agent workflows with iterative quality refinement, configurable review policies, and structured feedback loops.
**Revision Note:** Incorporates review feedback from `TREADLE_V2_DESIGN_REVIEW.md`. Changes from v2.0 are marked with "(v2.1)" in section headings where applicable.

---

## Motivating Use Cases

### Use Case A: Media Library Taxonomy (v1 target)

A musicological cataloguing tool. Scan files → identify metadata → enrich from external sources → human review → export. Stages are largely deterministic. Human review is a single gate. The pipeline is linear with occasional fan-out (multiple enrichment sources).

### Use Case B: AI Document Processing (v2 target)

Claude Code processes a corpus of PDFs through a multi-stage pipeline:

1. **PDF → Markdown conversion** — Extract text, preserve structure. Quality varies; some PDFs produce garbage and need retry with different extraction strategies.
2. **Markdown → Concept graph** — Identify entities, relationships, and concepts. Output must meet structural requirements (minimum connectivity, no orphan nodes, concepts properly typed).
3. **Markdown → Vector DB entries** — Generate embeddings and chunk documents. Chunking strategy affects retrieval quality and may need iteration.
4. **Markdown → FTS index entries** — Extract searchable text with metadata.
5. **Cross-referencing** — Link concept graph nodes across documents.

Key differences from Use Case A:

- **Stages produce outputs that vary in quality.** A stage can mechanically succeed (no crash, valid output) but produce inadequate results.
- **Quality is evaluated against explicit criteria.** "The concept graph must have at least 5 nodes," "The markdown must preserve all tables from the PDF," "The chunks must each be under 512 tokens."
- **Retry with feedback.** When quality isn't met, the stage should re-execute with structured feedback about what was wrong: "Tables were lost in conversion — try extraction strategy B." This isn't the same as retrying a network timeout.
- **Bounded retry → human escalation.** After N attempts, if quality still isn't met, escalate to a human reviewer rather than looping forever.
- **Some stages always require human review.** Regardless of automated quality assessment, certain stages (e.g., final concept graph review) always pause for human sign-off.
- **The agent doing the work may be external.** Claude Code is called via an API or subprocess, not a Rust function. Stage implementations may be thin wrappers around external tool invocations.

---

## What v1 Already Handles

- DAG structure and topological execution ✅
- Persistent state across restarts ✅
- Human review gates (`AwaitingReview`) ✅
- Fan-out with per-subtask tracking ✅
- Event stream for observability ✅

## What v1 Cannot Express

1. **Quality evaluation as a separate concern.** Stages evaluate their own success via `StageOutcome`. There's no way to attach an external quality check.
2. **Iterative refinement.** A stage runs once. If it fails, it's marked `Failed`. There's no concept of "try again with this feedback."
3. **Review policies.** Whether human review happens is decided by the stage returning `AwaitingReview`. There's no way to configure "this stage always requires review" or "this stage escalates to review after 3 failed quality checks" at the workflow level.
4. **Structured feedback between attempts.** `StageContext` has an `attempt: u32` counter but no feedback from previous attempts.
5. **Stage output capture.** Stages return an `Outcome` enum, not the actual artefacts they produced. There's no built-in way to pass a stage's output to a quality evaluator or to downstream stages.

---

## Proposed Design

### Principle: Separate Work, Judgement, and Policy

The v1 `Stage` trait conflates three concerns:

- **Doing the work** (execute the stage logic)
- **Judging the result** (did it work well enough?)
- **Deciding what to do next** (retry? escalate? continue?)

V2 separates these into three composable pieces:

| Concern | v1 | v2 |
|---------|----|----|
| Do the work | `Stage::execute` returns `StageOutcome` | `Stage::execute` returns `StageOutput` (the actual result) |
| Judge quality | Implicit in outcome choice | `QualityGate::evaluate` returns `QualityVerdict` |
| Decide next action | Hardcoded (complete/fail/review) | `ReviewPolicy` configuration per stage |

### Artefact Trait (v2.1)

Artefacts use a trait rather than `serde_json::Value`, cleanly separating "can this be persisted?" from "can this be inspected by a quality gate?" The trait provides opt-in serialisation for persistence and review UIs, and type-safe downcasting for quality gates and downstream stages that know the expected artefact type:

```rust
pub trait Artefact: Send + Sync + 'static {
    /// Attempts to serialise to JSON. Returns `None` if this artefact
    /// is opaque (binary, large, or not meaningfully serialisable).
    fn to_json(&self) -> Option<serde_json::Value> {
        None  // default: not serialisable
    }

    /// Downcasts to a concrete type. Used by quality gates and
    /// downstream stages that know the expected artefact type.
    fn as_any(&self) -> &dyn std::any::Any;
}
```

A blanket implementation covers `serde_json::Value` directly, and a wrapper type (`SerializableArtefact<T: Serialize>`) supports any `Serialize` type. This gives:

- Zero-cost opaque artefacts (no serialisation overhead)
- Opt-in JSON serialisation for persistence and review UIs
- Type-safe downcasting for quality gates that know what they're evaluating
- No forced allocation for stages that don't produce artefacts

**Artefact storage boundary:** The framework stores *artefact metadata and summaries* (via `Artefact::to_json()`) in the `StateStore`, never the artefact payload itself. If a stage produces a 50MB markdown file, the framework stores `None` (or a reference/metadata JSON) in the `AttemptRecord`, and the consuming application manages the actual file. The `Artefact` trait's `to_json()` method is the bridge: stages decide what summary goes into the attempt record. This boundary prevents large blobs from accumulating in SQLite and keeps the framework agnostic about storage strategy.

### Core Type Changes

#### StageOutput (new)

Stages stop returning a bare enum and instead return their actual output, wrapped in a type the engine can thread through evaluation and feedback:

```rust
pub struct StageOutput {
    /// What happened mechanically.
    pub outcome: StageOutcome,

    /// The artefact(s) this stage produced, if any.
    /// Opaque to the engine; meaningful to quality gates and downstream stages.
    pub artefacts: Option<Box<dyn Artefact>>,

    /// Human-readable summary of what the stage did.
    pub summary: Option<String>,
}
```

**Note on trait implementations:** Because `Box<dyn Artefact>` does not implement `Clone` or `PartialEq`, `StageOutput` cannot derive these traits. This is acceptable — the engine consumes `StageOutput` values; it does not compare or duplicate them. `StageOutput` implements `Debug` via a manual implementation that delegates to the artefact's debug representation.

`StageOutcome` retains its existing variants (`Completed`, `Skipped`, `FanOut`, `AwaitingReview`) but `AwaitingReview` becomes less common — most stages just return `Completed` with their artefacts, and the review policy decides whether to block.

#### Stage trait (revised)

```rust
#[async_trait]
pub trait Stage<W: WorkItem>: Send + Sync {
    fn name(&self) -> &str;

    async fn execute(&self, item: &W, ctx: &StageContext) -> Result<StageOutput>;
}
```

The signature barely changes — `StageOutput` replaces `StageOutcome`. Existing v1 stages that don't produce artefacts just return `StageOutput { outcome: StageOutcome::Completed, artefacts: None, summary: None }`. A `From<StageOutcome>` impl makes this painless.

#### StageContext (extended)

```rust
pub struct StageContext {
    pub item_id: String,
    pub stage_name: String,
    pub attempt: u32,
    pub subtask_name: Option<String>,

    // --- New in v2 ---

    /// Feedback from the quality gate on the previous attempt.
    /// `None` on the first attempt.
    pub feedback: Option<QualityFeedback>,

    /// Artefacts from upstream stages, keyed by stage name.
    /// Populated by the engine based on the DAG structure.
    pub upstream_artefacts: HashMap<String, Box<dyn Artefact>>,
}
```

This is the key mechanism for iterative refinement: when a stage is re-invoked after a quality gate rejection, `feedback` contains structured information about what was wrong. The stage implementation decides how to use this. For an AI agent wrapper, it would be injected into the prompt.

`upstream_artefacts` solves a second problem: stages that depend on the output of prior stages (concept graph extraction needs the markdown output from PDF conversion). In v1 this is handled out-of-band; v2 makes it explicit.

### QualityGate Trait (new)

```rust
/// A quality gate evaluates the output of a stage and returns a verdict.
///
/// # Concurrency
///
/// Implementations must be safe to call concurrently for *different* work
/// items. The executor does not call `evaluate` concurrently for the *same*
/// item through the same stage — each item proceeds sequentially through
/// the retry loop. However, multiple items may be advancing through the
/// pipeline simultaneously, so shared state (connection pools, API clients,
/// caches) must be thread-safe.
#[async_trait]
pub trait QualityGate<W: WorkItem>: Send + Sync {
    async fn evaluate(
        &self,
        item: &W,
        stage: &str,
        output: &StageOutput,
        ctx: &QualityContext,
    ) -> Result<QualityVerdict>;
}
```

**Note on generics:** The `QualityGate` trait is generic over `W: WorkItem`, meaning quality gates are tied to a specific work item type. This is by design — a gate for documents isn't reusable for audio files, and the type system enforces this constraint.

**Composition intent:** The `QualityGate` trait is designed to support composition (e.g., `gate_a.and(gate_b)` or `gate_a.or(gate_b)` via a `CompositeGate` wrapper). The trait signature — returning a `QualityVerdict` — is sufficient for `and`/`or` wrappers, and the builder API accepts `impl QualityGate<W> + 'static` (via trait objects), so a future `CompositeGate` drops in without API changes. Composition support is planned but deferred from the initial v2.0 implementation.

A quality gate is a separate, composable check attached to a stage. It receives the stage's output and returns a verdict:

```rust
pub enum QualityVerdict {
    /// Output meets all quality criteria. Proceed.
    Accepted,

    /// Output doesn't meet criteria. Includes structured feedback
    /// for the next attempt.
    Rejected {
        feedback: QualityFeedback,
    },

    /// Quality gate can't determine quality automatically.
    /// Escalate to human review regardless of retry budget.
    Uncertain {
        reason: String,
    },
}

pub struct QualityFeedback {
    /// Overall assessment.
    pub summary: String,

    /// Specific criteria that weren't met.
    pub failed_criteria: Vec<CriterionResult>,

    /// Structured data the stage can use to adjust its approach.
    pub guidance: Option<serde_json::Value>,
}

pub struct CriterionResult {
    /// Name of the criterion (e.g., "table_preservation").
    pub name: String,

    /// What was expected.
    pub expected: String,

    /// What was observed.
    pub actual: String,

    /// Whether this criterion was met.
    pub passed: bool,
}
```

#### QualityContext (v2.1)

The `QualityContext` provides attempt history and metadata to quality gates, enabling decisions based on prior attempts (e.g., "the last two attempts both failed on table extraction, so escalate now rather than retrying again"):

```rust
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

**Quality gates are optional.** A stage with no quality gate behaves exactly like v1 — it completes (or fails), and the engine moves on. Quality gates are attached at the workflow builder level, not baked into the stage itself:

```rust
let workflow = Workflow::builder()
    .stage("pdf_to_markdown", PdfToMarkdownStage::new())
    .stage("extract_concepts", ConceptExtractionStage::new())
    .quality_gate("pdf_to_markdown", MarkdownQualityCheck::new(requirements))
    .quality_gate("extract_concepts", ConceptGraphValidator::new(min_nodes: 5))
    .dependency("extract_concepts", "pdf_to_markdown")
    .build()?;
```

This keeps stages focused on doing work, quality gates focused on evaluating work, and the workflow definition as the place where they're composed.

### ReviewPolicy (revised in v2.1)

Each stage can have a review policy that governs when human review is triggered:

```rust
pub enum ReviewPolicy {
    /// Never require human review (default). Stage completes when
    /// execution succeeds and quality gate passes.
    Never,

    /// Always require human review after execution, even if the
    /// quality gate passes. The human sees the output and the
    /// quality assessment.
    Always,

    /// Require human review only when the quality gate rejects
    /// the output and the retry budget is exhausted.
    OnEscalation,

    /// Require human review only when the quality gate returns
    /// Uncertain.
    OnUncertain,

    /// Require human review on escalation or when the quality gate
    /// returns Uncertain.
    OnEscalationOrUncertain,
}
```

The original `Custom` variant with three boolean fields has been removed. The five named variants cover all meaningful combinations without the ambiguity of boolean flags (per TD-01: arguments use types, not bools). If new combinations are needed in future, they can be added as named variants.

Configured at the workflow builder level:

```rust
.review_policy("extract_concepts", ReviewPolicy::OnEscalation)
.review_policy("final_review", ReviewPolicy::Always)
```

### ReviewOutcome (v2.1)

Human reviewers can approve, reject, or approve with edits. This replaces the v1 `approve_review`/`reject_review` methods with a unified `resolve_review` method:

```rust
pub enum ReviewOutcome {
    /// Approve the output as-is.
    Approve,

    /// Reject the output. The stage is marked failed.
    Reject { reason: String },

    /// Approve with modifications. The pipeline continues with
    /// the edited output instead of the original.
    ApproveWithEdits {
        /// The modified artefacts replacing the stage's original output.
        edited_artefacts: Box<dyn Artefact>,
        /// Optional note about what was changed.
        note: Option<String>,
    },
}
```

And a method on `Workflow`:

```rust
pub async fn resolve_review(
    &self,
    item_id: &W::Id,
    stage: &str,
    outcome: ReviewOutcome,
    store: &dyn StateStore<W>,
) -> Result<()>
```

The existing v1 `approve_review` and `reject_review` methods become convenience wrappers around `resolve_review`:

```rust
pub async fn approve_review(&self, ...) -> Result<()> {
    self.resolve_review(item_id, stage, ReviewOutcome::Approve, store).await
}

pub async fn reject_review(&self, ..., reason: String) -> Result<()> {
    self.resolve_review(item_id, stage, ReviewOutcome::Reject { reason }, store).await
}
```

### RetryBudget (revised in v2.1)

Controls how many quality-gate-driven retries a stage gets before escalating:

```rust
pub struct RetryBudget {
    /// Maximum attempts before escalation. Includes the initial attempt.
    /// So max_attempts: 3 means 1 initial + 2 retries.
    pub max_attempts: u32,

    /// Optional delay between attempts.
    pub delay: Option<std::time::Duration>,

    /// Optional timeout per attempt. If set, the executor wraps each
    /// stage execution + quality gate evaluation in a `tokio::time::timeout`.
    /// A timed-out attempt counts as a failed attempt with appropriate
    /// feedback ("attempt timed out after Xms").
    pub attempt_timeout: Option<std::time::Duration>,

    /// What to do when the budget is exhausted.
    pub on_exhausted: ExhaustedAction,
}

pub enum ExhaustedAction {
    /// Fail the stage.
    Fail,

    /// Escalate to human review. The human sees all attempts,
    /// their quality assessments, and can approve the best one
    /// or request manual intervention.
    Escalate,
}
```

Default: `RetryBudget { max_attempts: 1, delay: None, attempt_timeout: None, on_exhausted: ExhaustedAction::Fail }` — which is exactly v1 behaviour (one attempt, fail on error, no escalation, no timeout).

The `attempt_timeout` field is critical for production AI-agent workflows, where LLM calls can take arbitrarily long or fail silently. Without it, a hung stage blocks the entire pipeline indefinitely.

Configured per stage:

```rust
.retry_budget("pdf_to_markdown", RetryBudget {
    max_attempts: 3,
    delay: None,
    attempt_timeout: Some(std::time::Duration::from_secs(120)),
    on_exhausted: ExhaustedAction::Escalate,
})
```

### Revised Execution Flow

Here is the new execution loop for a single stage, showing how work, judgement, and policy compose:

```
┌─────────────────────────────────────────────────────────────┐
│                    Execute Stage                            │
│                                                             │
│  attempt = 1                                                │
│  feedback = None                                            │
│                                                             │
│  loop:                                                      │
│    ┌───────────────────────────────────┐                    │
│    │  ctx = StageContext {             │                    │
│    │    attempt,                       │                    │
│    │    feedback,                      │                    │
│    │    upstream_artefacts,            │                    │
│    │  }                                │                    │
│    │  output = stage.execute(item,ctx) │                    │
│    └───────────────┬───────────────────┘                    │
│                    │                                        │
│             ┌──────v──────┐                                 │
│             │ Has quality │──── No ───> apply ReviewPolicy  │
│             │   gate?     │           (if Always → review,  │
│             └─────┬───────┘            else → Completed)    │
│                   │ Yes                                     │
│             ┌─────v──────────────┐                          │
│             │ gate.evaluate(     │                          │
│             │   item, output)    │                          │
│             └─────┬──────────────┘                          │
│                   │                                         │
│         ┌─────────┼──────────┐                              │
│         v         v          v                              │
│     Accepted   Rejected   Uncertain                         │
│         │         │          │                              │
│         │         │          └──> Escalate to human review  │
│         │         │                                         │
│         │    attempt < max?                                 │
│         │     ┌───┴─────┐                                   │
│         │    Yes        No                                  │
│         │     │         │                                   │
│         │     │    on_exhausted?                            │
│         │     │    ┌────┴─────┐                             │
│         │     │   Fail     Escalate                         │
│         │     │    │          │                             │
│         │     │    v          v                             │
│         │     │  Failed   AwaitingReview                    │
│         │     │           (with all attempts visible)       │
│         │     │                                             │
│         │     └──> attempt += 1                             │
│         │          feedback = gate's feedback               │
│         │          continue loop                            │
│         │                                                   │
│         └──> apply ReviewPolicy                             │
│             (if Always → review with quality report,        │
│              else → Completed)                              │
└─────────────────────────────────────────────────────────────┘
```

### Persistence: Attempt History

The state store needs to record the history of attempts, not just the latest status. New table/structure:

```rust
pub struct AttemptRecord {
    pub attempt: u32,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    pub output_summary: Option<String>,
    /// Serialisable summary of artefacts via `Artefact::to_json()`.
    /// This stores metadata/summaries only, never the artefact payload itself.
    pub artefacts: Option<serde_json::Value>,
    pub quality_verdict: Option<QualityVerdict>,
    pub feedback: Option<QualityFeedback>,
}
```

Added to `StateStore` trait:

```rust
async fn record_attempt(&self, item_id: &W::Id, stage: &str, attempt: AttemptRecord) -> Result<()>;
async fn get_attempts(&self, item_id: &W::Id, stage: &str) -> Result<Vec<AttemptRecord>>;
```

This is critical for human reviewers. When a stage escalates after 3 attempts, the reviewer can see all three attempts, what the quality gate said about each, and make an informed decision about which output to accept or whether to intervene manually.

### New Events (revised in v2.1)

```rust
pub enum WorkflowEvent {
    // ... existing events ...

    // New in v2
    QualityCheckPassed { item_id: String, stage: String, attempt: u32 },
    QualityCheckFailed { item_id: String, stage: String, attempt: u32, feedback_summary: String },
    RetryScheduled { item_id: String, stage: String, attempt: u32, max_attempts: u32 },
    /// Emitted before each retry attempt begins, for observability.
    RetryAttempt {
        item_id: String,
        stage: String,
        attempt: u32,
        max_attempts: u32,
        feedback_summary: Option<String>,
    },
    Escalated { item_id: String, stage: String, reason: String },
}
```

The `RetryAttempt` event is emitted immediately before a retry begins, allowing operators watching the event stream to see progress in real time: "stage X is being retried (attempt 2/3) because: tables were lost in conversion."

---

## Backward Compatibility

The design is backward-compatible with v1:

| v1 behaviour | v2 equivalent |
|--------------|---------------|
| Stage returns `Completed` | Stage returns `StageOutput::from(Completed)`, no quality gate, `ReviewPolicy::Never` |
| Stage returns `AwaitingReview` | Stage returns `Completed` with `ReviewPolicy::Always` — OR stage still returns `AwaitingReview` (both work) |
| Stage returns `Failed` | Unchanged |
| No retry | `RetryBudget::default()` (max_attempts: 1, on_exhausted: Fail) |
| `StageContext.attempt` existed but was unused | Now populated and meaningful |

A v1 stage implementation compiles and works identically in v2. The `From<StageOutcome> for StageOutput` impl handles the conversion. Quality gates, review policies, and retry budgets are all opt-in additions at the workflow builder level.

---

## API Example: Claude Code Document Pipeline

```rust
use treadle::*;

// Quality gate: checks that markdown preserves tables from source PDF
struct MarkdownQualityCheck {
    require_tables: bool,
    min_word_count: usize,
}

#[async_trait]
impl<W: WorkItem> QualityGate<W> for MarkdownQualityCheck {
    async fn evaluate(
        &self,
        _item: &W,
        _stage: &str,
        output: &StageOutput,
        _ctx: &QualityContext,
    ) -> Result<QualityVerdict> {
        // Downcast the artefact to our expected type
        let markdown_text = output.artefacts.as_ref()
            .and_then(|a| a.as_any().downcast_ref::<MarkdownOutput>())
            .map(|m| m.text.as_str())
            .ok_or_else(|| Error::msg("Expected MarkdownOutput artefact"))?;

        let mut failed = vec![];

        let word_count = markdown_text.split_whitespace().count();
        if word_count < self.min_word_count {
            failed.push(CriterionResult {
                name: "word_count".into(),
                expected: format!("≥ {}", self.min_word_count),
                actual: format!("{word_count}"),
                passed: false,
            });
        }

        if self.require_tables && !markdown_text.contains("|") {
            failed.push(CriterionResult {
                name: "table_preservation".into(),
                expected: "Tables present in markdown".into(),
                actual: "No tables found".into(),
                passed: false,
            });
        }

        if failed.is_empty() {
            Ok(QualityVerdict::Accepted)
        } else {
            Ok(QualityVerdict::Rejected {
                feedback: QualityFeedback {
                    summary: format!("{} criteria not met", failed.len()),
                    failed_criteria: failed,
                    guidance: Some(serde_json::json!({
                        "hint": "Try using OCR-based extraction for table recovery"
                    })),
                },
            })
        }
    }
}

// Build the workflow
let workflow = Workflow::builder()
    // Stages
    .stage("pdf_to_markdown", PdfToMarkdown::new())
    .stage("extract_concepts", ConceptExtractor::new())
    .stage("generate_embeddings", EmbeddingGenerator::new())
    .stage("build_fts_index", FtsIndexBuilder::new())
    .stage("cross_reference", CrossReferencer::new())
    .stage("final_review", HumanReviewStage::new())

    // Dependencies
    .dependency("extract_concepts", "pdf_to_markdown")
    .dependency("generate_embeddings", "pdf_to_markdown")
    .dependency("build_fts_index", "pdf_to_markdown")
    .dependency("cross_reference", "extract_concepts")
    .dependency("final_review", "cross_reference")
    .dependency("final_review", "generate_embeddings")
    .dependency("final_review", "build_fts_index")

    // Quality gates
    .quality_gate("pdf_to_markdown", MarkdownQualityCheck {
        require_tables: true,
        min_word_count: 100,
    })
    .quality_gate("extract_concepts", ConceptGraphValidator {
        min_nodes: 5,
        require_connected: true,
    })

    // Retry budgets
    .retry_budget("pdf_to_markdown", RetryBudget {
        max_attempts: 3,
        delay: None,
        attempt_timeout: Some(std::time::Duration::from_secs(120)),
        on_exhausted: ExhaustedAction::Escalate,
    })
    .retry_budget("extract_concepts", RetryBudget {
        max_attempts: 2,
        delay: None,
        attempt_timeout: Some(std::time::Duration::from_secs(60)),
        on_exhausted: ExhaustedAction::Escalate,
    })

    // Review policies
    .review_policy("final_review", ReviewPolicy::Always)
    .review_policy("pdf_to_markdown", ReviewPolicy::OnEscalation)
    .review_policy("extract_concepts", ReviewPolicy::OnEscalation)

    .build()?;
```

The pipeline DAG looks like:

```
                  ┌─▶ extract_concepts ─▶ cross_reference ─┐
pdf_to_markdown ──┼─▶ generate_embeddings ─────────────────┼─▶ final_review
                  └─▶ build_fts_index ─────────────────────┘
```

Stages with quality gates automatically retry on rejection. If retries are exhausted, the item escalates to human review with the full attempt history visible. The final review stage always blocks for human sign-off.

---

## Implementation Phases (v2 additions on top of v1)

### Phase 6: StageOutput and Artefact Passing

- Define the `Artefact` trait with `to_json()` and `as_any()` methods.
- Provide blanket impl for `serde_json::Value` and a `SerializableArtefact<T>` wrapper.
- Change `Stage::execute` return type from `StageOutcome` to `StageOutput`.
- Add `From<StageOutcome> for StageOutput` for backward compatibility.
- Add `upstream_artefacts` to `StageContext`.
- Engine populates `upstream_artefacts` from completed upstream stages' stored artefacts.
- Add artefact storage to `StateStore` trait and both implementations.
- Implement manual `Debug` for `StageOutput`.
- Existing tests continue to pass unchanged.

### Phase 7: Quality Gate Framework

- Define `QualityGate` trait (with concurrency doc comment), `QualityVerdict`, `QualityFeedback`, `CriterionResult`.
- Define `QualityContext` struct (attempt, max_attempts, previous_attempts, stage_name, item_id).
- Add `.quality_gate()` to `WorkflowBuilder`.
- Store quality gate references in `Workflow` alongside stages.
- No execution changes yet — just the type system and builder.

### Phase 8: Retry Budget, Review Policy, and Review Outcome

- Define `RetryBudget` (with `attempt_timeout`), `ExhaustedAction`, `ReviewPolicy` (five named variants, no `Custom`).
- Define `ReviewOutcome` enum (Approve, Reject, ApproveWithEdits).
- Add `resolve_review` method to `Workflow`; make `approve_review`/`reject_review` convenience wrappers.
- Add `.retry_budget()` and `.review_policy()` to `WorkflowBuilder`.
- Defaults match v1 behaviour (1 attempt, fail on error, no review policy, no timeout).

### Phase 9: Retry Loop Machinery

- Modify `advance()` to implement the retry loop with attempt counters and feedback threading.
- Add `AttemptRecord` and attempt history to `StateStore`.
- Thread `QualityFeedback` into `StageContext.feedback` on retry.
- Implement `attempt_timeout` via `tokio::time::timeout` wrapping.
- Add `RetryScheduled` and `RetryAttempt` events.
- Quality gate evaluation is a **no-op pass-through** in this phase — the retry machinery operates but the judgement step always returns `Accepted`. This tests the retry/escalation machinery in isolation.

### Phase 10: Quality Gate Integration

- Wire quality gate evaluation into the retry loop from Phase 9.
- The judgement step now actually runs `QualityGate::evaluate`, and `QualityVerdict` drives the retry/escalate decision.
- Apply `ReviewPolicy` to determine whether to block for review.
- New events: `QualityCheckPassed`, `QualityCheckFailed`, `Escalated`.
- End-to-end test: stage with quality gate, 2 retries, escalation to review.
- End-to-end test: `ReviewPolicy::Always` stage.

### Phase 11: Integration Tests and Documentation

- End-to-end test: pipeline mixing v1-style stages (no gates) and v2-style stages (with gates).
- End-to-end test: `ApproveWithEdits` review outcome flowing through the pipeline.
- End-to-end test: `attempt_timeout` triggering on a slow stage.
- Update crate documentation and examples.
- `examples/quality_pipeline.rs` demonstrating the Claude Code document processing use case.

---

## v1 Implementation Note: `unsafe impl Send/Sync`

The v1 implementation plan includes `unsafe impl Send for Workflow<W>` and `unsafe impl Sync for Workflow<W>`. Per AP-01 and the crate's own `#![forbid(unsafe_code)]`, this is incorrect. The `Arc<dyn Stage<W>>` inside `Workflow` is already `Send + Sync` (due to the `Stage` trait bounds), so `Workflow` should derive `Send + Sync` automatically. If it doesn't compile without the `unsafe impl`, the fix is to add the correct bounds to the fields (e.g., ensuring `DiGraph<RegisteredStage<W>, ()>` is `Send + Sync`), not to use `unsafe`. These lines must be removed before v2 work begins.

---

## Open Questions (Resolved)

1. **Artefact type** — Resolved: use an `Artefact` trait with `to_json()` for opt-in serialisation and `as_any()` for type-safe downcasting. See "Artefact Trait" section above.

2. **Quality gates async?** — Resolved: yes. The motivating use case (LLM evaluation) makes this essential.

3. **Human reviewers editing artefacts?** — Resolved: yes, via a `ReviewOutcome` enum (`Approve | Reject | ApproveWithEdits`) and a unified `resolve_review` method. See "ReviewOutcome" section above.

4. **Quality gate composition?** — Resolved: yes in principle, deferred in implementation. The `QualityGate` trait is designed to support composition. See the composition intent note in the "QualityGate Trait" section.

5. **Artefact size and storage** — Resolved: the framework stores artefact metadata/summaries (via `Artefact::to_json()`), never the artefact payload itself. See "Artefact storage boundary" in the "Artefact Trait" section. The consuming application manages artefact storage.
