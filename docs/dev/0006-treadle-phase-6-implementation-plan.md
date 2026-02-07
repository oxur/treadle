# Treadle Phase 6 Implementation Plan

## Phase 6: StageOutput and Artefact Passing

**Goal:** Introduce the `Artefact` trait, revise `Stage::execute` to return `StageOutput`, extend `StageContext` with upstream artefacts and feedback, and add artefact storage to `StateStore`. Backward compatibility with v1 stages is preserved via `From<StageOutcome> for StageOutput`.

**Prerequisites:**
- Phases 1–5 completed (core types, state stores, workflow builder, executor, status)
- `cargo build` and `cargo test` pass
- The `unsafe impl Send/Sync` lines in `src/workflow.rs` have been removed (see v2.1 design §2.12)

---

## Milestone 6.1 — Artefact Trait

### Files to Create/Modify
- `src/artefact.rs` (new)
- `src/lib.rs` (add module, re-exports)

### Implementation Details

#### 6.1.1 Create `src/artefact.rs`

```rust
//! Artefact types for stage output.
//!
//! This module defines the [`Artefact`] trait, which represents the output
//! produced by a stage. Artefacts are opaque to the engine but meaningful
//! to quality gates and downstream stages.

use std::any::Any;
use std::fmt::Debug;

/// An artefact produced by a stage.
///
/// Artefacts are the primary output of stage execution. The engine does
/// not inspect artefacts directly — it passes them to quality gates and
/// makes them available to downstream stages via [`StageContext::upstream_artefacts`].
///
/// # Serialisation
///
/// Artefacts may optionally serialise to JSON for persistence in the
/// [`StateStore`](crate::StateStore). The default implementation of
/// [`to_json`](Artefact::to_json) returns `None`, meaning the artefact
/// is opaque and will not be persisted. Override this method for artefacts
/// that should be stored in attempt records.
///
/// # Storage Boundary
///
/// The framework stores artefact *metadata and summaries* (via `to_json()`),
/// never the artefact payload itself. If a stage produces a large file,
/// the framework stores `None` (or a reference/metadata JSON) in the
/// `AttemptRecord`. The consuming application manages actual artefact storage.
///
/// # Downcasting
///
/// Quality gates and downstream stages that know the expected artefact type
/// can downcast via [`as_any`](Artefact::as_any):
///
/// ```rust,ignore
/// let markdown = artefact.as_any()
///     .downcast_ref::<MarkdownOutput>()
///     .expect("expected MarkdownOutput artefact");
/// ```
pub trait Artefact: Send + Sync + 'static {
    /// Attempts to serialise this artefact to JSON.
    ///
    /// Returns `None` if this artefact is opaque (binary, large, or not
    /// meaningfully serialisable). The returned value is stored in the
    /// `AttemptRecord` for persistence and review UIs.
    fn to_json(&self) -> Option<serde_json::Value> {
        None
    }

    /// Returns a reference to `self` as `&dyn Any` for downcasting.
    fn as_any(&self) -> &dyn Any;

    /// Returns a debug description of this artefact.
    ///
    /// Used by `StageOutput`'s `Debug` implementation.
    fn debug_description(&self) -> String {
        std::any::type_name::<Self>().to_string()
    }
}

/// Blanket `Artefact` implementation for `serde_json::Value`.
///
/// JSON values are always serialisable, so `to_json()` returns `Some(self.clone())`.
impl Artefact for serde_json::Value {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn debug_description(&self) -> String {
        format!("serde_json::Value({})", self)
    }
}

/// A wrapper that makes any `Serialize + Send + Sync + 'static` type
/// into an `Artefact` with automatic JSON serialisation.
///
/// # Example
///
/// ```rust,ignore
/// use treadle::SerializableArtefact;
///
/// #[derive(serde::Serialize, Debug, Clone)]
/// struct MyOutput { text: String }
///
/// let artefact = SerializableArtefact::new(MyOutput { text: "hello".into() });
/// assert!(artefact.to_json().is_some());
/// ```
pub struct SerializableArtefact<T: serde::Serialize + Send + Sync + 'static> {
    inner: T,
}

impl<T: serde::Serialize + Send + Sync + 'static> SerializableArtefact<T> {
    /// Wraps a serialisable value as an artefact.
    pub fn new(inner: T) -> Self {
        Self { inner }
    }

    /// Returns a reference to the inner value.
    pub fn inner(&self) -> &T {
        &self.inner
    }

    /// Consumes the wrapper, returning the inner value.
    pub fn into_inner(self) -> T {
        self.inner
    }
}

impl<T: serde::Serialize + Send + Sync + 'static> Artefact for SerializableArtefact<T> {
    fn to_json(&self) -> Option<serde_json::Value> {
        serde_json::to_value(&self.inner).ok()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn debug_description(&self) -> String {
        format!("SerializableArtefact<{}>", std::any::type_name::<T>())
    }
}

impl<T: serde::Serialize + Debug + Send + Sync + 'static> Debug for SerializableArtefact<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerializableArtefact")
            .field("inner", &self.inner)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A simple opaque artefact (no JSON serialisation)
    struct OpaqueData {
        _bytes: Vec<u8>,
    }

    impl Artefact for OpaqueData {
        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    // A typed artefact with custom JSON serialisation
    #[derive(Debug, Clone)]
    struct MarkdownOutput {
        text: String,
        table_count: usize,
    }

    impl Artefact for MarkdownOutput {
        fn to_json(&self) -> Option<serde_json::Value> {
            Some(serde_json::json!({
                "text_length": self.text.len(),
                "table_count": self.table_count,
            }))
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    #[test]
    fn test_opaque_artefact_no_json() {
        let artefact = OpaqueData { _bytes: vec![1, 2, 3] };
        assert!(artefact.to_json().is_none());
    }

    #[test]
    fn test_opaque_artefact_downcast() {
        let artefact: Box<dyn Artefact> = Box::new(OpaqueData { _bytes: vec![1, 2, 3] });
        let downcasted = artefact.as_any().downcast_ref::<OpaqueData>();
        assert!(downcasted.is_some());
    }

    #[test]
    fn test_typed_artefact_json() {
        let artefact = MarkdownOutput {
            text: "hello world".into(),
            table_count: 2,
        };
        let json = artefact.to_json().unwrap();
        assert_eq!(json["text_length"], 11);
        assert_eq!(json["table_count"], 2);
    }

    #[test]
    fn test_typed_artefact_downcast() {
        let artefact: Box<dyn Artefact> = Box::new(MarkdownOutput {
            text: "test".into(),
            table_count: 0,
        });
        let md = artefact.as_any().downcast_ref::<MarkdownOutput>().unwrap();
        assert_eq!(md.text, "test");
    }

    #[test]
    fn test_json_value_artefact() {
        let value = serde_json::json!({"key": "value"});
        let artefact: Box<dyn Artefact> = Box::new(value.clone());

        assert_eq!(artefact.to_json(), Some(value));
    }

    #[test]
    fn test_json_value_downcast() {
        let value = serde_json::json!(42);
        let artefact: Box<dyn Artefact> = Box::new(value);

        let downcasted = artefact.as_any().downcast_ref::<serde_json::Value>().unwrap();
        assert_eq!(*downcasted, serde_json::json!(42));
    }

    #[test]
    fn test_serializable_artefact() {
        #[derive(serde::Serialize, Debug)]
        struct Output {
            count: u32,
        }

        let artefact = SerializableArtefact::new(Output { count: 5 });
        let json = artefact.to_json().unwrap();
        assert_eq!(json["count"], 5);
    }

    #[test]
    fn test_serializable_artefact_downcast() {
        #[derive(serde::Serialize, Debug)]
        struct Output {
            count: u32,
        }

        let artefact: Box<dyn Artefact> = Box::new(SerializableArtefact::new(Output { count: 5 }));
        let inner = artefact
            .as_any()
            .downcast_ref::<SerializableArtefact<Output>>()
            .unwrap();
        assert_eq!(inner.inner().count, 5);
    }

    #[test]
    fn test_artefact_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Box<dyn Artefact>>();
    }

    #[test]
    fn test_debug_description_default() {
        let artefact = OpaqueData { _bytes: vec![] };
        let desc = artefact.debug_description();
        assert!(desc.contains("OpaqueData"));
    }
}
```

**Design Decisions:**
- `Artefact` trait with `to_json()` + `as_any()` provides opt-in serialisation and type-safe downcasting (v2.1 §2.1)
- `debug_description()` enables manual `Debug` impl on `StageOutput` without requiring `Debug` on all artefacts
- `SerializableArtefact<T>` wrapper makes it easy to wrap any `Serialize` type
- Blanket impl for `serde_json::Value` preserves backward compatibility with v1-style JSON artefacts
- Storage boundary documented: framework stores metadata only, never payloads (v2.1 §2.5)

#### 6.1.2 Update `src/lib.rs`

Add module and re-exports:

```rust
pub mod artefact;

pub use artefact::{Artefact, SerializableArtefact};
```

### Verification Commands

```bash
cargo build
cargo test artefact::tests
cargo clippy
```

---

## Milestone 6.2 — StageOutput and From<StageOutcome>

### Files to Modify
- `src/stage.rs` (add `StageOutput`, `From` impl)
- `src/lib.rs` (update exports)

### Implementation Details

#### 6.2.1 Add `StageOutput` to `src/stage.rs`

```rust
use crate::artefact::Artefact;

/// The output of a stage execution.
///
/// Stages return `StageOutput` instead of a bare `StageOutcome`. This wraps
/// the mechanical outcome with optional artefacts and a human-readable summary.
///
/// # Note on Trait Implementations
///
/// `StageOutput` cannot derive `Clone` or `PartialEq` because `Box<dyn Artefact>`
/// does not implement them. This is acceptable — the engine consumes `StageOutput`
/// values; it does not compare or duplicate them.
pub struct StageOutput {
    /// What happened mechanically (completed, skipped, fan-out, etc.).
    pub outcome: StageOutcome,

    /// The artefact(s) this stage produced, if any.
    ///
    /// Opaque to the engine; meaningful to quality gates and downstream stages.
    pub artefacts: Option<Box<dyn Artefact>>,

    /// Human-readable summary of what the stage did.
    pub summary: Option<String>,
}

impl StageOutput {
    /// Creates a `StageOutput` with an outcome and no artefacts.
    pub fn from_outcome(outcome: StageOutcome) -> Self {
        Self {
            outcome,
            artefacts: None,
            summary: None,
        }
    }

    /// Creates a completed output with artefacts.
    pub fn completed_with(artefacts: impl Artefact) -> Self {
        Self {
            outcome: StageOutcome::Completed,
            artefacts: Some(Box::new(artefacts)),
            summary: None,
        }
    }

    /// Creates a completed output with artefacts and a summary.
    pub fn completed_with_summary(
        artefacts: impl Artefact,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            outcome: StageOutcome::Completed,
            artefacts: Some(Box::new(artefacts)),
            summary: Some(summary.into()),
        }
    }

    /// Adds a summary to this output.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
}

/// Manual `Debug` implementation that delegates to the artefact's
/// `debug_description()` rather than requiring `Debug` on `dyn Artefact`.
impl std::fmt::Debug for StageOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let artefact_desc = self.artefacts.as_ref().map(|a| a.debug_description());
        f.debug_struct("StageOutput")
            .field("outcome", &self.outcome)
            .field("artefacts", &artefact_desc)
            .field("summary", &self.summary)
            .finish()
    }
}

/// Backward compatibility: v1 stages returning `StageOutcome` can be
/// automatically converted to `StageOutput` with no artefacts.
impl From<StageOutcome> for StageOutput {
    fn from(outcome: StageOutcome) -> Self {
        Self {
            outcome,
            artefacts: None,
            summary: None,
        }
    }
}
```

#### 6.2.2 Tests for StageOutput

Add to `src/stage.rs` tests:

```rust
mod stage_output_tests {
    use super::*;

    #[test]
    fn test_stage_output_from_outcome() {
        let output = StageOutput::from(StageOutcome::Completed);
        assert!(matches!(output.outcome, StageOutcome::Completed));
        assert!(output.artefacts.is_none());
        assert!(output.summary.is_none());
    }

    #[test]
    fn test_stage_output_from_skipped() {
        let output = StageOutput::from(StageOutcome::Skipped);
        assert!(matches!(output.outcome, StageOutcome::Skipped));
    }

    #[test]
    fn test_stage_output_completed_with_artefacts() {
        let output = StageOutput::completed_with(serde_json::json!({"key": "value"}));
        assert!(matches!(output.outcome, StageOutcome::Completed));
        assert!(output.artefacts.is_some());

        let json = output.artefacts.as_ref().unwrap().to_json().unwrap();
        assert_eq!(json["key"], "value");
    }

    #[test]
    fn test_stage_output_with_summary() {
        let output = StageOutput::from_outcome(StageOutcome::Completed)
            .with_summary("Processed 42 records");
        assert_eq!(output.summary, Some("Processed 42 records".to_string()));
    }

    #[test]
    fn test_stage_output_completed_with_summary() {
        let output = StageOutput::completed_with_summary(
            serde_json::json!({"count": 42}),
            "Extracted 42 entities",
        );
        assert!(output.artefacts.is_some());
        assert_eq!(output.summary, Some("Extracted 42 entities".to_string()));
    }

    #[test]
    fn test_stage_output_debug() {
        let output = StageOutput::completed_with(serde_json::json!(42));
        let debug = format!("{:?}", output);
        assert!(debug.contains("StageOutput"));
        assert!(debug.contains("Completed"));
        assert!(debug.contains("serde_json::Value"));
    }

    #[test]
    fn test_stage_output_debug_no_artefacts() {
        let output = StageOutput::from(StageOutcome::Completed);
        let debug = format!("{:?}", output);
        assert!(debug.contains("None"));
    }
}
```

### Verification Commands

```bash
cargo build
cargo test stage::stage_output_tests
cargo clippy
```

---

## Milestone 6.3 — Revise Stage Trait Return Type

### Files to Modify
- `src/stage.rs` (change `Stage::execute` return type)
- All existing stage implementations and tests

### Implementation Details

#### 6.3.1 Change `Stage::execute` Return Type

Change the trait definition:

```rust
#[async_trait]
pub trait Stage<W: WorkItem>: Send + Sync {
    /// Returns the name of this stage.
    fn name(&self) -> &str;

    /// Executes this stage on the given work item.
    ///
    /// # Returns
    ///
    /// A `StageOutput` containing the mechanical outcome, optional artefacts,
    /// and an optional summary. V1 stages can return `StageOutcome` values
    /// directly thanks to `From<StageOutcome> for StageOutput`:
    ///
    /// ```rust,ignore
    /// Ok(StageOutcome::Completed.into())
    /// ```
    async fn execute(&self, item: &W, ctx: &StageContext) -> Result<StageOutput>;
}
```

#### 6.3.2 Update Existing Stage Implementations

All existing test stages and implementations that return `Result<StageOutcome>` must be updated to return `Result<StageOutput>`. The `.into()` conversion makes this minimal:

**Before:**
```rust
async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
    Ok(StageOutcome::Completed)
}
```

**After:**
```rust
async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutput> {
    Ok(StageOutcome::Completed.into())
}
```

#### 6.3.3 Update Executor (`advance()`)

The executor in `src/workflow.rs` (from Phase 4) currently matches on `StageOutcome`. It must now extract the outcome from `StageOutput`:

**Before:**
```rust
let outcome = stage.execute(item, &ctx).await?;
match outcome {
    StageOutcome::Completed => { ... }
    StageOutcome::Skipped => { ... }
    // ...
}
```

**After:**
```rust
let output = stage.execute(item, &ctx).await?;
// Store artefacts if present
if let Some(ref artefact) = output.artefacts {
    // Store artefact JSON summary in state store (see Milestone 6.5)
}
match output.outcome {
    StageOutcome::Completed => { ... }
    StageOutcome::Skipped => { ... }
    // ...
}
```

### Verification Commands

```bash
cargo build
cargo test
cargo clippy
```

**Critical:** All existing tests must pass after this change. The `From<StageOutcome>` impl ensures backward compatibility.

---

## Milestone 6.4 — Extend StageContext with Feedback and Upstream Artefacts

### Files to Modify
- `src/stage.rs` (extend `StageContext`)

### Implementation Details

#### 6.4.1 Extend `StageContext`

Add v2 fields to `StageContext`. The `QualityFeedback` type is not yet defined (Phase 7), so use `Option<serde_json::Value>` as a placeholder for now, to be replaced in Phase 7:

```rust
pub struct StageContext {
    /// The stringified ID of the work item being processed.
    pub item_id: String,
    /// The name of the stage being executed.
    pub stage_name: String,
    /// Which execution attempt this is (1-indexed).
    pub attempt: u32,
    /// If this is a fan-out re-invocation, the name of the subtask.
    pub subtask_name: Option<String>,

    // --- New in v2 ---

    /// Feedback from the quality gate on the previous attempt.
    /// `None` on the first attempt or if no quality gate is attached.
    ///
    /// Note: This field uses `serde_json::Value` as a placeholder until
    /// the `QualityFeedback` type is defined in Phase 7.
    pub feedback: Option<serde_json::Value>,

    /// Artefacts from upstream stages, keyed by stage name.
    /// Populated by the engine based on the DAG structure.
    pub upstream_artefacts: HashMap<String, Box<dyn Artefact>>,
}
```

#### 6.4.2 Update StageContext Constructors

The existing `StageContext::new()` and builder methods must initialise the new fields:

```rust
impl StageContext {
    pub fn new(item_id: impl Into<String>, stage_name: impl Into<String>) -> Self {
        Self {
            item_id: item_id.into(),
            stage_name: stage_name.into(),
            attempt: 1,
            subtask_name: None,
            feedback: None,
            upstream_artefacts: HashMap::new(),
        }
    }

    // Existing builders remain...

    /// Sets the feedback from the previous attempt.
    pub fn with_feedback(mut self, feedback: serde_json::Value) -> Self {
        self.feedback = Some(feedback);
        self
    }

    /// Adds an upstream artefact.
    pub fn with_upstream_artefact(
        mut self,
        stage_name: impl Into<String>,
        artefact: Box<dyn Artefact>,
    ) -> Self {
        self.upstream_artefacts.insert(stage_name.into(), artefact);
        self
    }
}
```

#### 6.4.3 Update `Debug` for `StageContext`

Since `HashMap<String, Box<dyn Artefact>>` doesn't implement `Debug`, `StageContext` needs a manual `Debug` impl:

```rust
impl std::fmt::Debug for StageContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let artefact_keys: Vec<&String> = self.upstream_artefacts.keys().collect();
        f.debug_struct("StageContext")
            .field("item_id", &self.item_id)
            .field("stage_name", &self.stage_name)
            .field("attempt", &self.attempt)
            .field("subtask_name", &self.subtask_name)
            .field("feedback", &self.feedback)
            .field("upstream_artefact_keys", &artefact_keys)
            .finish()
    }
}
```

#### 6.4.4 Tests

```rust
#[test]
fn test_stage_context_v2_fields_default() {
    let ctx = StageContext::new("item-1", "scan");
    assert!(ctx.feedback.is_none());
    assert!(ctx.upstream_artefacts.is_empty());
}

#[test]
fn test_stage_context_with_feedback() {
    let feedback = serde_json::json!({"issue": "tables lost"});
    let ctx = StageContext::new("item-1", "scan")
        .with_feedback(feedback.clone());
    assert_eq!(ctx.feedback, Some(feedback));
}

#[test]
fn test_stage_context_with_upstream_artefacts() {
    let artefact: Box<dyn Artefact> = Box::new(serde_json::json!({"md": "hello"}));
    let ctx = StageContext::new("item-1", "extract")
        .with_upstream_artefact("pdf_to_markdown", artefact);
    assert!(ctx.upstream_artefacts.contains_key("pdf_to_markdown"));
}

#[test]
fn test_stage_context_debug_with_artefacts() {
    let artefact: Box<dyn Artefact> = Box::new(serde_json::json!(42));
    let ctx = StageContext::new("item-1", "stage")
        .with_upstream_artefact("upstream", artefact);
    let debug = format!("{:?}", ctx);
    assert!(debug.contains("upstream"));
}
```

### Verification Commands

```bash
cargo build
cargo test stage
cargo clippy
```

---

## Milestone 6.5 — Artefact Storage in StateStore

### Files to Modify
- `src/state_store/mod.rs` (extend `StateStore` trait with artefact storage)
- `src/state_store/memory.rs` (implement artefact storage)
- `src/state_store/sqlite.rs` (implement artefact storage)

### Implementation Details

#### 6.5.1 Extend `StateStore` Trait

Add methods for storing and retrieving artefact JSON summaries:

```rust
/// Stores the JSON summary of a stage's artefacts.
///
/// This stores the *serialisable summary* from `Artefact::to_json()`,
/// not the artefact itself. If the artefact is opaque, `artefact_json`
/// will be `None`.
async fn store_artefact_summary(
    &self,
    item_id: &W::Id,
    stage: &str,
    artefact_json: Option<serde_json::Value>,
) -> Result<()>;

/// Retrieves the stored artefact JSON summary for a stage.
async fn get_artefact_summary(
    &self,
    item_id: &W::Id,
    stage: &str,
) -> Result<Option<serde_json::Value>>;
```

#### 6.5.2 Implement in MemoryStateStore

Add to the `Storage` struct:

```rust
struct Storage {
    stages: HashMap<StageKey, StageStatus>,
    subtasks: HashMap<SubTaskKey, SubTaskStatus>,
    /// Artefact JSON summaries, keyed by (item_id, stage).
    artefact_summaries: HashMap<StageKey, Option<serde_json::Value>>,
}
```

Implement the new trait methods:

```rust
async fn store_artefact_summary(
    &self,
    item_id: &W::Id,
    stage: &str,
    artefact_json: Option<serde_json::Value>,
) -> Result<()> {
    let mut storage = self.storage.write().await;
    let key = StageKey::new(item_id, stage);
    storage.artefact_summaries.insert(key, artefact_json);
    Ok(())
}

async fn get_artefact_summary(
    &self,
    item_id: &W::Id,
    stage: &str,
) -> Result<Option<serde_json::Value>> {
    let storage = self.storage.read().await;
    let key = StageKey::new(item_id, stage);
    Ok(storage.artefact_summaries.get(&key).cloned().flatten())
}
```

#### 6.5.3 Implement in SqliteStateStore

Add a new column `artefact_summary TEXT` to the `stage_statuses` table (schema version 2 migration):

```sql
ALTER TABLE stage_statuses ADD COLUMN artefact_summary TEXT;
```

Implement the trait methods using `spawn_blocking` pattern.

#### 6.5.4 Update Executor to Store Artefacts

In the executor's `advance()` method, after a stage completes successfully, store the artefact summary:

```rust
let output = stage.execute(item, &ctx).await?;

// Store artefact summary
let artefact_json = output.artefacts.as_ref().and_then(|a| a.to_json());
store.store_artefact_summary(item.id(), stage_name, artefact_json).await?;
```

#### 6.5.5 Populate Upstream Artefacts in StageContext

When building the `StageContext` for a stage, populate `upstream_artefacts` from completed upstream stages. Since the `StateStore` only stores JSON summaries (not the full `Box<dyn Artefact>`), upstream artefacts from the store are `serde_json::Value`:

```rust
let mut upstream = HashMap::new();
for dep_name in workflow.dependencies(stage_name)? {
    if let Some(json) = store.get_artefact_summary(item.id(), dep_name).await? {
        upstream.insert(
            dep_name.to_string(),
            Box::new(json) as Box<dyn Artefact>,
        );
    }
}
ctx.upstream_artefacts = upstream;
```

#### 6.5.6 Tests

```rust
#[tokio::test]
async fn test_store_and_retrieve_artefact_summary() {
    let store = TestStore::new();
    let item_id = "item-1".to_string();

    let json = serde_json::json!({"table_count": 3, "word_count": 500});
    store.store_artefact_summary(&item_id, "pdf_to_md", Some(json.clone())).await.unwrap();

    let retrieved = store.get_artefact_summary(&item_id, "pdf_to_md").await.unwrap();
    assert_eq!(retrieved, Some(json));
}

#[tokio::test]
async fn test_store_none_artefact_summary() {
    let store = TestStore::new();
    let item_id = "item-1".to_string();

    store.store_artefact_summary(&item_id, "opaque_stage", None).await.unwrap();

    let retrieved = store.get_artefact_summary(&item_id, "opaque_stage").await.unwrap();
    assert!(retrieved.is_none());
}

#[tokio::test]
async fn test_missing_artefact_summary_returns_none() {
    let store = TestStore::new();
    let item_id = "item-1".to_string();

    let retrieved = store.get_artefact_summary(&item_id, "nonexistent").await.unwrap();
    assert!(retrieved.is_none());
}
```

### Verification Commands

```bash
cargo build --all-features
cargo test --all-features
cargo clippy --all-features
```

---

## Phase 6 Completion Checklist

- [ ] `cargo build` succeeds
- [ ] `cargo test` passes all tests (including all existing v1 tests)
- [ ] `cargo clippy -- -D warnings` clean
- [ ] `cargo doc --no-deps` builds without warnings
- [ ] `Artefact` trait defined with `to_json()` and `as_any()`
- [ ] `SerializableArtefact<T>` wrapper implemented
- [ ] Blanket `Artefact` impl for `serde_json::Value`
- [ ] `StageOutput` struct with manual `Debug`
- [ ] `From<StageOutcome> for StageOutput` preserves backward compatibility
- [ ] `Stage::execute` returns `Result<StageOutput>`
- [ ] `StageContext` extended with `feedback` and `upstream_artefacts`
- [ ] `StateStore` extended with artefact summary storage
- [ ] Both `MemoryStateStore` and `SqliteStateStore` updated
- [ ] Executor stores artefact summaries and populates upstream artefacts
- [ ] All existing tests pass unchanged (via `From` conversion)

### Public API After Phase 6

```rust
// New in Phase 6
treadle::Artefact              // Trait
treadle::SerializableArtefact  // Wrapper for Serialize types
treadle::StageOutput           // Stage return type (replaces StageOutcome)

// Modified in Phase 6
treadle::Stage                 // execute() now returns Result<StageOutput>
treadle::StageContext           // +feedback, +upstream_artefacts
treadle::StateStore            // +store_artefact_summary, +get_artefact_summary
```

---

## Notes on Rust Guidelines Applied

### Anti-Patterns Avoided
- AP-01: No bool arguments — artefact decisions use types
- AP-03: `Send + Sync + 'static` bounds on `Artefact` trait for async safety
- AP-09: No `unwrap()` in library code (`to_json()` returns `Option`, not `Result`)

### Core Idioms Applied
- ID-01: `#[non_exhaustive]` not needed (trait, not enum)
- ID-04: `impl Into<String>` for constructor parameters
- ID-09: `new()` as canonical constructor for `SerializableArtefact`
- ID-35: Builder methods returning `Self` on `StageOutput`

### Type Design
- TD-01: `Artefact` trait avoids boolean flags for serialisation decisions
- Custom `Debug` impl for types containing `dyn Artefact` (v2.1 §2.7)
