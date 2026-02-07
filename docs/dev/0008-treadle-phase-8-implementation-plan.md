# Treadle Phase 8 Implementation Plan

## Phase 8: Retry Budget, Review Policy, and Review Outcome

**Goal:** Introduce the `RetryBudget`, `ExhaustedAction`, `ReviewPolicy`, and `ReviewOutcome` types. Add the `resolve_review` method to `Workflow` with `approve_review`/`reject_review` convenience wrappers. Extend `WorkflowBuilder` with `.retry_budget()` and `.review_policy()` methods that validate stage existence at build time. Defaults match v1 behaviour: 1 attempt, fail on error, no review policy, no timeout.

**Prerequisites:**
- Phases 1--7 completed (core types, state stores, workflow builder, executor, status, artefact passing, quality gate framework)
- `cargo build` and `cargo test` pass
- The `Artefact` trait, `StageOutput`, `QualityGate`, `QualityVerdict`, `QualityFeedback`, and `QualityContext` types are defined

---

## Milestone 8.1 -- RetryBudget and ExhaustedAction

### Files to Create/Modify
- `src/retry.rs` (new)
- `src/lib.rs` (add module, re-exports)

### Implementation Details

#### 8.1.1 Create `src/retry.rs`

```rust
//! Retry budget types for stage execution.
//!
//! This module defines [`RetryBudget`] and [`ExhaustedAction`], which control
//! how many quality-gate-driven retries a stage receives before the engine
//! takes a fallback action (fail or escalate to human review).

use std::time::Duration;

/// What to do when a stage's retry budget is exhausted.
///
/// This enum controls the fallback behaviour when a quality gate continues
/// to reject stage output after all allowed attempts have been used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExhaustedAction {
    /// Fail the stage. The work item is marked as failed for this stage
    /// and no further progress is made on this path.
    Fail,

    /// Escalate to human review. The reviewer sees all attempt records,
    /// their quality assessments, and can approve the best output or
    /// request manual intervention.
    Escalate,
}

/// Controls how many quality-gate-driven retries a stage receives.
///
/// A `RetryBudget` is attached to a stage at the workflow builder level.
/// It governs the retry loop: when a quality gate rejects stage output,
/// the engine re-executes the stage (with feedback) up to `max_attempts`
/// times before taking the [`on_exhausted`](RetryBudget::on_exhausted)
/// action.
///
/// # Defaults
///
/// The default retry budget matches v1 behaviour:
///
/// ```
/// use treadle::RetryBudget;
///
/// let budget = RetryBudget::default();
/// assert_eq!(budget.max_attempts(), 1);
/// assert!(budget.delay().is_none());
/// assert!(budget.attempt_timeout().is_none());
/// ```
///
/// # Attempt Counting
///
/// `max_attempts` includes the initial attempt. So `max_attempts: 3` means
/// 1 initial attempt + 2 retries.
///
/// # Timeout
///
/// The optional `attempt_timeout` is critical for production AI-agent
/// workflows, where LLM calls can take arbitrarily long or fail silently.
/// When set, the executor wraps each stage execution + quality gate
/// evaluation in a `tokio::time::timeout`. A timed-out attempt counts
/// as a failed attempt with appropriate feedback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetryBudget {
    /// Maximum number of attempts (including the initial attempt).
    max_attempts: u32,

    /// Optional delay between retry attempts.
    delay: Option<Duration>,

    /// Optional timeout per attempt. If set, the executor wraps each
    /// stage execution + quality gate evaluation in a timeout.
    attempt_timeout: Option<Duration>,

    /// What to do when the retry budget is exhausted.
    on_exhausted: ExhaustedAction,
}

impl RetryBudget {
    /// Creates a new `RetryBudget` with the given maximum number of attempts.
    ///
    /// All other fields default to `None` / `Fail`. Use the builder methods
    /// to customise.
    ///
    /// # Panics
    ///
    /// Panics if `max_attempts` is 0. At least one attempt is always required.
    ///
    /// # Example
    ///
    /// ```
    /// use treadle::{RetryBudget, ExhaustedAction};
    ///
    /// let budget = RetryBudget::new(3)
    ///     .with_delay(std::time::Duration::from_secs(1))
    ///     .with_attempt_timeout(std::time::Duration::from_secs(120))
    ///     .with_on_exhausted(ExhaustedAction::Escalate);
    ///
    /// assert_eq!(budget.max_attempts(), 3);
    /// ```
    pub fn new(max_attempts: u32) -> Self {
        assert!(max_attempts > 0, "max_attempts must be at least 1");
        Self {
            max_attempts,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        }
    }

    /// Sets the delay between retry attempts.
    pub fn with_delay(mut self, delay: Duration) -> Self {
        self.delay = Some(delay);
        self
    }

    /// Sets the per-attempt timeout.
    pub fn with_attempt_timeout(mut self, timeout: Duration) -> Self {
        self.attempt_timeout = Some(timeout);
        self
    }

    /// Sets the action to take when the retry budget is exhausted.
    pub fn with_on_exhausted(mut self, action: ExhaustedAction) -> Self {
        self.on_exhausted = action;
        self
    }

    /// Returns the maximum number of attempts (including the initial attempt).
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Returns the delay between retry attempts, if set.
    pub fn delay(&self) -> Option<Duration> {
        self.delay
    }

    /// Returns the per-attempt timeout, if set.
    pub fn attempt_timeout(&self) -> Option<Duration> {
        self.attempt_timeout
    }

    /// Returns the action to take when the retry budget is exhausted.
    pub fn on_exhausted(&self) -> ExhaustedAction {
        self.on_exhausted
    }

    /// Returns true if retries are allowed (max_attempts > 1).
    pub fn allows_retry(&self) -> bool {
        self.max_attempts > 1
    }

    /// Returns true if the given attempt number (1-indexed) has
    /// exhausted the budget.
    pub fn is_exhausted(&self, attempt: u32) -> bool {
        attempt >= self.max_attempts
    }
}

impl Default for RetryBudget {
    /// Default matches v1 behaviour: 1 attempt, no delay, no timeout, fail on exhaustion.
    fn default() -> Self {
        Self {
            max_attempts: 1,
            delay: None,
            attempt_timeout: None,
            on_exhausted: ExhaustedAction::Fail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ExhaustedAction tests ---

    #[test]
    fn test_exhausted_action_fail_debug() {
        let action = ExhaustedAction::Fail;
        let debug = format!("{:?}", action);
        assert!(debug.contains("Fail"));
    }

    #[test]
    fn test_exhausted_action_escalate_debug() {
        let action = ExhaustedAction::Escalate;
        let debug = format!("{:?}", action);
        assert!(debug.contains("Escalate"));
    }

    #[test]
    fn test_exhausted_action_clone() {
        let action = ExhaustedAction::Escalate;
        let cloned = action.clone();
        assert_eq!(action, cloned);
    }

    #[test]
    fn test_exhausted_action_copy() {
        let action = ExhaustedAction::Fail;
        let copied = action;
        assert_eq!(action, copied);
    }

    #[test]
    fn test_exhausted_action_equality() {
        assert_eq!(ExhaustedAction::Fail, ExhaustedAction::Fail);
        assert_eq!(ExhaustedAction::Escalate, ExhaustedAction::Escalate);
        assert_ne!(ExhaustedAction::Fail, ExhaustedAction::Escalate);
    }

    // --- RetryBudget::new() tests ---

    #[test]
    fn test_retry_budget_new_single_attempt() {
        let budget = RetryBudget::new(1);
        assert_eq!(budget.max_attempts(), 1);
        assert!(budget.delay().is_none());
        assert!(budget.attempt_timeout().is_none());
        assert_eq!(budget.on_exhausted(), ExhaustedAction::Fail);
    }

    #[test]
    fn test_retry_budget_new_multiple_attempts() {
        let budget = RetryBudget::new(5);
        assert_eq!(budget.max_attempts(), 5);
    }

    #[test]
    #[should_panic(expected = "max_attempts must be at least 1")]
    fn test_retry_budget_new_zero_panics() {
        let _ = RetryBudget::new(0);
    }

    // --- RetryBudget::default() tests ---

    #[test]
    fn test_retry_budget_default_matches_v1() {
        let budget = RetryBudget::default();
        assert_eq!(budget.max_attempts(), 1);
        assert!(budget.delay().is_none());
        assert!(budget.attempt_timeout().is_none());
        assert_eq!(budget.on_exhausted(), ExhaustedAction::Fail);
    }

    // --- Builder method tests ---

    #[test]
    fn test_retry_budget_with_delay() {
        let delay = Duration::from_millis(500);
        let budget = RetryBudget::new(3).with_delay(delay);
        assert_eq!(budget.delay(), Some(delay));
    }

    #[test]
    fn test_retry_budget_with_attempt_timeout() {
        let timeout = Duration::from_secs(120);
        let budget = RetryBudget::new(3).with_attempt_timeout(timeout);
        assert_eq!(budget.attempt_timeout(), Some(timeout));
    }

    #[test]
    fn test_retry_budget_with_on_exhausted() {
        let budget = RetryBudget::new(3).with_on_exhausted(ExhaustedAction::Escalate);
        assert_eq!(budget.on_exhausted(), ExhaustedAction::Escalate);
    }

    #[test]
    fn test_retry_budget_full_builder_chain() {
        let budget = RetryBudget::new(3)
            .with_delay(Duration::from_secs(1))
            .with_attempt_timeout(Duration::from_secs(60))
            .with_on_exhausted(ExhaustedAction::Escalate);

        assert_eq!(budget.max_attempts(), 3);
        assert_eq!(budget.delay(), Some(Duration::from_secs(1)));
        assert_eq!(budget.attempt_timeout(), Some(Duration::from_secs(60)));
        assert_eq!(budget.on_exhausted(), ExhaustedAction::Escalate);
    }

    // --- allows_retry() tests ---

    #[test]
    fn test_retry_budget_allows_retry_single_attempt() {
        let budget = RetryBudget::new(1);
        assert!(!budget.allows_retry());
    }

    #[test]
    fn test_retry_budget_allows_retry_multiple_attempts() {
        let budget = RetryBudget::new(3);
        assert!(budget.allows_retry());
    }

    #[test]
    fn test_retry_budget_allows_retry_default() {
        let budget = RetryBudget::default();
        assert!(!budget.allows_retry());
    }

    // --- is_exhausted() tests ---

    #[test]
    fn test_retry_budget_is_exhausted_at_max() {
        let budget = RetryBudget::new(3);
        assert!(!budget.is_exhausted(1));
        assert!(!budget.is_exhausted(2));
        assert!(budget.is_exhausted(3));
    }

    #[test]
    fn test_retry_budget_is_exhausted_beyond_max() {
        let budget = RetryBudget::new(2);
        assert!(budget.is_exhausted(3));
    }

    #[test]
    fn test_retry_budget_is_exhausted_single_attempt() {
        let budget = RetryBudget::new(1);
        assert!(budget.is_exhausted(1));
    }

    // --- Clone and PartialEq tests ---

    #[test]
    fn test_retry_budget_clone() {
        let budget = RetryBudget::new(3)
            .with_delay(Duration::from_secs(1))
            .with_attempt_timeout(Duration::from_secs(60))
            .with_on_exhausted(ExhaustedAction::Escalate);
        let cloned = budget.clone();
        assert_eq!(budget, cloned);
    }

    #[test]
    fn test_retry_budget_equality() {
        let a = RetryBudget::new(3).with_on_exhausted(ExhaustedAction::Escalate);
        let b = RetryBudget::new(3).with_on_exhausted(ExhaustedAction::Escalate);
        assert_eq!(a, b);
    }

    #[test]
    fn test_retry_budget_inequality_max_attempts() {
        let a = RetryBudget::new(3);
        let b = RetryBudget::new(5);
        assert_ne!(a, b);
    }

    #[test]
    fn test_retry_budget_inequality_on_exhausted() {
        let a = RetryBudget::new(3).with_on_exhausted(ExhaustedAction::Fail);
        let b = RetryBudget::new(3).with_on_exhausted(ExhaustedAction::Escalate);
        assert_ne!(a, b);
    }

    #[test]
    fn test_retry_budget_inequality_delay() {
        let a = RetryBudget::new(3).with_delay(Duration::from_secs(1));
        let b = RetryBudget::new(3);
        assert_ne!(a, b);
    }

    #[test]
    fn test_retry_budget_inequality_timeout() {
        let a = RetryBudget::new(3).with_attempt_timeout(Duration::from_secs(60));
        let b = RetryBudget::new(3);
        assert_ne!(a, b);
    }

    // --- Debug tests ---

    #[test]
    fn test_retry_budget_debug() {
        let budget = RetryBudget::new(3)
            .with_delay(Duration::from_secs(1))
            .with_on_exhausted(ExhaustedAction::Escalate);
        let debug = format!("{:?}", budget);
        assert!(debug.contains("RetryBudget"));
        assert!(debug.contains("max_attempts"));
        assert!(debug.contains("3"));
        assert!(debug.contains("Escalate"));
    }

    #[test]
    fn test_retry_budget_debug_default() {
        let budget = RetryBudget::default();
        let debug = format!("{:?}", budget);
        assert!(debug.contains("RetryBudget"));
        assert!(debug.contains("Fail"));
    }
}
```

**Design Decisions:**
- `RetryBudget` uses private fields with getter methods and builder-style setters to enforce invariants (e.g., `max_attempts >= 1`) -- see the `new()` constructor's panic guard.
- `ExhaustedAction` derives `Copy` because it is a small, stateless enum -- no need for reference semantics.
- `Default` for `RetryBudget` matches v1 behaviour exactly: 1 attempt, no delay, no timeout, fail on exhaustion (v2.1 design document, "Backward Compatibility" section).
- `is_exhausted()` and `allows_retry()` are convenience methods that keep the retry logic out of the executor, improving testability.
- `attempt_timeout` is `Option<Duration>` rather than a mandatory field because most v1 stages do not need timeouts; opt-in keeps the default lightweight.

#### 8.1.2 Update `src/lib.rs`

Add module and re-exports:

```rust
pub mod retry;

pub use retry::{ExhaustedAction, RetryBudget};
```

### Verification Commands

```bash
cargo build
cargo test retry::tests
cargo clippy
```

---

## Milestone 8.2 -- ReviewPolicy Enum

### Files to Create/Modify
- `src/review.rs` (new)
- `src/lib.rs` (add module, re-exports)

### Implementation Details

#### 8.2.1 Create `src/review.rs`

```rust
//! Review policy and outcome types.
//!
//! This module defines [`ReviewPolicy`] and [`ReviewOutcome`], which govern
//! when human review is triggered and what a reviewer can decide.

use crate::artefact::Artefact;

/// Governs when human review is required for a stage.
///
/// Each stage can have a review policy attached at the workflow builder level.
/// The policy is evaluated after the quality gate (if any) has run. Stages
/// without an explicit review policy default to [`Never`](ReviewPolicy::Never).
///
/// # Named Variants (TD-01)
///
/// This enum uses five named variants rather than a `Custom` variant with
/// boolean flags. This follows the TD-01 principle: arguments use types, not
/// bools. If new combinations are needed in the future, they can be added as
/// named variants without breaking existing code.
///
/// # Examples
///
/// ```
/// use treadle::ReviewPolicy;
///
/// // Default: no review required
/// let policy = ReviewPolicy::default();
/// assert!(matches!(policy, ReviewPolicy::Never));
///
/// // Always require human sign-off
/// let policy = ReviewPolicy::Always;
///
/// // Review only when retry budget is exhausted
/// let policy = ReviewPolicy::OnEscalation;
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPolicy {
    /// Never require human review (default). The stage completes when
    /// execution succeeds and the quality gate passes (if any).
    Never,

    /// Always require human review after execution, even if the
    /// quality gate passes. The reviewer sees the output and the
    /// quality assessment.
    Always,

    /// Require human review only when the quality gate rejects the
    /// output and the retry budget is exhausted.
    OnEscalation,

    /// Require human review only when the quality gate returns
    /// `Uncertain`.
    OnUncertain,

    /// Require human review when the retry budget is exhausted (escalation)
    /// or when the quality gate returns `Uncertain`.
    OnEscalationOrUncertain,
}

impl ReviewPolicy {
    /// Returns true if this policy requires review on escalation
    /// (retry budget exhausted).
    pub fn reviews_on_escalation(&self) -> bool {
        matches!(
            self,
            Self::Always | Self::OnEscalation | Self::OnEscalationOrUncertain
        )
    }

    /// Returns true if this policy requires review when the quality gate
    /// returns `Uncertain`.
    pub fn reviews_on_uncertain(&self) -> bool {
        matches!(
            self,
            Self::Always | Self::OnUncertain | Self::OnEscalationOrUncertain
        )
    }

    /// Returns true if this policy always requires review, regardless
    /// of quality gate outcome.
    pub fn always_reviews(&self) -> bool {
        matches!(self, Self::Always)
    }
}

impl Default for ReviewPolicy {
    /// Default: never require human review. Matches v1 behaviour.
    fn default() -> Self {
        Self::Never
    }
}

/// The outcome of a human review.
///
/// Returned by the reviewer when resolving a review. This replaces the v1
/// pattern of separate `approve_review` and `reject_review` methods with a
/// unified `resolve_review` method that accepts a `ReviewOutcome`.
///
/// # Variants
///
/// - [`Approve`](ReviewOutcome::Approve): Accept the output as-is.
/// - [`Reject`](ReviewOutcome::Reject): Reject the output with a reason.
/// - [`ApproveWithEdits`](ReviewOutcome::ApproveWithEdits): Accept with
///   modified artefacts.
///
/// # Note on Trait Implementations
///
/// `ReviewOutcome` cannot derive `Clone` or `PartialEq` because the
/// `ApproveWithEdits` variant contains `Box<dyn Artefact>`, which does
/// not implement these traits. This is acceptable -- the engine consumes
/// `ReviewOutcome` values; it does not compare or duplicate them.
pub enum ReviewOutcome {
    /// Approve the output as-is. The stage is marked complete and the
    /// pipeline continues.
    Approve,

    /// Reject the output. The stage is marked failed with the given reason.
    Reject {
        /// Human-readable reason for rejection.
        reason: String,
    },

    /// Approve with modifications. The pipeline continues with the edited
    /// artefacts instead of the original stage output.
    ApproveWithEdits {
        /// The modified artefacts replacing the stage's original output.
        edited_artefacts: Box<dyn Artefact>,

        /// Optional note about what was changed.
        note: Option<String>,
    },
}

/// Manual `Debug` implementation for `ReviewOutcome` because `Box<dyn Artefact>`
/// does not implement `Debug`.
impl std::fmt::Debug for ReviewOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Approve => write!(f, "ReviewOutcome::Approve"),
            Self::Reject { reason } => f
                .debug_struct("ReviewOutcome::Reject")
                .field("reason", reason)
                .finish(),
            Self::ApproveWithEdits {
                edited_artefacts,
                note,
            } => f
                .debug_struct("ReviewOutcome::ApproveWithEdits")
                .field("edited_artefacts", &edited_artefacts.debug_description())
                .field("note", note)
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;

    // --- ReviewPolicy tests ---

    #[test]
    fn test_review_policy_default_is_never() {
        let policy = ReviewPolicy::default();
        assert_eq!(policy, ReviewPolicy::Never);
    }

    #[test]
    fn test_review_policy_clone() {
        let policy = ReviewPolicy::Always;
        let cloned = policy.clone();
        assert_eq!(policy, cloned);
    }

    #[test]
    fn test_review_policy_copy() {
        let policy = ReviewPolicy::OnEscalation;
        let copied = policy;
        assert_eq!(policy, copied);
    }

    #[test]
    fn test_review_policy_equality() {
        assert_eq!(ReviewPolicy::Never, ReviewPolicy::Never);
        assert_eq!(ReviewPolicy::Always, ReviewPolicy::Always);
        assert_eq!(ReviewPolicy::OnEscalation, ReviewPolicy::OnEscalation);
        assert_eq!(ReviewPolicy::OnUncertain, ReviewPolicy::OnUncertain);
        assert_eq!(
            ReviewPolicy::OnEscalationOrUncertain,
            ReviewPolicy::OnEscalationOrUncertain
        );
    }

    #[test]
    fn test_review_policy_inequality() {
        assert_ne!(ReviewPolicy::Never, ReviewPolicy::Always);
        assert_ne!(ReviewPolicy::OnEscalation, ReviewPolicy::OnUncertain);
        assert_ne!(ReviewPolicy::Always, ReviewPolicy::OnEscalationOrUncertain);
    }

    #[test]
    fn test_review_policy_reviews_on_escalation() {
        assert!(!ReviewPolicy::Never.reviews_on_escalation());
        assert!(ReviewPolicy::Always.reviews_on_escalation());
        assert!(ReviewPolicy::OnEscalation.reviews_on_escalation());
        assert!(!ReviewPolicy::OnUncertain.reviews_on_escalation());
        assert!(ReviewPolicy::OnEscalationOrUncertain.reviews_on_escalation());
    }

    #[test]
    fn test_review_policy_reviews_on_uncertain() {
        assert!(!ReviewPolicy::Never.reviews_on_uncertain());
        assert!(ReviewPolicy::Always.reviews_on_uncertain());
        assert!(!ReviewPolicy::OnEscalation.reviews_on_uncertain());
        assert!(ReviewPolicy::OnUncertain.reviews_on_uncertain());
        assert!(ReviewPolicy::OnEscalationOrUncertain.reviews_on_uncertain());
    }

    #[test]
    fn test_review_policy_always_reviews() {
        assert!(!ReviewPolicy::Never.always_reviews());
        assert!(ReviewPolicy::Always.always_reviews());
        assert!(!ReviewPolicy::OnEscalation.always_reviews());
        assert!(!ReviewPolicy::OnUncertain.always_reviews());
        assert!(!ReviewPolicy::OnEscalationOrUncertain.always_reviews());
    }

    #[test]
    fn test_review_policy_debug() {
        let policy = ReviewPolicy::OnEscalationOrUncertain;
        let debug = format!("{:?}", policy);
        assert!(debug.contains("OnEscalationOrUncertain"));
    }

    #[test]
    fn test_review_policy_all_variants_exist() {
        // Exhaustive match ensures all variants are covered.
        let policies = [
            ReviewPolicy::Never,
            ReviewPolicy::Always,
            ReviewPolicy::OnEscalation,
            ReviewPolicy::OnUncertain,
            ReviewPolicy::OnEscalationOrUncertain,
        ];
        assert_eq!(policies.len(), 5);
    }

    // --- ReviewOutcome tests ---

    // Test artefact for ReviewOutcome::ApproveWithEdits
    struct TestArtefact {
        data: String,
    }

    impl Artefact for TestArtefact {
        fn as_any(&self) -> &dyn Any {
            self
        }

        fn debug_description(&self) -> String {
            format!("TestArtefact({})", self.data)
        }
    }

    #[test]
    fn test_review_outcome_approve() {
        let outcome = ReviewOutcome::Approve;
        assert!(matches!(outcome, ReviewOutcome::Approve));
    }

    #[test]
    fn test_review_outcome_reject() {
        let outcome = ReviewOutcome::Reject {
            reason: "Quality too low".to_string(),
        };
        match &outcome {
            ReviewOutcome::Reject { reason } => {
                assert_eq!(reason, "Quality too low");
            }
            _ => panic!("Expected Reject variant"),
        }
    }

    #[test]
    fn test_review_outcome_reject_empty_reason() {
        let outcome = ReviewOutcome::Reject {
            reason: String::new(),
        };
        match &outcome {
            ReviewOutcome::Reject { reason } => {
                assert!(reason.is_empty());
            }
            _ => panic!("Expected Reject variant"),
        }
    }

    #[test]
    fn test_review_outcome_approve_with_edits() {
        let artefact = TestArtefact {
            data: "edited content".to_string(),
        };
        let outcome = ReviewOutcome::ApproveWithEdits {
            edited_artefacts: Box::new(artefact),
            note: Some("Fixed table formatting".to_string()),
        };
        match &outcome {
            ReviewOutcome::ApproveWithEdits {
                edited_artefacts,
                note,
            } => {
                assert_eq!(note.as_deref(), Some("Fixed table formatting"));
                let downcasted = edited_artefacts
                    .as_any()
                    .downcast_ref::<TestArtefact>()
                    .unwrap();
                assert_eq!(downcasted.data, "edited content");
            }
            _ => panic!("Expected ApproveWithEdits variant"),
        }
    }

    #[test]
    fn test_review_outcome_approve_with_edits_no_note() {
        let artefact = TestArtefact {
            data: "edited".to_string(),
        };
        let outcome = ReviewOutcome::ApproveWithEdits {
            edited_artefacts: Box::new(artefact),
            note: None,
        };
        match &outcome {
            ReviewOutcome::ApproveWithEdits { note, .. } => {
                assert!(note.is_none());
            }
            _ => panic!("Expected ApproveWithEdits variant"),
        }
    }

    // --- Debug tests ---

    #[test]
    fn test_review_outcome_debug_approve() {
        let outcome = ReviewOutcome::Approve;
        let debug = format!("{:?}", outcome);
        assert!(debug.contains("Approve"));
    }

    #[test]
    fn test_review_outcome_debug_reject() {
        let outcome = ReviewOutcome::Reject {
            reason: "bad output".to_string(),
        };
        let debug = format!("{:?}", outcome);
        assert!(debug.contains("Reject"));
        assert!(debug.contains("bad output"));
    }

    #[test]
    fn test_review_outcome_debug_approve_with_edits() {
        let artefact = TestArtefact {
            data: "test".to_string(),
        };
        let outcome = ReviewOutcome::ApproveWithEdits {
            edited_artefacts: Box::new(artefact),
            note: Some("note".to_string()),
        };
        let debug = format!("{:?}", outcome);
        assert!(debug.contains("ApproveWithEdits"));
        assert!(debug.contains("TestArtefact"));
        assert!(debug.contains("note"));
    }

    // --- Send + Sync tests ---

    #[test]
    fn test_review_policy_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ReviewPolicy>();
    }

    #[test]
    fn test_review_outcome_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ReviewOutcome>();
    }
}
```

**Design Decisions:**
- `ReviewPolicy` uses five named variants with no `Custom` boolean variant, following the TD-01 principle: arguments use types, not bools. This avoids the ambiguity of a `Custom { on_escalation: bool, on_uncertain: bool, always: bool }` variant. If new combinations are needed, they are added as named variants.
- Convenience methods (`reviews_on_escalation()`, `reviews_on_uncertain()`, `always_reviews()`) encode the semantics of each variant and will be used by the executor in Phase 10 to determine whether to block for review.
- `ReviewOutcome` has a manual `Debug` impl because `Box<dyn Artefact>` does not implement `Debug`. It delegates to `debug_description()` on the artefact, consistent with `StageOutput`'s approach (Phase 6).
- `ReviewPolicy` derives `Copy` because it is a small, stateless enum. `ReviewOutcome` cannot be `Copy` or `Clone` because the `ApproveWithEdits` variant holds a `Box<dyn Artefact>`.
- `Default` for `ReviewPolicy` is `Never`, matching v1 behaviour where no human review is configured at the workflow level.

#### 8.2.2 Update `src/lib.rs`

Add module and re-exports:

```rust
pub mod review;

pub use review::{ReviewOutcome, ReviewPolicy};
```

### Verification Commands

```bash
cargo build
cargo test review::tests
cargo clippy
```

---

## Milestone 8.3 -- resolve_review Method and Convenience Wrappers

### Files to Modify
- `src/workflow.rs` (add `resolve_review`, `approve_review`, `reject_review`)
- `src/error.rs` (add `InvalidReview` error variant if needed)

### Implementation Details

#### 8.3.1 Add `InvalidReview` Error Variant

Add to `src/error.rs`:

```rust
/// A review operation was invalid (e.g., stage not awaiting review).
#[error("Invalid review: {0}")]
InvalidReview(String),
```

#### 8.3.2 Add `resolve_review` to `Workflow`

Add to `src/workflow.rs`:

```rust
use crate::review::ReviewOutcome;

impl Workflow {
    /// Resolves a pending review for a work item at a specific stage.
    ///
    /// This method handles three outcomes:
    ///
    /// - [`ReviewOutcome::Approve`]: Marks the stage as complete and allows
    ///   the pipeline to continue.
    /// - [`ReviewOutcome::Reject`]: Marks the stage as failed with the
    ///   given reason.
    /// - [`ReviewOutcome::ApproveWithEdits`]: Marks the stage as complete
    ///   and stores the edited artefacts for downstream consumption.
    ///
    /// # Errors
    ///
    /// - [`TreadleError::StageNotFound`] if the stage does not exist
    ///   in this workflow.
    /// - [`TreadleError::InvalidReview`] if the stage is not currently
    ///   paused/awaiting review.
    /// - [`TreadleError::StateStore`] if persistence fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// workflow.resolve_review(
    ///     "item-1",
    ///     "extract_concepts",
    ///     ReviewOutcome::Approve,
    ///     &mut store,
    /// ).await?;
    /// ```
    pub async fn resolve_review<S: StateStore>(
        &self,
        item_id: &str,
        stage: &str,
        outcome: ReviewOutcome,
        store: &mut S,
    ) -> Result<()> {
        // Validate stage exists in workflow
        if !self.has_stage(stage) {
            return Err(TreadleError::StageNotFound(stage.to_string()));
        }

        // Validate stage is awaiting review (paused)
        let current_state = store.get_stage_state(item_id, stage).await?;
        match &current_state {
            Some(state) if matches!(state.status, crate::StageStatus::Paused) => {
                // Valid -- stage is paused for review
            }
            Some(state) => {
                return Err(TreadleError::InvalidReview(format!(
                    "stage '{}' for item '{}' is in {:?} status, not Paused",
                    stage, item_id, state.status,
                )));
            }
            None => {
                return Err(TreadleError::InvalidReview(format!(
                    "no state found for stage '{}' on item '{}'",
                    stage, item_id,
                )));
            }
        }

        let mut state = current_state.unwrap();

        match outcome {
            ReviewOutcome::Approve => {
                state.mark_complete();
                store.save_stage_state(item_id, stage, &state).await?;

                self.emit(WorkflowEvent::StageCompleted {
                    item_id: item_id.to_string(),
                    stage: stage.to_string(),
                });
            }
            ReviewOutcome::Reject { reason } => {
                state.mark_failed(reason.clone());
                store.save_stage_state(item_id, stage, &state).await?;

                self.emit(WorkflowEvent::StageFailed {
                    item_id: item_id.to_string(),
                    stage: stage.to_string(),
                    error: reason,
                });
            }
            ReviewOutcome::ApproveWithEdits {
                edited_artefacts: _,
                note,
            } => {
                // Mark stage complete. In Phase 10 (quality gate integration),
                // the edited artefacts will be stored for downstream consumption.
                // For now, we just log the note in the stage state if present.
                state.mark_complete();
                if let Some(note_text) = note {
                    state.error = Some(format!("[review note] {}", note_text));
                }
                store.save_stage_state(item_id, stage, &state).await?;

                self.emit(WorkflowEvent::StageCompleted {
                    item_id: item_id.to_string(),
                    stage: stage.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Convenience wrapper: approves a pending review.
    ///
    /// Equivalent to `resolve_review(item_id, stage, ReviewOutcome::Approve, store)`.
    ///
    /// # Errors
    ///
    /// Same as [`resolve_review`](Workflow::resolve_review).
    pub async fn approve_review<S: StateStore>(
        &self,
        item_id: &str,
        stage: &str,
        store: &mut S,
    ) -> Result<()> {
        self.resolve_review(item_id, stage, ReviewOutcome::Approve, store)
            .await
    }

    /// Convenience wrapper: rejects a pending review with a reason.
    ///
    /// Equivalent to `resolve_review(item_id, stage, ReviewOutcome::Reject { reason }, store)`.
    ///
    /// # Errors
    ///
    /// Same as [`resolve_review`](Workflow::resolve_review).
    pub async fn reject_review<S: StateStore>(
        &self,
        item_id: &str,
        stage: &str,
        reason: String,
        store: &mut S,
    ) -> Result<()> {
        self.resolve_review(item_id, stage, ReviewOutcome::Reject { reason }, store)
            .await
    }
}
```

#### 8.3.3 Tests for resolve_review

Add to `src/workflow.rs` tests module:

```rust
mod resolve_review_tests {
    use super::*;

    #[tokio::test]
    async fn test_resolve_review_approve_paused_stage() {
        let workflow = Workflow::builder()
            .stage("review", TestStage::new("review"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        // Set stage to Paused (awaiting review)
        let mut state = crate::StageState::new();
        state.mark_paused();
        store
            .save_stage_state(item_id, "review", &state)
            .await
            .unwrap();

        // Approve the review
        workflow
            .approve_review(item_id, "review", &mut store)
            .await
            .unwrap();

        // Stage should now be Complete
        let final_state = store
            .get_stage_state(item_id, "review")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(final_state.status, crate::StageStatus::Complete));
    }

    #[tokio::test]
    async fn test_resolve_review_reject_paused_stage() {
        let workflow = Workflow::builder()
            .stage("review", TestStage::new("review"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        // Set stage to Paused
        let mut state = crate::StageState::new();
        state.mark_paused();
        store
            .save_stage_state(item_id, "review", &state)
            .await
            .unwrap();

        // Reject the review
        workflow
            .reject_review(item_id, "review", "Not good enough".to_string(), &mut store)
            .await
            .unwrap();

        // Stage should now be Failed
        let final_state = store
            .get_stage_state(item_id, "review")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(final_state.status, crate::StageStatus::Failed));
        assert_eq!(final_state.error, Some("Not good enough".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_review_approve_with_edits() {
        let workflow = Workflow::builder()
            .stage("review", TestStage::new("review"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        // Set stage to Paused
        let mut state = crate::StageState::new();
        state.mark_paused();
        store
            .save_stage_state(item_id, "review", &state)
            .await
            .unwrap();

        // Approve with edits
        let outcome = ReviewOutcome::ApproveWithEdits {
            edited_artefacts: Box::new(serde_json::json!({"fixed": true})),
            note: Some("Corrected table layout".to_string()),
        };
        workflow
            .resolve_review(item_id, "review", outcome, &mut store)
            .await
            .unwrap();

        // Stage should now be Complete
        let final_state = store
            .get_stage_state(item_id, "review")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(final_state.status, crate::StageStatus::Complete));
    }

    #[tokio::test]
    async fn test_resolve_review_approve_with_edits_no_note() {
        let workflow = Workflow::builder()
            .stage("review", TestStage::new("review"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        let mut state = crate::StageState::new();
        state.mark_paused();
        store
            .save_stage_state(item_id, "review", &state)
            .await
            .unwrap();

        let outcome = ReviewOutcome::ApproveWithEdits {
            edited_artefacts: Box::new(serde_json::json!(42)),
            note: None,
        };
        workflow
            .resolve_review(item_id, "review", outcome, &mut store)
            .await
            .unwrap();

        let final_state = store
            .get_stage_state(item_id, "review")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(final_state.status, crate::StageStatus::Complete));
        // No note means no error field set
        assert!(final_state.error.is_none());
    }

    #[tokio::test]
    async fn test_resolve_review_stage_not_found() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let result = workflow
            .approve_review("item-1", "nonexistent", &mut store)
            .await;

        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    #[tokio::test]
    async fn test_resolve_review_stage_not_paused() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        // Set stage to InProgress (not Paused)
        let mut state = crate::StageState::new();
        state.mark_in_progress();
        store
            .save_stage_state(item_id, "a", &state)
            .await
            .unwrap();

        let result = workflow
            .approve_review(item_id, "a", &mut store)
            .await;

        assert!(matches!(result, Err(TreadleError::InvalidReview(_))));
    }

    #[tokio::test]
    async fn test_resolve_review_stage_no_state() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();

        // No state saved -- fresh item
        let result = workflow
            .approve_review("item-1", "a", &mut store)
            .await;

        assert!(matches!(result, Err(TreadleError::InvalidReview(_))));
    }

    #[tokio::test]
    async fn test_resolve_review_stage_complete_is_invalid() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        // Set stage to Complete
        let mut state = crate::StageState::new();
        state.mark_complete();
        store
            .save_stage_state(item_id, "a", &state)
            .await
            .unwrap();

        let result = workflow
            .approve_review(item_id, "a", &mut store)
            .await;

        assert!(matches!(result, Err(TreadleError::InvalidReview(_))));
    }

    #[tokio::test]
    async fn test_resolve_review_stage_failed_is_invalid() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        let mut state = crate::StageState::new();
        state.mark_failed("previous failure".to_string());
        store
            .save_stage_state(item_id, "a", &state)
            .await
            .unwrap();

        let result = workflow
            .approve_review(item_id, "a", &mut store)
            .await;

        assert!(matches!(result, Err(TreadleError::InvalidReview(_))));
    }

    #[tokio::test]
    async fn test_resolve_review_emits_completed_event_on_approve() {
        let workflow = Workflow::builder()
            .stage("review", TestStage::new("review"))
            .build()
            .unwrap();

        let mut receiver = workflow.subscribe();
        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        let mut state = crate::StageState::new();
        state.mark_paused();
        store
            .save_stage_state(item_id, "review", &state)
            .await
            .unwrap();

        workflow
            .approve_review(item_id, "review", &mut store)
            .await
            .unwrap();

        let event = receiver.try_recv().unwrap();
        assert!(matches!(
            event,
            WorkflowEvent::StageCompleted { .. }
        ));
        assert_eq!(event.stage(), Some("review"));
    }

    #[tokio::test]
    async fn test_resolve_review_emits_failed_event_on_reject() {
        let workflow = Workflow::builder()
            .stage("review", TestStage::new("review"))
            .build()
            .unwrap();

        let mut receiver = workflow.subscribe();
        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        let mut state = crate::StageState::new();
        state.mark_paused();
        store
            .save_stage_state(item_id, "review", &state)
            .await
            .unwrap();

        workflow
            .reject_review(item_id, "review", "reason".to_string(), &mut store)
            .await
            .unwrap();

        let event = receiver.try_recv().unwrap();
        assert!(matches!(event, WorkflowEvent::StageFailed { .. }));
    }

    #[tokio::test]
    async fn test_approve_then_advance_continues_pipeline() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // Simulate stage a being paused for review
        let mut state = crate::StageState::new();
        state.mark_paused();
        store
            .save_stage_state(&item.id, "a", &state)
            .await
            .unwrap();

        // Approve the review
        workflow
            .approve_review(&item.id, "a", &mut store)
            .await
            .unwrap();

        // Now advance should execute stage b
        workflow.advance(&item, &mut store).await.unwrap();

        let b_state = store
            .get_stage_state(&item.id, "b")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(b_state.status, crate::StageStatus::Complete));
    }
}
```

### Verification Commands

```bash
cargo build
cargo test workflow::tests::resolve_review_tests
cargo clippy
```

---

## Milestone 8.4 -- WorkflowBuilder Integration

### Files to Modify
- `src/workflow.rs` (extend `WorkflowBuilder` and `Workflow`)

### Implementation Details

#### 8.4.1 Add Storage Fields to `Workflow`

Extend the `Workflow` struct to store per-stage retry budgets and review policies:

```rust
use crate::retry::RetryBudget;
use crate::review::ReviewPolicy;

pub struct Workflow {
    /// The underlying directed graph.
    graph: DiGraph<RegisteredStage, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Cached topological order of stage names.
    topo_order: Vec<String>,
    /// Event broadcast channel sender.
    event_tx: broadcast::Sender<WorkflowEvent>,

    // --- New in Phase 8 ---

    /// Per-stage retry budgets, keyed by stage name.
    retry_budgets: HashMap<String, RetryBudget>,
    /// Per-stage review policies, keyed by stage name.
    review_policies: HashMap<String, ReviewPolicy>,
}
```

#### 8.4.2 Add Getter Methods to `Workflow`

```rust
impl Workflow {
    /// Returns the retry budget for a stage, or the default budget
    /// if none was explicitly configured.
    ///
    /// The default budget matches v1 behaviour: 1 attempt, no delay,
    /// no timeout, fail on exhaustion.
    ///
    /// # Errors
    ///
    /// Returns [`TreadleError::StageNotFound`] if the stage does not exist.
    pub fn get_retry_budget(&self, stage: &str) -> Result<RetryBudget> {
        if !self.has_stage(stage) {
            return Err(TreadleError::StageNotFound(stage.to_string()));
        }
        Ok(self
            .retry_budgets
            .get(stage)
            .cloned()
            .unwrap_or_default())
    }

    /// Returns the review policy for a stage, or the default policy
    /// if none was explicitly configured.
    ///
    /// The default policy is [`ReviewPolicy::Never`], matching v1 behaviour.
    ///
    /// # Errors
    ///
    /// Returns [`TreadleError::StageNotFound`] if the stage does not exist.
    pub fn get_review_policy(&self, stage: &str) -> Result<ReviewPolicy> {
        if !self.has_stage(stage) {
            return Err(TreadleError::StageNotFound(stage.to_string()));
        }
        Ok(self
            .review_policies
            .get(stage)
            .copied()
            .unwrap_or_default())
    }
}
```

#### 8.4.3 Add Deferred Budget/Policy Storage to `WorkflowBuilder`

```rust
/// A deferred retry budget assignment.
struct DeferredRetryBudget {
    stage: String,
    budget: RetryBudget,
}

/// A deferred review policy assignment.
struct DeferredReviewPolicy {
    stage: String,
    policy: ReviewPolicy,
}

pub struct WorkflowBuilder {
    /// The graph being built.
    graph: DiGraph<RegisteredStage, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Deferred dependencies to be resolved at build time.
    deferred_deps: Vec<DeferredDependency>,

    // --- New in Phase 8 ---

    /// Deferred retry budget assignments to be validated at build time.
    deferred_retry_budgets: Vec<DeferredRetryBudget>,
    /// Deferred review policy assignments to be validated at build time.
    deferred_review_policies: Vec<DeferredReviewPolicy>,
}
```

#### 8.4.4 Add Builder Methods

```rust
impl WorkflowBuilder {
    /// Assigns a retry budget to a stage.
    ///
    /// The stage name is validated at build time -- if the stage does not
    /// exist, [`build()`](WorkflowBuilder::build) returns an error.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use treadle::{Workflow, RetryBudget, ExhaustedAction};
    ///
    /// let workflow = Workflow::builder()
    ///     .stage("scan", ScanStage)
    ///     .retry_budget("scan", RetryBudget::new(3)
    ///         .with_on_exhausted(ExhaustedAction::Escalate))
    ///     .build()?;
    /// ```
    pub fn retry_budget(
        mut self,
        stage: impl Into<String>,
        budget: RetryBudget,
    ) -> Self {
        self.deferred_retry_budgets.push(DeferredRetryBudget {
            stage: stage.into(),
            budget,
        });
        self
    }

    /// Assigns a review policy to a stage.
    ///
    /// The stage name is validated at build time -- if the stage does not
    /// exist, [`build()`](WorkflowBuilder::build) returns an error.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use treadle::{Workflow, ReviewPolicy};
    ///
    /// let workflow = Workflow::builder()
    ///     .stage("final_review", FinalReviewStage)
    ///     .review_policy("final_review", ReviewPolicy::Always)
    ///     .build()?;
    /// ```
    pub fn review_policy(
        mut self,
        stage: impl Into<String>,
        policy: ReviewPolicy,
    ) -> Self {
        self.deferred_review_policies.push(DeferredReviewPolicy {
            stage: stage.into(),
            policy,
        });
        self
    }
}
```

#### 8.4.5 Update `build()` to Validate and Store Budgets/Policies

Update the `build()` method to validate that stage names in retry budgets and review policies refer to existing stages:

```rust
pub fn build(mut self) -> Result<Workflow> {
    // ... existing dependency resolution and cycle detection ...

    // Validate and collect retry budgets
    let mut retry_budgets = HashMap::new();
    for deferred in &self.deferred_retry_budgets {
        if !self.name_to_index.contains_key(&deferred.stage) {
            return Err(TreadleError::StageNotFound(deferred.stage.clone()));
        }
        retry_budgets.insert(deferred.stage.clone(), deferred.budget.clone());
    }

    // Validate and collect review policies
    let mut review_policies = HashMap::new();
    for deferred in &self.deferred_review_policies {
        if !self.name_to_index.contains_key(&deferred.stage) {
            return Err(TreadleError::StageNotFound(deferred.stage.clone()));
        }
        review_policies.insert(deferred.stage.clone(), deferred.policy);
    }

    // Create event broadcast channel
    let (event_tx, _) = broadcast::channel(DEFAULT_EVENT_CHANNEL_CAPACITY);

    Ok(Workflow {
        graph: self.graph,
        name_to_index: self.name_to_index,
        topo_order,
        event_tx,
        retry_budgets,
        review_policies,
    })
}
```

#### 8.4.6 Update `WorkflowBuilder::new()` and `Default`

```rust
impl WorkflowBuilder {
    fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            name_to_index: HashMap::new(),
            deferred_deps: Vec::new(),
            deferred_retry_budgets: Vec::new(),
            deferred_review_policies: Vec::new(),
        }
    }
}
```

#### 8.4.7 Tests for WorkflowBuilder Integration

```rust
mod builder_retry_policy_tests {
    use super::*;
    use crate::retry::{ExhaustedAction, RetryBudget};
    use crate::review::ReviewPolicy;

    // --- retry_budget() tests ---

    #[test]
    fn test_builder_retry_budget_valid_stage() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .retry_budget(
                "scan",
                RetryBudget::new(3).with_on_exhausted(ExhaustedAction::Escalate),
            )
            .build()
            .unwrap();

        let budget = workflow.get_retry_budget("scan").unwrap();
        assert_eq!(budget.max_attempts(), 3);
        assert_eq!(budget.on_exhausted(), ExhaustedAction::Escalate);
    }

    #[test]
    fn test_builder_retry_budget_nonexistent_stage() {
        let result = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .retry_budget("nonexistent", RetryBudget::new(3))
            .build();

        assert!(matches!(result, Err(TreadleError::StageNotFound(ref s)) if s == "nonexistent"));
    }

    #[test]
    fn test_builder_retry_budget_default_when_not_set() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .build()
            .unwrap();

        let budget = workflow.get_retry_budget("scan").unwrap();
        assert_eq!(budget, RetryBudget::default());
        assert_eq!(budget.max_attempts(), 1);
        assert_eq!(budget.on_exhausted(), ExhaustedAction::Fail);
        assert!(budget.delay().is_none());
        assert!(budget.attempt_timeout().is_none());
    }

    #[test]
    fn test_builder_retry_budget_multiple_stages() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .retry_budget("a", RetryBudget::new(3))
            .retry_budget(
                "b",
                RetryBudget::new(5)
                    .with_delay(std::time::Duration::from_secs(2)),
            )
            .build()
            .unwrap();

        let budget_a = workflow.get_retry_budget("a").unwrap();
        assert_eq!(budget_a.max_attempts(), 3);
        assert!(budget_a.delay().is_none());

        let budget_b = workflow.get_retry_budget("b").unwrap();
        assert_eq!(budget_b.max_attempts(), 5);
        assert_eq!(
            budget_b.delay(),
            Some(std::time::Duration::from_secs(2))
        );
    }

    #[test]
    fn test_builder_retry_budget_with_timeout() {
        let workflow = Workflow::builder()
            .stage("llm", TestStage::new("llm"))
            .retry_budget(
                "llm",
                RetryBudget::new(3)
                    .with_attempt_timeout(std::time::Duration::from_secs(120))
                    .with_on_exhausted(ExhaustedAction::Escalate),
            )
            .build()
            .unwrap();

        let budget = workflow.get_retry_budget("llm").unwrap();
        assert_eq!(
            budget.attempt_timeout(),
            Some(std::time::Duration::from_secs(120))
        );
    }

    #[test]
    fn test_builder_retry_budget_last_wins() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .retry_budget("scan", RetryBudget::new(3))
            .retry_budget("scan", RetryBudget::new(5))
            .build()
            .unwrap();

        let budget = workflow.get_retry_budget("scan").unwrap();
        assert_eq!(budget.max_attempts(), 5);
    }

    // --- review_policy() tests ---

    #[test]
    fn test_builder_review_policy_valid_stage() {
        let workflow = Workflow::builder()
            .stage("review", TestStage::new("review"))
            .review_policy("review", ReviewPolicy::Always)
            .build()
            .unwrap();

        let policy = workflow.get_review_policy("review").unwrap();
        assert_eq!(policy, ReviewPolicy::Always);
    }

    #[test]
    fn test_builder_review_policy_nonexistent_stage() {
        let result = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .review_policy("nonexistent", ReviewPolicy::Always)
            .build();

        assert!(matches!(result, Err(TreadleError::StageNotFound(ref s)) if s == "nonexistent"));
    }

    #[test]
    fn test_builder_review_policy_default_when_not_set() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .build()
            .unwrap();

        let policy = workflow.get_review_policy("scan").unwrap();
        assert_eq!(policy, ReviewPolicy::Never);
    }

    #[test]
    fn test_builder_review_policy_multiple_stages() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .review_policy("a", ReviewPolicy::Never)
            .review_policy("b", ReviewPolicy::Always)
            .review_policy("c", ReviewPolicy::OnEscalation)
            .build()
            .unwrap();

        assert_eq!(
            workflow.get_review_policy("a").unwrap(),
            ReviewPolicy::Never
        );
        assert_eq!(
            workflow.get_review_policy("b").unwrap(),
            ReviewPolicy::Always
        );
        assert_eq!(
            workflow.get_review_policy("c").unwrap(),
            ReviewPolicy::OnEscalation
        );
    }

    #[test]
    fn test_builder_review_policy_all_variants() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .stage("d", TestStage::new("d"))
            .stage("e", TestStage::new("e"))
            .review_policy("a", ReviewPolicy::Never)
            .review_policy("b", ReviewPolicy::Always)
            .review_policy("c", ReviewPolicy::OnEscalation)
            .review_policy("d", ReviewPolicy::OnUncertain)
            .review_policy("e", ReviewPolicy::OnEscalationOrUncertain)
            .build()
            .unwrap();

        assert_eq!(
            workflow.get_review_policy("a").unwrap(),
            ReviewPolicy::Never
        );
        assert_eq!(
            workflow.get_review_policy("b").unwrap(),
            ReviewPolicy::Always
        );
        assert_eq!(
            workflow.get_review_policy("c").unwrap(),
            ReviewPolicy::OnEscalation
        );
        assert_eq!(
            workflow.get_review_policy("d").unwrap(),
            ReviewPolicy::OnUncertain
        );
        assert_eq!(
            workflow.get_review_policy("e").unwrap(),
            ReviewPolicy::OnEscalationOrUncertain
        );
    }

    #[test]
    fn test_builder_review_policy_last_wins() {
        let workflow = Workflow::builder()
            .stage("review", TestStage::new("review"))
            .review_policy("review", ReviewPolicy::Never)
            .review_policy("review", ReviewPolicy::Always)
            .build()
            .unwrap();

        let policy = workflow.get_review_policy("review").unwrap();
        assert_eq!(policy, ReviewPolicy::Always);
    }

    // --- Getter error tests ---

    #[test]
    fn test_get_retry_budget_stage_not_found() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let result = workflow.get_retry_budget("nonexistent");
        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    #[test]
    fn test_get_review_policy_stage_not_found() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let result = workflow.get_review_policy("nonexistent");
        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    // --- Combined builder tests ---

    #[test]
    fn test_builder_retry_budget_and_review_policy_together() {
        let workflow = Workflow::builder()
            .stage("pdf_to_md", TestStage::new("pdf_to_md"))
            .stage("extract", TestStage::new("extract"))
            .stage("final_review", TestStage::new("final_review"))
            .dependency("extract", "pdf_to_md")
            .dependency("final_review", "extract")
            .retry_budget(
                "pdf_to_md",
                RetryBudget::new(3)
                    .with_attempt_timeout(std::time::Duration::from_secs(120))
                    .with_on_exhausted(ExhaustedAction::Escalate),
            )
            .retry_budget(
                "extract",
                RetryBudget::new(2)
                    .with_on_exhausted(ExhaustedAction::Escalate),
            )
            .review_policy("pdf_to_md", ReviewPolicy::OnEscalation)
            .review_policy("extract", ReviewPolicy::OnEscalation)
            .review_policy("final_review", ReviewPolicy::Always)
            .build()
            .unwrap();

        // Verify retry budgets
        let pdf_budget = workflow.get_retry_budget("pdf_to_md").unwrap();
        assert_eq!(pdf_budget.max_attempts(), 3);
        assert_eq!(pdf_budget.on_exhausted(), ExhaustedAction::Escalate);
        assert_eq!(
            pdf_budget.attempt_timeout(),
            Some(std::time::Duration::from_secs(120))
        );

        let extract_budget = workflow.get_retry_budget("extract").unwrap();
        assert_eq!(extract_budget.max_attempts(), 2);

        // final_review has no explicit retry budget -- should be default
        let review_budget = workflow.get_retry_budget("final_review").unwrap();
        assert_eq!(review_budget, RetryBudget::default());

        // Verify review policies
        assert_eq!(
            workflow.get_review_policy("pdf_to_md").unwrap(),
            ReviewPolicy::OnEscalation
        );
        assert_eq!(
            workflow.get_review_policy("extract").unwrap(),
            ReviewPolicy::OnEscalation
        );
        assert_eq!(
            workflow.get_review_policy("final_review").unwrap(),
            ReviewPolicy::Always
        );
    }

    #[test]
    fn test_builder_retry_budget_and_dependency_validation() {
        // Both the dependency and the retry budget reference a nonexistent stage.
        // The dependency validation runs first.
        let result = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .dependency("b", "a") // b does not exist
            .retry_budget("a", RetryBudget::new(3))
            .build();

        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    #[test]
    fn test_builder_empty_workflow_with_no_budgets_or_policies() {
        let workflow = Workflow::builder().build().unwrap();
        assert_eq!(workflow.stage_count(), 0);
        // No stages to query, so getters would return StageNotFound
    }

    #[test]
    fn test_builder_stages_before_retry_budget() {
        // Retry budget can be declared before the stage is added,
        // as long as the stage exists at build time.
        let workflow = Workflow::builder()
            .retry_budget("scan", RetryBudget::new(3))
            .stage("scan", TestStage::new("scan"))
            .build()
            .unwrap();

        let budget = workflow.get_retry_budget("scan").unwrap();
        assert_eq!(budget.max_attempts(), 3);
    }

    #[test]
    fn test_builder_stages_before_review_policy() {
        // Review policy can be declared before the stage is added.
        let workflow = Workflow::builder()
            .review_policy("review", ReviewPolicy::Always)
            .stage("review", TestStage::new("review"))
            .build()
            .unwrap();

        let policy = workflow.get_review_policy("review").unwrap();
        assert_eq!(policy, ReviewPolicy::Always);
    }
}
```

### Verification Commands

```bash
cargo build
cargo test workflow::tests::builder_retry_policy_tests
cargo test
cargo clippy
```

---

## Phase 8 Completion Checklist

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes all tests (including all existing v1 tests)
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `cargo doc --no-deps` builds without warnings
- [ ] `RetryBudget` struct defined with private fields and builder methods
- [ ] `RetryBudget::default()` matches v1 behaviour (1 attempt, fail, no delay, no timeout)
- [ ] `RetryBudget::new(0)` panics (at least 1 attempt required)
- [ ] `ExhaustedAction` enum defined (`Fail`, `Escalate`)
- [ ] `ReviewPolicy` enum defined with five named variants (no `Custom`)
- [ ] `ReviewPolicy::default()` is `Never`
- [ ] `ReviewPolicy` convenience methods: `reviews_on_escalation()`, `reviews_on_uncertain()`, `always_reviews()`
- [ ] `ReviewOutcome` enum defined (`Approve`, `Reject`, `ApproveWithEdits`)
- [ ] `ReviewOutcome` has manual `Debug` impl
- [ ] `resolve_review` method on `Workflow`
- [ ] `approve_review` convenience wrapper
- [ ] `reject_review` convenience wrapper
- [ ] `resolve_review` validates stage exists
- [ ] `resolve_review` validates stage is in Paused status
- [ ] `resolve_review` emits correct events
- [ ] `InvalidReview` error variant added to `TreadleError`
- [ ] `.retry_budget()` on `WorkflowBuilder` (validates stage at build time)
- [ ] `.review_policy()` on `WorkflowBuilder` (validates stage at build time)
- [ ] `get_retry_budget()` on `Workflow` (returns default if not configured)
- [ ] `get_review_policy()` on `Workflow` (returns default if not configured)
- [ ] `Workflow` stores `retry_budgets: HashMap<String, RetryBudget>`
- [ ] `Workflow` stores `review_policies: HashMap<String, ReviewPolicy>`
- [ ] All existing tests pass unchanged

### Public API After Phase 8

```rust
// New in Phase 8
treadle::RetryBudget              // Struct (with builder methods)
treadle::ExhaustedAction          // Enum (Fail, Escalate)
treadle::ReviewPolicy             // Enum (Never, Always, OnEscalation, OnUncertain, OnEscalationOrUncertain)
treadle::ReviewOutcome            // Enum (Approve, Reject, ApproveWithEdits)

// New error variant
treadle::TreadleError::InvalidReview  // For invalid review operations

// New methods on Workflow
Workflow::resolve_review()        // Resolve a pending review
Workflow::approve_review()        // Convenience: approve
Workflow::reject_review()         // Convenience: reject
Workflow::get_retry_budget()      // Get per-stage retry budget (or default)
Workflow::get_review_policy()     // Get per-stage review policy (or default)

// New methods on WorkflowBuilder
WorkflowBuilder::retry_budget()   // Assign retry budget to a stage
WorkflowBuilder::review_policy()  // Assign review policy to a stage
```

---

## Notes on Rust Guidelines Applied

### Anti-Patterns Avoided
- **AP-01 (Boolean arguments):** `ReviewPolicy` uses five named variants, not a `Custom { on_escalation: bool, on_uncertain: bool, always: bool }` variant. This follows TD-01 and avoids ambiguous boolean combinations.
- **AP-03 (Missing Send + Sync bounds):** `RetryBudget` and `ReviewPolicy` are `Send + Sync` by derivation. `ReviewOutcome` is `Send + Sync` because `Box<dyn Artefact>` requires `Send + Sync` on the trait.
- **AP-09 (Unwrap in library code):** No `unwrap()` in production code. The `resolve_review` method returns proper errors for all invalid states.

### Core Idioms Applied
- **ID-04 (`impl Into<String>`):** Builder methods accept `impl Into<String>` for stage names.
- **ID-09 (`new()` as canonical constructor):** `RetryBudget::new(max_attempts)` enforces invariants.
- **ID-35 (Builder returning `Self`):** All builder methods on `RetryBudget` and `WorkflowBuilder` return `Self` for chaining.
- **Non-exhaustive consideration:** `ReviewPolicy` and `ExhaustedAction` do not use `#[non_exhaustive]` because they are consumed internally by the executor, and exhaustive matching is desirable within the crate. If they need to be extensible for downstream crates, `#[non_exhaustive]` can be added later.

### Type Design
- **TD-01:** `ReviewPolicy` avoids boolean flags entirely. Each variant has clear, unambiguous semantics.
- **Private fields with accessors:** `RetryBudget` uses private fields with getter methods to enforce the `max_attempts >= 1` invariant after construction.
- **Manual `Debug`:** `ReviewOutcome` has a manual `Debug` impl because `Box<dyn Artefact>` does not implement `Debug`, consistent with the approach used for `StageOutput` in Phase 6.
- **`Copy` where appropriate:** `ExhaustedAction` and `ReviewPolicy` are small enums that derive `Copy`. `RetryBudget` implements `Clone` but not `Copy` because it contains `Option<Duration>` (which is `Copy`, but the struct is moderately large). `ReviewOutcome` is neither `Copy` nor `Clone` due to `Box<dyn Artefact>`.
