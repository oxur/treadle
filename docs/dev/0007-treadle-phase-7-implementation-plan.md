# Treadle Phase 7 Implementation Plan

## Phase 7: Quality Gate Framework

**Goal:** Define the quality gate type system -- `QualityGate` trait, `QualityVerdict`, `QualityFeedback`, `CriterionResult`, and `QualityContext` -- and integrate quality gate registration into `WorkflowBuilder` and `Workflow`. No execution changes are made in this phase; the types and builder plumbing are established so that Phase 9 (retry loop machinery) and Phase 10 (quality gate integration) can wire them into the executor.

**Prerequisites:**
- Phases 1--5 completed (core types, state stores, workflow builder, executor, status)
- Phase 6 completed (`Artefact` trait, `StageOutput`, revised `Stage::execute`, artefact storage)
- `cargo build` and `cargo test` pass
- The `unsafe impl Send/Sync` lines in `src/workflow.rs` have been removed (see v2.1 design SS2.12)

---

## Milestone 7.1 -- QualityVerdict, QualityFeedback, CriterionResult

### Files to Create/Modify
- `src/quality.rs` (new)
- `src/lib.rs` (add module, re-exports)

### Implementation Details

#### 7.1.1 Create `src/quality.rs`

```rust
//! Quality gate types for the Treadle workflow engine.
//!
//! This module defines the types used by quality gates to evaluate stage
//! output. Quality gates are optional, composable checks attached to stages
//! at the workflow builder level. They evaluate whether a stage's output
//! meets defined criteria and return a [`QualityVerdict`] indicating whether
//! the output is acceptable, rejected (with feedback), or uncertain.
//!
//! # Composition Intent
//!
//! The [`QualityGate`] trait is designed to support future composition
//! (e.g., `gate_a.and(gate_b)` or `gate_a.or(gate_b)` via a
//! `CompositeGate` wrapper). The trait signature -- returning a
//! [`QualityVerdict`] -- is sufficient for `and`/`or` wrappers, and the
//! builder API accepts trait objects, so a future `CompositeGate` drops in
//! without API changes. Composition support is planned but deferred from
//! the initial implementation.

use crate::stage::StageOutput;
use crate::work_item::WorkItem;
use crate::Result;
use async_trait::async_trait;

/// The verdict returned by a quality gate after evaluating stage output.
///
/// Quality gates inspect the artefacts and summary produced by a stage and
/// return one of three verdicts:
///
/// - [`Accepted`](QualityVerdict::Accepted) -- output meets all criteria
/// - [`Rejected`](QualityVerdict::Rejected) -- output does not meet criteria,
///   with structured feedback for the next attempt
/// - [`Uncertain`](QualityVerdict::Uncertain) -- the gate cannot determine
///   quality automatically; escalate to human review
///
/// # Serialisation
///
/// `QualityVerdict` derives `Serialize` and `Deserialize` so it can be
/// persisted in attempt records and displayed in review UIs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum QualityVerdict {
    /// Output meets all quality criteria. Proceed to the next stage
    /// (subject to the stage's [`ReviewPolicy`]).
    Accepted,

    /// Output does not meet one or more quality criteria. Includes
    /// structured feedback that will be threaded into the
    /// [`StageContext::feedback`] field on the next attempt.
    Rejected {
        /// Structured feedback describing what failed and how to improve.
        feedback: QualityFeedback,
    },

    /// The quality gate cannot determine quality automatically.
    /// The stage should be escalated to human review regardless of
    /// the retry budget.
    Uncertain {
        /// A human-readable explanation of why automated evaluation
        /// was not possible.
        reason: String,
    },
}

impl QualityVerdict {
    /// Returns `true` if this verdict is [`Accepted`](QualityVerdict::Accepted).
    pub fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted)
    }

    /// Returns `true` if this verdict is [`Rejected`](QualityVerdict::Rejected).
    pub fn is_rejected(&self) -> bool {
        matches!(self, Self::Rejected { .. })
    }

    /// Returns `true` if this verdict is [`Uncertain`](QualityVerdict::Uncertain).
    pub fn is_uncertain(&self) -> bool {
        matches!(self, Self::Uncertain { .. })
    }

    /// Returns the feedback if this verdict is [`Rejected`](QualityVerdict::Rejected).
    pub fn feedback(&self) -> Option<&QualityFeedback> {
        match self {
            Self::Rejected { feedback } => Some(feedback),
            _ => None,
        }
    }

    /// Returns the reason if this verdict is [`Uncertain`](QualityVerdict::Uncertain).
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Uncertain { reason } => Some(reason),
            _ => None,
        }
    }
}

/// Structured feedback from a quality gate rejection.
///
/// When a quality gate rejects stage output, it provides a `QualityFeedback`
/// containing a human-readable summary, the list of criteria that were not
/// met, and optional structured guidance for the next attempt.
///
/// This feedback is:
/// - Stored in the [`AttemptRecord`] for review UIs (Phase 9)
/// - Threaded into [`StageContext::feedback`] on the next attempt so the
///   stage implementation can adjust its behaviour
///
/// # Serialisation
///
/// `QualityFeedback` derives `Serialize` and `Deserialize` for persistence
/// in the state store and for round-trip through JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct QualityFeedback {
    /// Human-readable summary of the quality assessment.
    ///
    /// Example: "2 criteria not met: table preservation failed, word count below minimum"
    pub summary: String,

    /// The specific criteria that were evaluated, including both
    /// passed and failed results.
    pub failed_criteria: Vec<CriterionResult>,

    /// Optional structured guidance for the next attempt.
    ///
    /// This is opaque to the engine but meaningful to stage
    /// implementations. For an AI agent wrapper, this might contain
    /// hints like `{"strategy": "ocr", "focus": "tables"}`.
    pub guidance: Option<serde_json::Value>,
}

impl QualityFeedback {
    /// Creates a new `QualityFeedback` with a summary and failed criteria.
    pub fn new(summary: impl Into<String>, failed_criteria: Vec<CriterionResult>) -> Self {
        Self {
            summary: summary.into(),
            failed_criteria,
            guidance: None,
        }
    }

    /// Adds structured guidance to this feedback.
    pub fn with_guidance(mut self, guidance: serde_json::Value) -> Self {
        self.guidance = Some(guidance);
        self
    }

    /// Returns `true` if all criteria passed (i.e., no failures).
    pub fn all_passed(&self) -> bool {
        self.failed_criteria.iter().all(|c| c.passed)
    }

    /// Returns the number of failed criteria.
    pub fn failure_count(&self) -> usize {
        self.failed_criteria.iter().filter(|c| !c.passed).count()
    }
}

/// The result of evaluating a single quality criterion.
///
/// Each criterion has a name, expected value, actual observed value, and
/// a boolean indicating whether it passed.
///
/// # Example
///
/// ```
/// use treadle::CriterionResult;
///
/// let result = CriterionResult::failed(
///     "word_count",
///     ">= 100",
///     "42",
/// );
/// assert!(!result.passed);
/// assert_eq!(result.name, "word_count");
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CriterionResult {
    /// Name of the criterion (e.g., "table_preservation", "word_count").
    pub name: String,

    /// What was expected (e.g., ">= 100", "tables present").
    pub expected: String,

    /// What was actually observed (e.g., "42", "no tables found").
    pub actual: String,

    /// Whether this criterion was met.
    pub passed: bool,
}

impl CriterionResult {
    /// Creates a passing criterion result.
    pub fn passed(
        name: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            expected: expected.into(),
            actual: actual.into(),
            passed: true,
        }
    }

    /// Creates a failing criterion result.
    pub fn failed(
        name: impl Into<String>,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            expected: expected.into(),
            actual: actual.into(),
            passed: false,
        }
    }
}
```

#### 7.1.2 Update `src/lib.rs`

Add the module declaration and re-exports:

```rust
pub mod quality;

pub use quality::{CriterionResult, QualityFeedback, QualityVerdict};
```

#### 7.1.3 Tests for QualityVerdict, QualityFeedback, CriterionResult

These tests are placed inside `src/quality.rs` in a `#[cfg(test)] mod tests` block:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // CriterionResult tests
    // ---------------------------------------------------------------

    #[test]
    fn test_criterion_result_passed_construction() {
        let cr = CriterionResult::passed("word_count", ">= 100", "150");
        assert_eq!(cr.name, "word_count");
        assert_eq!(cr.expected, ">= 100");
        assert_eq!(cr.actual, "150");
        assert!(cr.passed);
    }

    #[test]
    fn test_criterion_result_failed_construction() {
        let cr = CriterionResult::failed("table_preservation", "tables present", "no tables");
        assert_eq!(cr.name, "table_preservation");
        assert_eq!(cr.expected, "tables present");
        assert_eq!(cr.actual, "no tables");
        assert!(!cr.passed);
    }

    #[test]
    fn test_criterion_result_serialize_roundtrip() {
        let cr = CriterionResult::failed("connectivity", ">= 0.8", "0.3");
        let json = serde_json::to_string(&cr).unwrap();
        let deserialized: CriterionResult = serde_json::from_str(&json).unwrap();
        assert_eq!(cr, deserialized);
    }

    #[test]
    fn test_criterion_result_debug() {
        let cr = CriterionResult::passed("check", "yes", "yes");
        let debug = format!("{:?}", cr);
        assert!(debug.contains("CriterionResult"));
        assert!(debug.contains("check"));
    }

    #[test]
    fn test_criterion_result_clone() {
        let cr = CriterionResult::failed("x", "a", "b");
        let cloned = cr.clone();
        assert_eq!(cr, cloned);
    }

    // ---------------------------------------------------------------
    // QualityFeedback tests
    // ---------------------------------------------------------------

    #[test]
    fn test_quality_feedback_new() {
        let feedback = QualityFeedback::new("2 criteria failed", vec![
            CriterionResult::failed("word_count", ">= 100", "42"),
            CriterionResult::failed("tables", "present", "absent"),
        ]);
        assert_eq!(feedback.summary, "2 criteria failed");
        assert_eq!(feedback.failed_criteria.len(), 2);
        assert!(feedback.guidance.is_none());
    }

    #[test]
    fn test_quality_feedback_with_guidance() {
        let feedback = QualityFeedback::new("1 criterion failed", vec![
            CriterionResult::failed("tables", "present", "absent"),
        ])
        .with_guidance(serde_json::json!({"strategy": "ocr"}));

        assert!(feedback.guidance.is_some());
        assert_eq!(feedback.guidance.unwrap()["strategy"], "ocr");
    }

    #[test]
    fn test_quality_feedback_all_passed_true() {
        let feedback = QualityFeedback::new("all good", vec![
            CriterionResult::passed("a", "x", "x"),
            CriterionResult::passed("b", "y", "y"),
        ]);
        assert!(feedback.all_passed());
        assert_eq!(feedback.failure_count(), 0);
    }

    #[test]
    fn test_quality_feedback_all_passed_false() {
        let feedback = QualityFeedback::new("issues", vec![
            CriterionResult::passed("a", "x", "x"),
            CriterionResult::failed("b", "y", "z"),
        ]);
        assert!(!feedback.all_passed());
        assert_eq!(feedback.failure_count(), 1);
    }

    #[test]
    fn test_quality_feedback_empty_criteria() {
        let feedback = QualityFeedback::new("no criteria", vec![]);
        assert!(feedback.all_passed());
        assert_eq!(feedback.failure_count(), 0);
    }

    #[test]
    fn test_quality_feedback_serialize_roundtrip() {
        let feedback = QualityFeedback::new("test", vec![
            CriterionResult::failed("c1", "a", "b"),
        ])
        .with_guidance(serde_json::json!({"hint": "try harder"}));

        let json = serde_json::to_string(&feedback).unwrap();
        let deserialized: QualityFeedback = serde_json::from_str(&json).unwrap();
        assert_eq!(feedback, deserialized);
    }

    #[test]
    fn test_quality_feedback_serialize_without_guidance() {
        let feedback = QualityFeedback::new("simple", vec![]);
        let json = serde_json::to_string(&feedback).unwrap();
        let deserialized: QualityFeedback = serde_json::from_str(&json).unwrap();
        assert_eq!(feedback, deserialized);
        assert!(deserialized.guidance.is_none());
    }

    #[test]
    fn test_quality_feedback_debug() {
        let feedback = QualityFeedback::new("debug test", vec![]);
        let debug = format!("{:?}", feedback);
        assert!(debug.contains("QualityFeedback"));
        assert!(debug.contains("debug test"));
    }

    #[test]
    fn test_quality_feedback_clone() {
        let feedback = QualityFeedback::new("clone test", vec![
            CriterionResult::passed("x", "a", "a"),
        ])
        .with_guidance(serde_json::json!({"key": "value"}));

        let cloned = feedback.clone();
        assert_eq!(feedback, cloned);
    }

    // ---------------------------------------------------------------
    // QualityVerdict tests
    // ---------------------------------------------------------------

    #[test]
    fn test_quality_verdict_accepted() {
        let verdict = QualityVerdict::Accepted;
        assert!(verdict.is_accepted());
        assert!(!verdict.is_rejected());
        assert!(!verdict.is_uncertain());
        assert!(verdict.feedback().is_none());
        assert!(verdict.reason().is_none());
    }

    #[test]
    fn test_quality_verdict_rejected() {
        let feedback = QualityFeedback::new("failed", vec![
            CriterionResult::failed("c1", "a", "b"),
        ]);
        let verdict = QualityVerdict::Rejected {
            feedback: feedback.clone(),
        };
        assert!(!verdict.is_accepted());
        assert!(verdict.is_rejected());
        assert!(!verdict.is_uncertain());
        assert_eq!(verdict.feedback(), Some(&feedback));
        assert!(verdict.reason().is_none());
    }

    #[test]
    fn test_quality_verdict_uncertain() {
        let verdict = QualityVerdict::Uncertain {
            reason: "cannot parse output format".to_string(),
        };
        assert!(!verdict.is_accepted());
        assert!(!verdict.is_rejected());
        assert!(verdict.is_uncertain());
        assert!(verdict.feedback().is_none());
        assert_eq!(verdict.reason(), Some("cannot parse output format"));
    }

    #[test]
    fn test_quality_verdict_serialize_accepted() {
        let verdict = QualityVerdict::Accepted;
        let json = serde_json::to_string(&verdict).unwrap();
        let deserialized: QualityVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(verdict, deserialized);
    }

    #[test]
    fn test_quality_verdict_serialize_rejected() {
        let verdict = QualityVerdict::Rejected {
            feedback: QualityFeedback::new("bad output", vec![
                CriterionResult::failed("tables", "present", "missing"),
            ])
            .with_guidance(serde_json::json!({"try": "ocr"})),
        };
        let json = serde_json::to_string(&verdict).unwrap();
        let deserialized: QualityVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(verdict, deserialized);
    }

    #[test]
    fn test_quality_verdict_serialize_uncertain() {
        let verdict = QualityVerdict::Uncertain {
            reason: "ambiguous output".to_string(),
        };
        let json = serde_json::to_string(&verdict).unwrap();
        let deserialized: QualityVerdict = serde_json::from_str(&json).unwrap();
        assert_eq!(verdict, deserialized);
    }

    #[test]
    fn test_quality_verdict_debug() {
        let verdict = QualityVerdict::Accepted;
        let debug = format!("{:?}", verdict);
        assert!(debug.contains("Accepted"));
    }

    #[test]
    fn test_quality_verdict_clone() {
        let verdict = QualityVerdict::Rejected {
            feedback: QualityFeedback::new("test", vec![]),
        };
        let cloned = verdict.clone();
        assert_eq!(verdict, cloned);
    }

    #[test]
    fn test_quality_verdict_equality() {
        assert_eq!(QualityVerdict::Accepted, QualityVerdict::Accepted);
        assert_ne!(
            QualityVerdict::Accepted,
            QualityVerdict::Uncertain { reason: "x".to_string() },
        );
    }
}
```

**Design Decisions:**
- `QualityVerdict`, `QualityFeedback`, and `CriterionResult` all derive `Serialize` + `Deserialize` for persistence in attempt records (Phase 9) and display in review UIs
- `QualityVerdict` derives `PartialEq` and `Clone` -- unlike `StageOutput`, verdicts need to be compared (in tests and in retry logic) and duplicated (stored in attempt records)
- `CriterionResult` uses `String` fields rather than enums for `expected`/`actual` because quality criteria are domain-specific and cannot be enumerated by the framework
- `QualityFeedback::guidance` uses `Option<serde_json::Value>` to remain opaque to the engine while being serialisable
- Convenience methods on `QualityVerdict` (`is_accepted()`, `feedback()`, etc.) avoid forcing downstream code to match on variants for simple queries
- `CriterionResult::passed()`/`failed()` constructors with `impl Into<String>` follow the project's builder/constructor conventions

### Verification Commands

```bash
cargo build
cargo test quality::tests
cargo clippy
```

---

## Milestone 7.2 -- QualityContext

### Files to Modify
- `src/quality.rs` (add `QualityContext`)
- `src/lib.rs` (update re-exports)

### Implementation Details

#### 7.2.1 Add `QualityContext` to `src/quality.rs`

```rust
/// Context provided to a quality gate during evaluation.
///
/// Contains metadata about the current evaluation attempt, including
/// the attempt number, maximum allowed attempts, and the history of
/// previous attempts for this stage.
///
/// # Attempt History
///
/// The `previous_attempts` field provides the full history of prior
/// attempts as serialised JSON values. In Phase 9, when `AttemptRecord`
/// is defined, this field will be replaced with `Vec<AttemptRecord>`.
/// Until then, each entry is a JSON object containing whatever attempt
/// metadata has been recorded.
///
/// # Usage
///
/// Quality gates can use `QualityContext` to make decisions based on
/// attempt history. For example, a gate might escalate to human review
/// if the last two attempts both failed on the same criterion, rather
/// than retrying a third time.
#[derive(Debug, Clone)]
pub struct QualityContext {
    /// Which attempt this quality check is evaluating (1-indexed).
    ///
    /// The first attempt is `1`, the first retry is `2`, etc.
    pub attempt: u32,

    /// Maximum attempts allowed by the stage's retry budget.
    ///
    /// A value of `1` means no retries (single attempt only).
    pub max_attempts: u32,

    /// Previous attempt records for this item through this stage.
    ///
    /// Empty on the first attempt. On subsequent attempts, contains
    /// serialised records of prior attempts including their verdicts
    /// and feedback.
    ///
    /// **Note:** This field uses `Vec<serde_json::Value>` as a
    /// placeholder. In Phase 9, when `AttemptRecord` is defined,
    /// this will be replaced with `Vec<AttemptRecord>`.
    pub previous_attempts: Vec<serde_json::Value>,

    /// The name of the stage being evaluated.
    pub stage_name: String,

    /// The stringified ID of the work item being evaluated.
    pub item_id: String,
}

impl QualityContext {
    /// Creates a new `QualityContext`.
    ///
    /// # Arguments
    ///
    /// * `stage_name` - The name of the stage being evaluated
    /// * `item_id` - The work item's identifier
    /// * `attempt` - The current attempt number (1-indexed)
    /// * `max_attempts` - The maximum number of attempts allowed
    pub fn new(
        stage_name: impl Into<String>,
        item_id: impl Into<String>,
        attempt: u32,
        max_attempts: u32,
    ) -> Self {
        Self {
            attempt,
            max_attempts,
            previous_attempts: Vec::new(),
            stage_name: stage_name.into(),
            item_id: item_id.into(),
        }
    }

    /// Adds previous attempt records to this context.
    pub fn with_previous_attempts(mut self, attempts: Vec<serde_json::Value>) -> Self {
        self.previous_attempts = attempts;
        self
    }

    /// Returns `true` if this is the first attempt.
    pub fn is_first_attempt(&self) -> bool {
        self.attempt == 1
    }

    /// Returns `true` if this is the final attempt (no more retries
    /// available after this one).
    pub fn is_final_attempt(&self) -> bool {
        self.attempt >= self.max_attempts
    }

    /// Returns the number of remaining attempts after this one.
    pub fn remaining_attempts(&self) -> u32 {
        self.max_attempts.saturating_sub(self.attempt)
    }
}
```

#### 7.2.2 Update `src/lib.rs` re-exports

```rust
pub use quality::{CriterionResult, QualityContext, QualityFeedback, QualityVerdict};
```

#### 7.2.3 Tests for QualityContext

Add to the `tests` module in `src/quality.rs`:

```rust
    // ---------------------------------------------------------------
    // QualityContext tests
    // ---------------------------------------------------------------

    #[test]
    fn test_quality_context_new() {
        let ctx = QualityContext::new("pdf_to_md", "item-1", 1, 3);
        assert_eq!(ctx.stage_name, "pdf_to_md");
        assert_eq!(ctx.item_id, "item-1");
        assert_eq!(ctx.attempt, 1);
        assert_eq!(ctx.max_attempts, 3);
        assert!(ctx.previous_attempts.is_empty());
    }

    #[test]
    fn test_quality_context_with_previous_attempts() {
        let prev = vec![
            serde_json::json!({"attempt": 1, "verdict": "Rejected"}),
            serde_json::json!({"attempt": 2, "verdict": "Rejected"}),
        ];
        let ctx = QualityContext::new("stage", "item-1", 3, 3)
            .with_previous_attempts(prev.clone());

        assert_eq!(ctx.previous_attempts.len(), 2);
        assert_eq!(ctx.previous_attempts[0]["attempt"], 1);
        assert_eq!(ctx.previous_attempts[1]["verdict"], "Rejected");
    }

    #[test]
    fn test_quality_context_is_first_attempt() {
        let first = QualityContext::new("s", "i", 1, 3);
        assert!(first.is_first_attempt());

        let second = QualityContext::new("s", "i", 2, 3);
        assert!(!second.is_first_attempt());
    }

    #[test]
    fn test_quality_context_is_final_attempt() {
        let ctx = QualityContext::new("s", "i", 3, 3);
        assert!(ctx.is_final_attempt());

        let ctx = QualityContext::new("s", "i", 2, 3);
        assert!(!ctx.is_final_attempt());

        let ctx = QualityContext::new("s", "i", 1, 1);
        assert!(ctx.is_final_attempt());
    }

    #[test]
    fn test_quality_context_is_final_attempt_over_max() {
        // Edge case: attempt exceeds max_attempts
        let ctx = QualityContext::new("s", "i", 5, 3);
        assert!(ctx.is_final_attempt());
    }

    #[test]
    fn test_quality_context_remaining_attempts() {
        let ctx = QualityContext::new("s", "i", 1, 3);
        assert_eq!(ctx.remaining_attempts(), 2);

        let ctx = QualityContext::new("s", "i", 3, 3);
        assert_eq!(ctx.remaining_attempts(), 0);

        let ctx = QualityContext::new("s", "i", 2, 5);
        assert_eq!(ctx.remaining_attempts(), 3);
    }

    #[test]
    fn test_quality_context_remaining_attempts_saturates() {
        // Edge case: attempt exceeds max_attempts -- should not underflow
        let ctx = QualityContext::new("s", "i", 10, 3);
        assert_eq!(ctx.remaining_attempts(), 0);
    }

    #[test]
    fn test_quality_context_single_attempt_budget() {
        // max_attempts: 1 means no retries
        let ctx = QualityContext::new("s", "i", 1, 1);
        assert!(ctx.is_first_attempt());
        assert!(ctx.is_final_attempt());
        assert_eq!(ctx.remaining_attempts(), 0);
    }

    #[test]
    fn test_quality_context_debug() {
        let ctx = QualityContext::new("pdf_to_md", "doc-42", 2, 5);
        let debug = format!("{:?}", ctx);
        assert!(debug.contains("QualityContext"));
        assert!(debug.contains("pdf_to_md"));
        assert!(debug.contains("doc-42"));
    }

    #[test]
    fn test_quality_context_clone() {
        let ctx = QualityContext::new("stage", "item", 1, 3)
            .with_previous_attempts(vec![serde_json::json!({"x": 1})]);
        let cloned = ctx.clone();
        assert_eq!(cloned.stage_name, ctx.stage_name);
        assert_eq!(cloned.item_id, ctx.item_id);
        assert_eq!(cloned.attempt, ctx.attempt);
        assert_eq!(cloned.max_attempts, ctx.max_attempts);
        assert_eq!(cloned.previous_attempts.len(), ctx.previous_attempts.len());
    }
```

**Design Decisions:**
- `QualityContext` does not derive `Serialize`/`Deserialize` -- it is an ephemeral input to quality gate evaluation, not persisted
- `previous_attempts` uses `Vec<serde_json::Value>` as a placeholder for `Vec<AttemptRecord>` (Phase 9). This avoids a forward dependency while keeping the field in the struct for API stability
- `attempt` is 1-indexed to match human expectations ("attempt 1 of 3") and the existing `StageContext::attempt` convention
- `remaining_attempts()` uses `saturating_sub` to avoid underflow if attempt exceeds max_attempts (defensive programming)
- `is_final_attempt()` uses `>=` rather than `==` for robustness against edge cases
- Builder-style `with_previous_attempts()` follows the project convention

### Verification Commands

```bash
cargo build
cargo test quality::tests
cargo clippy
```

---

## Milestone 7.3 -- QualityGate Trait

### Files to Modify
- `src/quality.rs` (add trait definition)
- `src/lib.rs` (update re-exports)

### Implementation Details

#### 7.3.1 Add `QualityGate` trait to `src/quality.rs`

```rust
/// A quality gate evaluates the output of a stage and returns a verdict.
///
/// Quality gates are the "judgement" layer in Treadle's separation of
/// concerns: stages do the work, quality gates judge the result, and
/// review policies decide what to do next.
///
/// # Concurrency
///
/// Implementations must be safe to call concurrently for *different* work
/// items. The executor does not call `evaluate` concurrently for the *same*
/// item through the same stage -- each item proceeds sequentially through
/// the retry loop. However, multiple items may be advancing through the
/// pipeline simultaneously, so shared state (connection pools, API clients,
/// caches) must be thread-safe.
///
/// # Generics
///
/// The trait is generic over `W: WorkItem`, meaning quality gates are tied
/// to a specific work item type. This is by design -- a gate for documents
/// is not reusable for audio files, and the type system enforces this
/// constraint.
///
/// # Composition
///
/// This trait is designed for future composition (e.g., `CompositeGate`
/// wrapping multiple gates with `and`/`or` semantics). The return type
/// `QualityVerdict` supports this pattern, and the builder accepts trait
/// objects, so a future `CompositeGate` drops in without API changes.
/// Composition is not implemented in this phase.
///
/// # Object Safety
///
/// This trait is object-safe. Quality gates are stored as
/// `Arc<dyn QualityGate<W>>` inside the workflow.
///
/// # Example
///
/// ```rust,ignore
/// use treadle::*;
/// use async_trait::async_trait;
///
/// struct WordCountGate {
///     min_words: usize,
/// }
///
/// #[async_trait]
/// impl<W: WorkItem> QualityGate<W> for WordCountGate {
///     async fn evaluate(
///         &self,
///         _item: &W,
///         _stage: &str,
///         output: &StageOutput,
///         _ctx: &QualityContext,
///     ) -> Result<QualityVerdict> {
///         let word_count = output.summary
///             .as_deref()
///             .unwrap_or("")
///             .split_whitespace()
///             .count();
///
///         if word_count >= self.min_words {
///             Ok(QualityVerdict::Accepted)
///         } else {
///             Ok(QualityVerdict::Rejected {
///                 feedback: QualityFeedback::new(
///                     format!("Word count {} below minimum {}", word_count, self.min_words),
///                     vec![CriterionResult::failed(
///                         "word_count",
///                         format!(">= {}", self.min_words),
///                         format!("{}", word_count),
///                     )],
///                 ),
///             })
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait QualityGate<W: WorkItem>: Send + Sync {
    /// Evaluates the output of a stage and returns a quality verdict.
    ///
    /// # Arguments
    ///
    /// * `item` - The work item that was processed
    /// * `stage` - The name of the stage whose output is being evaluated
    /// * `output` - The stage's output, including artefacts and summary
    /// * `ctx` - Context about the current attempt, including history
    ///
    /// # Returns
    ///
    /// A [`QualityVerdict`] indicating whether the output is acceptable,
    /// rejected (with feedback), or uncertain.
    ///
    /// # Errors
    ///
    /// Returns an error if the evaluation itself fails (e.g., network error
    /// when calling an external evaluation service). This is distinct from
    /// a [`QualityVerdict::Rejected`] verdict, which indicates the output
    /// was successfully evaluated but found wanting.
    async fn evaluate(
        &self,
        item: &W,
        stage: &str,
        output: &StageOutput,
        ctx: &QualityContext,
    ) -> Result<QualityVerdict>;
}
```

#### 7.3.2 Update `src/lib.rs` re-exports

```rust
pub use quality::{
    CriterionResult, QualityContext, QualityFeedback, QualityGate, QualityVerdict,
};
```

#### 7.3.3 Tests for QualityGate trait

Add to the `tests` module in `src/quality.rs`:

```rust
    // ---------------------------------------------------------------
    // QualityGate trait tests
    // ---------------------------------------------------------------

    use serde::{Deserialize, Serialize};

    // Test work item for trait tests
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestItem {
        id: String,
    }

    impl WorkItem for TestItem {
        fn id(&self) -> &str {
            &self.id
        }
    }

    // A gate that always accepts
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

    // A gate that always rejects with feedback
    struct AlwaysRejectGate {
        reason: String,
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
                feedback: QualityFeedback::new(
                    self.reason.clone(),
                    vec![CriterionResult::failed("always_fail", "pass", "fail")],
                ),
            })
        }
    }

    // A gate that returns Uncertain
    struct UncertainGate;

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
                reason: "cannot determine quality".to_string(),
            })
        }
    }

    // A gate that returns an error
    struct ErrorGate;

    #[async_trait]
    impl QualityGate<TestItem> for ErrorGate {
        async fn evaluate(
            &self,
            _item: &TestItem,
            _stage: &str,
            _output: &StageOutput,
            _ctx: &QualityContext,
        ) -> Result<QualityVerdict> {
            Err(crate::TreadleError::StageExecution(
                "evaluation service unavailable".to_string(),
            ))
        }
    }

    // A gate that inspects the context
    struct AttemptAwareGate;

    #[async_trait]
    impl QualityGate<TestItem> for AttemptAwareGate {
        async fn evaluate(
            &self,
            _item: &TestItem,
            _stage: &str,
            _output: &StageOutput,
            ctx: &QualityContext,
        ) -> Result<QualityVerdict> {
            if ctx.is_final_attempt() {
                // Escalate on final attempt rather than rejecting
                Ok(QualityVerdict::Uncertain {
                    reason: "final attempt, escalating".to_string(),
                })
            } else {
                Ok(QualityVerdict::Rejected {
                    feedback: QualityFeedback::new(
                        format!("attempt {} of {}", ctx.attempt, ctx.max_attempts),
                        vec![],
                    ),
                })
            }
        }
    }

    // Helper to create a minimal StageOutput for testing
    fn test_stage_output() -> StageOutput {
        StageOutput::from_outcome(crate::stage::StageOutcome::Complete)
    }

    #[tokio::test]
    async fn test_quality_gate_always_accept() {
        let gate = AlwaysAcceptGate;
        let item = TestItem { id: "item-1".to_string() };
        let output = test_stage_output();
        let ctx = QualityContext::new("stage", "item-1", 1, 3);

        let verdict = gate.evaluate(&item, "stage", &output, &ctx).await.unwrap();
        assert!(verdict.is_accepted());
    }

    #[tokio::test]
    async fn test_quality_gate_always_reject() {
        let gate = AlwaysRejectGate {
            reason: "quality too low".to_string(),
        };
        let item = TestItem { id: "item-1".to_string() };
        let output = test_stage_output();
        let ctx = QualityContext::new("stage", "item-1", 1, 3);

        let verdict = gate.evaluate(&item, "stage", &output, &ctx).await.unwrap();
        assert!(verdict.is_rejected());
        let feedback = verdict.feedback().unwrap();
        assert_eq!(feedback.summary, "quality too low");
        assert_eq!(feedback.failure_count(), 1);
    }

    #[tokio::test]
    async fn test_quality_gate_uncertain() {
        let gate = UncertainGate;
        let item = TestItem { id: "item-1".to_string() };
        let output = test_stage_output();
        let ctx = QualityContext::new("stage", "item-1", 1, 3);

        let verdict = gate.evaluate(&item, "stage", &output, &ctx).await.unwrap();
        assert!(verdict.is_uncertain());
        assert_eq!(verdict.reason(), Some("cannot determine quality"));
    }

    #[tokio::test]
    async fn test_quality_gate_error() {
        let gate = ErrorGate;
        let item = TestItem { id: "item-1".to_string() };
        let output = test_stage_output();
        let ctx = QualityContext::new("stage", "item-1", 1, 1);

        let result = gate.evaluate(&item, "stage", &output, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("evaluation service unavailable"));
    }

    #[tokio::test]
    async fn test_quality_gate_attempt_aware() {
        let gate = AttemptAwareGate;
        let item = TestItem { id: "item-1".to_string() };
        let output = test_stage_output();

        // Not final attempt -- should reject
        let ctx = QualityContext::new("stage", "item-1", 1, 3);
        let verdict = gate.evaluate(&item, "stage", &output, &ctx).await.unwrap();
        assert!(verdict.is_rejected());

        // Final attempt -- should return uncertain
        let ctx = QualityContext::new("stage", "item-1", 3, 3);
        let verdict = gate.evaluate(&item, "stage", &output, &ctx).await.unwrap();
        assert!(verdict.is_uncertain());
    }

    #[tokio::test]
    async fn test_quality_gate_trait_object_safety() {
        let gate: Box<dyn QualityGate<TestItem>> = Box::new(AlwaysAcceptGate);
        let item = TestItem { id: "item-1".to_string() };
        let output = test_stage_output();
        let ctx = QualityContext::new("stage", "item-1", 1, 1);

        let verdict = gate.evaluate(&item, "stage", &output, &ctx).await.unwrap();
        assert!(verdict.is_accepted());
    }

    #[tokio::test]
    async fn test_quality_gate_arc_trait_object() {
        use std::sync::Arc;

        let gate: Arc<dyn QualityGate<TestItem>> = Arc::new(AlwaysAcceptGate);
        let item = TestItem { id: "item-1".to_string() };
        let output = test_stage_output();
        let ctx = QualityContext::new("stage", "item-1", 1, 1);

        let verdict = gate.evaluate(&item, "stage", &output, &ctx).await.unwrap();
        assert!(verdict.is_accepted());
    }

    #[test]
    fn test_quality_gate_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Box<dyn QualityGate<TestItem>>>();
    }

    #[test]
    fn test_quality_gate_arc_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<std::sync::Arc<dyn QualityGate<TestItem>>>();
    }
```

**Design Decisions:**
- `QualityGate` is generic over `W: WorkItem` (not `dyn WorkItem`) to provide type safety -- gates know exactly what work item type they evaluate
- The trait requires `Send + Sync` because quality gates are stored as `Arc<dyn QualityGate<W>>` and evaluated from async contexts
- The trait uses `async_trait` for consistency with the `Stage` trait and because quality evaluation may involve async operations (e.g., calling an LLM evaluation API)
- The `evaluate` method takes `&StageOutput` by reference because it inspects but does not consume the output
- Errors from `evaluate` represent evaluation failures (e.g., network errors), not quality rejections -- the distinction is important for retry logic
- The trait is object-safe, verified by tests that construct `Box<dyn QualityGate<W>>` and `Arc<dyn QualityGate<W>>`

### Verification Commands

```bash
cargo build
cargo test quality::tests
cargo clippy
```

---

## Milestone 7.4 -- WorkflowBuilder Integration

### Files to Modify
- `src/workflow.rs` (add quality gate storage to `Workflow`, add `.quality_gate()` to `WorkflowBuilder`)
- `src/lib.rs` (no additional changes needed beyond Milestone 7.3)

### Implementation Details

#### 7.4.1 Update `RegisteredStage` and `Workflow` in `src/workflow.rs`

Add the quality gate import and storage. Note that because `Workflow` currently uses `dyn Stage` (not generic over `W: WorkItem`), and the `QualityGate` trait requires `W: WorkItem`, the quality gate storage must use the same approach. In the current v1 codebase, the `Stage` trait uses `dyn WorkItem` (not a generic parameter), so quality gates must also be stored without the generic parameter. This is achieved by using `dyn QualityGate<dyn WorkItem>` -- however, since `QualityGate` requires `W: WorkItem` and `dyn WorkItem` does not satisfy `WorkItem` (it is not `Sized`), we need a design adaptation.

**Adaptation:** Since the v1 `Stage` trait uses `dyn WorkItem` (object-safe, not generic), and the v2.1 design envisions `Stage<W: WorkItem>` (generic), but the current codebase has not yet made that transition, we store quality gates as type-erased trait objects. We introduce a helper trait `AnyQualityGate` that is object-safe and non-generic, wrapping the typed `QualityGate<W>`. This will be simplified when the `Stage` trait is made generic in a future phase.

**Alternative approach (simpler for now):** Since Phase 7 explicitly states "no execution changes yet -- just the type system and builder," and the current `Stage` trait is not generic, we store quality gates as `Arc<dyn std::any::Any + Send + Sync>` with a stage-name key. This is a pragmatic choice: the gate is stored opaquely and will be downcast when the executor needs it (Phase 10). The builder validates that stage names exist, ensuring correctness without requiring the generic parameter at the workflow level.

After careful consideration, the cleanest approach consistent with the existing codebase is to use a trait alias pattern. However, given that the v1 `WorkItem` trait uses `dyn WorkItem`, we use `Box<dyn std::any::Any + Send + Sync>` for storage and provide typed access via a helper method.

```rust
use crate::quality::QualityGate;
use std::any::Any;

/// Type-erased quality gate storage.
///
/// Quality gates are generic over `W: WorkItem`, but `Workflow` is not
/// (it uses `dyn WorkItem`). Gates are stored as type-erased `Any` trait
/// objects and will be downcast to the concrete `dyn QualityGate<W>` type
/// when the executor evaluates them (Phase 10).
///
/// Build-time validation ensures that every quality gate references a
/// valid stage name. Runtime downcasting is safe because the builder
/// enforces type consistency.
type ErasedGate = Arc<dyn Any + Send + Sync>;
```

Add to the `Workflow` struct:

```rust
pub struct Workflow {
    /// The underlying directed graph.
    graph: DiGraph<RegisteredStage, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Cached topological order of stage names.
    topo_order: Vec<String>,
    /// Event broadcast channel sender.
    event_tx: broadcast::Sender<WorkflowEvent>,
    /// Quality gates, keyed by stage name.
    quality_gates: HashMap<String, ErasedGate>,
}
```

Add accessor to `Workflow`:

```rust
impl Workflow {
    // ... existing methods ...

    /// Returns `true` if the given stage has a quality gate attached.
    pub fn has_quality_gate(&self, stage: &str) -> bool {
        self.quality_gates.contains_key(stage)
    }

    /// Returns the names of all stages that have quality gates attached.
    pub fn gated_stages(&self) -> Vec<&str> {
        self.quality_gates.keys().map(|s| s.as_str()).collect()
    }

    /// Returns the type-erased quality gate for the given stage, if any.
    ///
    /// This method is intended for internal use by the executor. The
    /// caller must downcast the returned `Arc<dyn Any>` to the expected
    /// `Arc<dyn QualityGate<W>>` type.
    pub(crate) fn get_quality_gate(&self, stage: &str) -> Option<&ErasedGate> {
        self.quality_gates.get(stage)
    }
}
```

#### 7.4.2 Update `WorkflowBuilder`

Add quality gate support to the builder:

```rust
pub struct WorkflowBuilder {
    /// The graph being built.
    graph: DiGraph<RegisteredStage, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Deferred dependencies to be resolved at build time.
    deferred_deps: Vec<DeferredDependency>,
    /// Quality gates, keyed by stage name (validated at build time).
    quality_gates: HashMap<String, ErasedGate>,
}
```

Update `WorkflowBuilder::new()`:

```rust
fn new() -> Self {
    Self {
        graph: DiGraph::new(),
        name_to_index: HashMap::new(),
        deferred_deps: Vec::new(),
        quality_gates: HashMap::new(),
    }
}
```

Add the `.quality_gate()` method:

```rust
impl WorkflowBuilder {
    // ... existing methods ...

    /// Attaches a quality gate to a stage.
    ///
    /// The quality gate will be evaluated after the stage executes
    /// (Phase 10). The stage name is validated at build time -- if
    /// the named stage does not exist, [`build()`](Self::build) will
    /// return an error.
    ///
    /// Only one quality gate per stage is supported. Calling this
    /// method twice for the same stage will replace the previous gate.
    /// For composite evaluation, use a `CompositeGate` wrapper
    /// (planned for a future phase).
    ///
    /// # Type Erasure
    ///
    /// The gate is stored as a type-erased `Arc<dyn Any + Send + Sync>`.
    /// The executor will downcast it to the concrete
    /// `Arc<dyn QualityGate<W>>` type when evaluating.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let workflow = Workflow::builder()
    ///     .stage("pdf_to_md", PdfToMdStage::new())
    ///     .quality_gate("pdf_to_md", MarkdownQualityCheck::new())
    ///     .build()?;
    /// ```
    pub fn quality_gate<W: WorkItem + 'static>(
        mut self,
        stage_name: impl Into<String>,
        gate: impl QualityGate<W> + 'static,
    ) -> Self {
        let name = stage_name.into();
        let erased: ErasedGate = Arc::new(gate);
        self.quality_gates.insert(name, erased);
        self
    }
}
```

#### 7.4.3 Update `build()` to validate quality gate stage names

Add validation in the `build()` method, after resolving deferred dependencies and before computing topological order:

```rust
pub fn build(mut self) -> Result<Workflow> {
    // Resolve deferred dependencies
    for dep in &self.deferred_deps {
        let stage_idx = self
            .name_to_index
            .get(&dep.stage)
            .ok_or_else(|| TreadleError::StageNotFound(dep.stage.clone()))?;
        let depends_on_idx = self
            .name_to_index
            .get(&dep.depends_on)
            .ok_or_else(|| TreadleError::StageNotFound(dep.depends_on.clone()))?;

        // Edge direction: depends_on -> stage (dependency flows forward)
        self.graph.add_edge(*depends_on_idx, *stage_idx, ());
    }

    // Validate quality gate stage names
    for gate_stage in self.quality_gates.keys() {
        if !self.name_to_index.contains_key(gate_stage) {
            return Err(TreadleError::StageNotFound(gate_stage.clone()));
        }
    }

    // Check for cycles
    if petgraph::algo::is_cyclic_directed(&self.graph) {
        return Err(TreadleError::DagCycle);
    }

    // Compute topological order
    let topo_order = petgraph::algo::toposort(&self.graph, None)
        .map_err(|_| TreadleError::DagCycle)?
        .into_iter()
        .map(|idx| self.graph[idx].name.clone())
        .collect();

    // Create event broadcast channel
    let (event_tx, _) = broadcast::channel(DEFAULT_EVENT_CHANNEL_CAPACITY);

    Ok(Workflow {
        graph: self.graph,
        name_to_index: self.name_to_index,
        topo_order,
        event_tx,
        quality_gates: self.quality_gates,
    })
}
```

#### 7.4.4 Update `Workflow` Debug impl

Update the manual `Debug` implementation to include quality gate information:

```rust
impl std::fmt::Debug for Workflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let gated: Vec<&str> = self.gated_stages();
        f.debug_struct("Workflow")
            .field("stages", &self.topo_order)
            .field("stage_count", &self.stage_count())
            .field("gated_stages", &gated)
            .finish()
    }
}
```

#### 7.4.5 Tests for WorkflowBuilder quality gate integration

Add to the `tests` module in `src/workflow.rs`:

```rust
    // ---------------------------------------------------------------
    // Milestone 7.4 tests: Quality gate builder integration
    // ---------------------------------------------------------------

    use crate::quality::{
        CriterionResult, QualityContext, QualityFeedback, QualityGate, QualityVerdict,
    };
    use crate::stage::StageOutput;

    // A simple quality gate for testing
    struct TestQualityGate {
        verdict: QualityVerdict,
    }

    impl TestQualityGate {
        fn accepting() -> Self {
            Self {
                verdict: QualityVerdict::Accepted,
            }
        }

        fn rejecting(reason: &str) -> Self {
            Self {
                verdict: QualityVerdict::Rejected {
                    feedback: QualityFeedback::new(
                        reason.to_string(),
                        vec![CriterionResult::failed("test", "pass", "fail")],
                    ),
                },
            }
        }
    }

    // Define TestWorkItem here (uses the existing TestItem from above, but
    // we need it to implement WorkItem for the QualityGate generic parameter)
    // TestItem already implements WorkItem from earlier in this test module.

    #[async_trait]
    impl QualityGate<TestItem> for TestQualityGate {
        async fn evaluate(
            &self,
            _item: &TestItem,
            _stage: &str,
            _output: &StageOutput,
            _ctx: &QualityContext,
        ) -> crate::Result<QualityVerdict> {
            Ok(self.verdict.clone())
        }
    }

    #[test]
    fn test_builder_quality_gate_basic() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .quality_gate::<TestItem>("scan", TestQualityGate::accepting())
            .build()
            .unwrap();

        assert!(workflow.has_quality_gate("scan"));
        assert!(!workflow.has_quality_gate("nonexistent"));
    }

    #[test]
    fn test_builder_quality_gate_multiple_stages() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .stage("enrich", TestStage::new("enrich"))
            .stage("review", TestStage::new("review"))
            .quality_gate::<TestItem>("scan", TestQualityGate::accepting())
            .quality_gate::<TestItem>("enrich", TestQualityGate::rejecting("bad"))
            .dependency("enrich", "scan")
            .dependency("review", "enrich")
            .build()
            .unwrap();

        assert!(workflow.has_quality_gate("scan"));
        assert!(workflow.has_quality_gate("enrich"));
        assert!(!workflow.has_quality_gate("review"));
    }

    #[test]
    fn test_builder_quality_gate_gated_stages() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .quality_gate::<TestItem>("a", TestQualityGate::accepting())
            .quality_gate::<TestItem>("c", TestQualityGate::accepting())
            .build()
            .unwrap();

        let mut gated = workflow.gated_stages();
        gated.sort();
        assert_eq!(gated, vec!["a", "c"]);
    }

    #[test]
    fn test_builder_quality_gate_nonexistent_stage_fails() {
        let result = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .quality_gate::<TestItem>("nonexistent", TestQualityGate::accepting())
            .build();

        assert!(matches!(result, Err(TreadleError::StageNotFound(ref name)) if name == "nonexistent"));
    }

    #[test]
    fn test_builder_quality_gate_replaces_previous() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .quality_gate::<TestItem>("scan", TestQualityGate::accepting())
            .quality_gate::<TestItem>("scan", TestQualityGate::rejecting("replaced"))
            .build()
            .unwrap();

        // Should still have a gate (the replacement)
        assert!(workflow.has_quality_gate("scan"));
        // The gated_stages should list "scan" only once
        assert_eq!(workflow.gated_stages().len(), 1);
    }

    #[test]
    fn test_builder_no_quality_gates() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .build()
            .unwrap();

        assert!(!workflow.has_quality_gate("scan"));
        assert!(workflow.gated_stages().is_empty());
    }

    #[test]
    fn test_builder_quality_gate_with_dependencies() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .quality_gate::<TestItem>("b", TestQualityGate::accepting())
            .build()
            .unwrap();

        assert!(workflow.has_quality_gate("b"));
        assert!(!workflow.has_quality_gate("a"));
        assert!(!workflow.has_quality_gate("c"));

        // Dependencies should still work correctly
        let b_deps = workflow.dependencies("b").unwrap();
        assert_eq!(b_deps, vec!["a"]);
    }

    #[test]
    fn test_builder_quality_gate_preserves_existing_behaviour() {
        // Adding quality gates should not affect existing workflow behaviour
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .quality_gate::<TestItem>("b", TestQualityGate::accepting())
            .build()
            .unwrap();

        assert_eq!(workflow.stage_count(), 3);
        assert!(workflow.has_stage("a"));
        assert!(workflow.has_stage("b"));
        assert!(workflow.has_stage("c"));
        assert_eq!(workflow.root_stages(), vec!["a"]);
        assert_eq!(workflow.leaf_stages(), vec!["c"]);
    }

    #[test]
    fn test_workflow_debug_includes_gated_stages() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .quality_gate::<TestItem>("a", TestQualityGate::accepting())
            .build()
            .unwrap();

        let debug = format!("{:?}", workflow);
        assert!(debug.contains("gated_stages"));
    }

    #[test]
    fn test_get_quality_gate_returns_some_for_gated_stage() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .quality_gate::<TestItem>("scan", TestQualityGate::accepting())
            .build()
            .unwrap();

        assert!(workflow.get_quality_gate("scan").is_some());
    }

    #[test]
    fn test_get_quality_gate_returns_none_for_ungated_stage() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .build()
            .unwrap();

        assert!(workflow.get_quality_gate("scan").is_none());
    }

    #[test]
    fn test_get_quality_gate_returns_none_for_nonexistent_stage() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .build()
            .unwrap();

        assert!(workflow.get_quality_gate("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_advance_unaffected_by_quality_gates() {
        // Quality gates are registered but the executor does not use them
        // yet (Phase 10). Verify that advance() still works correctly.
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .quality_gate::<TestItem>("a", TestQualityGate::accepting())
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        // Both stages should complete (gates are not evaluated yet)
        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }
```

**Design Decisions:**
- Quality gates are stored as `Arc<dyn Any + Send + Sync>` (type-erased) because the current `Workflow` is not generic over `W: WorkItem`. When `Workflow` becomes generic (or when the executor is updated in Phase 10), the gates will be downcast to `Arc<dyn QualityGate<W>>`
- The `.quality_gate()` method on `WorkflowBuilder` requires a turbofish `<W>` type parameter to specify the work item type. This is slightly ergonomically awkward but maintains type safety -- the gate must be compatible with the work items the workflow processes
- Build-time validation ensures quality gate stage names reference existing stages, catching typos early
- Only one quality gate per stage is allowed; calling `.quality_gate()` twice replaces the previous gate. Composite gates are deferred to a future phase
- The `has_quality_gate()` and `gated_stages()` methods are public for introspection, while `get_quality_gate()` is `pub(crate)` for executor use
- Quality gates do not affect execution in Phase 7 -- they are registered but ignored. Phase 10 wires them into the executor

### Verification Commands

```bash
cargo build
cargo test
cargo clippy
cargo doc --no-deps
```

---

## Phase 7 Completion Checklist

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes all tests (including all existing v1 tests and Phase 6 tests)
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `cargo doc --no-deps` builds without warnings
- [ ] `QualityVerdict` enum defined with `Accepted`, `Rejected { feedback }`, `Uncertain { reason }`
- [ ] `QualityVerdict` derives `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`
- [ ] `QualityVerdict` convenience methods: `is_accepted()`, `is_rejected()`, `is_uncertain()`, `feedback()`, `reason()`
- [ ] `QualityFeedback` struct defined with `summary`, `failed_criteria`, `guidance`
- [ ] `QualityFeedback` derives `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`
- [ ] `QualityFeedback` constructor `new()` and builder `with_guidance()`
- [ ] `QualityFeedback` helper methods `all_passed()`, `failure_count()`
- [ ] `CriterionResult` struct defined with `name`, `expected`, `actual`, `passed`
- [ ] `CriterionResult` derives `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq`
- [ ] `CriterionResult` constructors `passed()` and `failed()`
- [ ] `QualityContext` struct defined with `attempt`, `max_attempts`, `previous_attempts`, `stage_name`, `item_id`
- [ ] `QualityContext` derives `Debug`, `Clone`
- [ ] `QualityContext` constructor `new()` and builder `with_previous_attempts()`
- [ ] `QualityContext` helper methods `is_first_attempt()`, `is_final_attempt()`, `remaining_attempts()`
- [ ] `QualityGate` trait defined with `evaluate()` method, `Send + Sync` bounds
- [ ] `QualityGate` concurrency documentation in doc comment
- [ ] `QualityGate` composition intent documented
- [ ] `QualityGate` object-safe (verified by Box and Arc tests)
- [ ] `WorkflowBuilder::quality_gate()` method added
- [ ] Quality gate stage name validated at build time
- [ ] Quality gates stored in `Workflow` alongside stages
- [ ] `Workflow::has_quality_gate()` and `Workflow::gated_stages()` public methods
- [ ] `Workflow::get_quality_gate()` pub(crate) method
- [ ] `Workflow` Debug impl updated to include gated stages
- [ ] Existing tests pass unchanged (quality gates do not affect execution)
- [ ] `previous_attempts` uses `Vec<serde_json::Value>` placeholder (to be replaced in Phase 9)
- [ ] Serialisation round-trip tests for `QualityVerdict`, `QualityFeedback`, `CriterionResult`
- [ ] All re-exports added to `src/lib.rs`

---

## Public API After Phase 7

```rust
// New in Phase 7
treadle::QualityGate             // Trait: evaluate stage output
treadle::QualityVerdict          // Enum: Accepted, Rejected, Uncertain
treadle::QualityFeedback         // Struct: summary, failed_criteria, guidance
treadle::CriterionResult         // Struct: name, expected, actual, passed
treadle::QualityContext          // Struct: attempt context for quality gates

// Modified in Phase 7
treadle::Workflow                // +has_quality_gate(), +gated_stages()
treadle::WorkflowBuilder         // +quality_gate()
```

---

## Notes on Rust Guidelines Applied

### Anti-Patterns Avoided
- **AP-01:** No boolean arguments -- `CriterionResult` uses named constructors (`passed()`, `failed()`) rather than a boolean parameter
- **AP-03:** `Send + Sync` bounds on `QualityGate` trait for async safety; quality gates stored as `Arc<dyn ...>` for shared ownership
- **AP-09:** No `unwrap()` in library code -- all fallible operations return `Result`
- **AP-11:** No premature abstraction -- composition support (`CompositeGate`) is documented as intent but deferred

### Core Idioms Applied
- **ID-04:** `impl Into<String>` for constructor parameters (`QualityFeedback::new()`, `CriterionResult::passed/failed()`, `QualityContext::new()`)
- **ID-09:** `new()` as canonical constructor pattern
- **ID-35:** Builder methods returning `Self` (`with_guidance()`, `with_previous_attempts()`)

### Type Design
- **TD-01:** `QualityVerdict` uses distinct enum variants rather than boolean fields
- **TD-02:** `CriterionResult` captures the full evaluation result (name, expected, actual, passed) for rich feedback
- **TD-03:** `QualityContext` is separate from `StageContext` -- different concerns, different lifetimes

### Testing Approach
- Comprehensive unit tests for all types: construction, accessors, edge cases
- Serialisation round-trip tests for all `Serialize`/`Deserialize` types
- Trait object safety verification (Box and Arc)
- Send + Sync compile-time assertions
- Builder validation tests (valid and invalid stage names)
- Integration test verifying quality gates do not affect existing execution behaviour
- Test naming follows `test_<component>_<scenario>_<expectation>` convention
