---
number: 1
title: "Treadle — Implementation Plan"
author: "stage and"
component: All
tags: [change-me]
created: 2026-02-07
updated: 2026-02-07
state: Active
supersedes: null
superseded-by: null
version: 1.0
---

# Treadle — Implementation Plan

**Project:** treadle — A persistent, resumable, human-in-the-loop workflow engine backed by a petgraph DAG  
**Repo:** https://github.com/oxur/treadle  
**Language:** Rust (edition 2021, MSRV 1.75)  
**License:** MIT OR Apache-2.0

---

## How to Use This Plan

This document is a phased implementation plan for the `treadle` crate. Each phase contains multiple milestones. Each milestone is scoped to be completable in a single Claude Code session (roughly 1–3 files, one coherent feature, with tests).

**Rules for implementers:**

1. Complete milestones in order within each phase. Phases must be completed sequentially.
2. Every milestone must compile with `cargo build` and pass `cargo test` before moving on.
3. Every public type, trait, and method must have rustdoc comments.
4. Use `#[cfg(feature = "sqlite")]` gating for all SQLite-specific code.
5. Follow the API surface described in the README (included below for reference).
6. Run `cargo clippy` and resolve all warnings at each milestone.

---

## Dependency Reference

```toml
[dependencies]
petgraph = "0.8"
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
rusqlite = { version = "0.38", features = ["bundled"], optional = true }

[features]
default = ["sqlite"]
sqlite = ["dep:rusqlite"]
```

---

## Phase 1: Core Types and Traits

**Goal:** Define the foundational type system. No execution logic yet — just the vocabulary of the crate.

### Milestone 1.1 — Error Types and Result Alias

**Files:** `src/lib.rs`, `src/error.rs`

- Create `src/lib.rs` with top-level module declarations and crate-level doc comment.
- Define `TreadleError` enum using `thiserror`:
  - `DagCycle` — cycle detected during workflow construction
  - `StageNotFound(String)` — referenced stage doesn't exist
  - `DuplicateStage(String)` — stage name already registered
  - `StateStoreError(String)` — persistence layer failure
  - `ExecutionError { stage: String, source: Box<dyn std::error::Error + Send + Sync> }` — stage execution failed
  - `InvalidTransition { from: StageState, to: StageState }` — illegal state transition
- Define `pub type Result<T> = std::result::Result<T, TreadleError>;`
- Add unit tests for error Display output.

### Milestone 1.2 — WorkItem Trait

**Files:** `src/work_item.rs`

- Define the `WorkItem` trait:
  ```rust
  pub trait WorkItem: Send + Sync + 'static {
      type Id: Clone + Eq + std::hash::Hash + std::fmt::Display + Send + Sync + 'static;
      fn id(&self) -> &Self::Id;
  }
  ```
- Re-export from `src/lib.rs`.
- Add a test helper `TestItem` struct implementing `WorkItem` in a `#[cfg(test)]` module (this will be reused across tests).

### Milestone 1.3 — Stage State, Status, and Outcome Types

**Files:** `src/stage.rs` (types portion)

- Define `StageState` enum (derives: Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize):
  - `Pending` — not yet started
  - `Running` — currently executing
  - `Completed` — finished successfully
  - `Failed` — execution failed
  - `AwaitingReview` — paused for human review
  - `Skipped` — stage determined nothing to do
- Define `ReviewData` struct:
  - `summary: String`
  - `details: Option<serde_json::Value>` — arbitrary structured data for review UI
  - `created_at: chrono::DateTime<chrono::Utc>`
  - Constructor `ReviewData::new(summary: impl Into<String>) -> Self`
- Define `SubTask` struct:
  - `name: String`
  - `metadata: Option<serde_json::Value>`
- Define `StageOutcome` enum:
  - `Completed`
  - `AwaitingReview(ReviewData)`
  - `FanOut(Vec<SubTask>)`
  - `Skipped`
- Define `StageStatus` struct:
  - `state: StageState`
  - `updated_at: chrono::DateTime<chrono::Utc>`
  - `error: Option<String>`
  - `review_data: Option<ReviewData>`
  - `subtasks: Vec<SubTaskStatus>`
  - Constructors: `StageStatus::pending()`, `StageStatus::completed()`, `StageStatus::failed(error)`, `StageStatus::awaiting_review(data)`, `StageStatus::skipped()`
- Define `SubTaskStatus` struct:
  - `name: String`
  - `state: StageState`
  - `error: Option<String>`
  - `updated_at: chrono::DateTime<chrono::Utc>`
- Add serde round-trip tests for all types.

### Milestone 1.4 — Stage Trait and StageContext

**Files:** `src/stage.rs` (trait portion)

- Define `StageContext` struct:
  - `item_id: String` — stringified item ID for logging/tracing
  - `stage_name: String`
  - `attempt: u32` — which attempt this is (for retry-aware stages)
  - Constructor: `StageContext::new(item_id: impl Into<String>, stage_name: impl Into<String>) -> Self`
- Define the `Stage` trait:
  ```rust
  #[async_trait]
  pub trait Stage<W: WorkItem>: Send + Sync {
      fn name(&self) -> &str;
      async fn execute(&self, item: &W, ctx: &StageContext) -> Result<StageOutcome>;
  }
  ```
- Re-export all public types from `src/lib.rs`.
- Write a test with a trivial stage implementation that returns each `StageOutcome` variant.

### Milestone 1.5 — StateStore Trait

**Files:** `src/state_store.rs`

- Define the `StateStore` trait:
  ```rust
  #[async_trait]
  pub trait StateStore<W: WorkItem>: Send + Sync {
      async fn get_status(&self, item_id: &W::Id, stage: &str) -> Result<Option<StageStatus>>;
      async fn set_status(&self, item_id: &W::Id, stage: &str, status: StageStatus) -> Result<()>;
      async fn get_all_statuses(&self, item_id: &W::Id) -> Result<std::collections::HashMap<String, StageStatus>>;
      async fn query_items(&self, stage: &str, state: StageState) -> Result<Vec<String>>;
      async fn get_subtask_statuses(&self, item_id: &W::Id, stage: &str) -> Result<Vec<SubTaskStatus>>;
      async fn set_subtask_status(&self, item_id: &W::Id, stage: &str, subtask: &str, status: SubTaskStatus) -> Result<()>;
  }
  ```
- Re-export from `src/lib.rs`.
- No implementation yet — just the trait definition.

---

## Phase 2: State Store Implementations

**Goal:** Two working StateStore backends — in-memory (for testing) and SQLite (for production).

### Milestone 2.1 — In-Memory StateStore

**Files:** `src/state_store/memory.rs`, update `src/state_store.rs` to be a module

- Refactor `src/state_store.rs` into `src/state_store/mod.rs` (trait) + `src/state_store/memory.rs`.
- Implement `MemoryStateStore<W: WorkItem>`:
  - Internal storage: `Arc<Mutex<HashMap<...>>>` or similar.
  - `MemoryStateStore::new() -> Self`
  - Implement all `StateStore` methods.
- Write comprehensive tests:
  - Set and get status round-trip.
  - `get_all_statuses` returns all stages for an item.
  - `query_items` filters by stage and state.
  - Subtask status set/get.
  - Missing item returns `None`.
  - Concurrent access (spawn multiple tokio tasks).

### Milestone 2.2 — SQLite StateStore: Schema and Connection

**Files:** `src/state_store/sqlite.rs`

- Gate entire file with `#[cfg(feature = "sqlite")]`.
- Define `SqliteStateStore` struct (not generic — stores stringified IDs):
  - Wraps `rusqlite::Connection` in appropriate sync/async wrapper (rusqlite is sync; use `tokio::task::spawn_blocking`).
  - `SqliteStateStore::open(path: impl AsRef<std::path::Path>) -> Result<Self>` — opens DB, runs migrations.
  - `SqliteStateStore::open_in_memory() -> Result<Self>` — for testing.
- Define schema (two tables):
  - `stage_statuses`: `item_id TEXT, stage TEXT, state TEXT, updated_at TEXT, error TEXT, review_data TEXT, PRIMARY KEY (item_id, stage)`
  - `subtask_statuses`: `item_id TEXT, stage TEXT, subtask_name TEXT, state TEXT, error TEXT, updated_at TEXT, PRIMARY KEY (item_id, stage, subtask_name)`
- Run schema creation on open.
- Test: open in-memory, verify tables exist.

### Milestone 2.3 — SQLite StateStore: Full Implementation

**Files:** `src/state_store/sqlite.rs` (continued)

- Implement all `StateStore<W>` trait methods for `SqliteStateStore`.
  - Serialize `StageStatus` fields to/from SQL columns.
  - `review_data` stored as JSON text.
  - Use `spawn_blocking` for all rusqlite calls.
- Write the same comprehensive test suite as Milestone 2.1, but targeting `SqliteStateStore::open_in_memory()`.
- Add a test that opens a file-based DB, writes data, drops the store, reopens, and reads back — proving persistence.

### Milestone 2.4 — Re-exports and Module Organization

**Files:** `src/lib.rs`, `src/state_store/mod.rs`

- Ensure clean public API:
  - `treadle::WorkItem`
  - `treadle::Stage`, `treadle::StageOutcome`, `treadle::StageContext`
  - `treadle::StageState`, `treadle::StageStatus`, `treadle::SubTask`, `treadle::SubTaskStatus`, `treadle::ReviewData`
  - `treadle::StateStore`
  - `treadle::MemoryStateStore`
  - `treadle::SqliteStateStore` (behind `#[cfg(feature = "sqlite")]`)
  - `treadle::Result`, `treadle::TreadleError`
- Add a module-level doc comment on `src/lib.rs` with a brief usage example.
- Verify `cargo doc --open` produces clean documentation.

---

## Phase 3: Workflow Builder and DAG

**Goal:** A `Workflow` struct backed by petgraph that validates the DAG and provides topological ordering.

### Milestone 3.1 — Workflow Struct and Builder: Stage Registration

**Files:** `src/workflow.rs`

- Define `Workflow<W: WorkItem>`:
  - Internal petgraph `DiGraph` where nodes hold `Box<dyn Stage<W>>` (or an Arc'd wrapper) and node metadata (stage name).
  - A `HashMap<String, NodeIndex>` for name → node lookup.
  - `Workflow::builder() -> WorkflowBuilder<W>`
- Define `WorkflowBuilder<W: WorkItem>`:
  - `stage(name: impl Into<String>, stage: impl Stage<W> + 'static) -> Self` — adds a node.
  - Returns error on duplicate stage names (store and check at build time, or fail eagerly).
- Test: create builder, add stages, verify count.

### Milestone 3.2 — Workflow Builder: Dependencies and DAG Validation

**Files:** `src/workflow.rs` (continued)

- Add to `WorkflowBuilder`:
  - `dependency(stage: &str, depends_on: &str) -> Self` — adds a directed edge (depends_on → stage).
  - `build(self) -> Result<Workflow<W>>`:
    - Validates all referenced stages exist → `StageNotFound` error.
    - Runs `petgraph::algo::is_cyclic_directed` → `DagCycle` error.
    - Computes and caches topological order via `petgraph::algo::toposort`.
    - Returns the built `Workflow`.
- Expose `Workflow::stages() -> &[String]` — returns stage names in topological order.
- Expose `Workflow::dependencies(stage: &str) -> Result<Vec<&str>>` — returns immediate dependencies of a stage.
- Tests:
  - Linear pipeline: A → B → C. Verify topo order.
  - Diamond: A → B, A → C, B → D, C → D. Verify valid topo order.
  - Cycle detection: A → B → A returns `DagCycle`.
  - Missing stage reference returns `StageNotFound`.
  - Duplicate stage name returns `DuplicateStage`.

### Milestone 3.3 — Workflow: Ready-Stage Resolution

**Files:** `src/workflow.rs` (continued)

- Add method:
  ```rust
  impl<W: WorkItem> Workflow<W> {
      pub async fn ready_stages(&self, item_id: &W::Id, store: &dyn StateStore<W>) -> Result<Vec<String>>
  }
  ```
  - Returns stage names whose dependencies are ALL `Completed` or `Skipped`, and whose own status is `Pending` or not yet present in the store.
  - This is the core scheduling logic — "what can run next for this item?"
- Tests using `MemoryStateStore`:
  - Fresh item: only root stages (no dependencies) are ready.
  - After completing root: next stage becomes ready.
  - Diamond convergence: stage with two deps only ready when both complete.
  - Stage already completed/running/awaiting_review: not in ready list.
  - Skipped dependency counts as satisfied.

---

## Phase 4: Workflow Executor

**Goal:** The `advance` method that drives items through the pipeline, plus the event stream.

### Milestone 4.1 — Event Types and Broadcast Channel

**Files:** `src/event.rs`

- Define `WorkflowEvent` enum (derives: Clone, Debug):
  ```rust
  pub enum WorkflowEvent {
      StageStarted { item_id: String, stage: String },
      StageCompleted { item_id: String, stage: String },
      StageFailed { item_id: String, stage: String, error: String },
      ReviewRequired { item_id: String, stage: String, data: ReviewData },
      StageSkipped { item_id: String, stage: String },
      FanOutStarted { item_id: String, stage: String, subtasks: Vec<String> },
      SubTaskStarted { item_id: String, stage: String, subtask: String },
      SubTaskCompleted { item_id: String, stage: String, subtask: String },
      SubTaskFailed { item_id: String, stage: String, subtask: String, error: String },
  }
  ```
- Add to `Workflow`:
  - Internal `tokio::sync::broadcast::Sender<WorkflowEvent>` (created at build time).
  - `pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<WorkflowEvent>`
- Note: `WorkflowEvent` uses `String` for `item_id` (not `W::Id`) to keep the event type non-generic and easy to serialize/log.
- Test: subscribe, send manually, receive.

### Milestone 4.2 — Core Advance Logic (Single Stage Execution)

**Files:** `src/executor.rs`

- Implement on `Workflow<W>`:
  ```rust
  pub async fn advance(&self, item: &W, store: &dyn StateStore<W>) -> Result<()>
  ```
  - Calls `ready_stages` to find executable stages.
  - For each ready stage (executed concurrently via `tokio::JoinSet` or sequentially — start sequential, optimize later):
    1. Set status to `Running`, emit `StageStarted`.
    2. Build `StageContext`, call `stage.execute(item, &ctx)`.
    3. Match on `StageOutcome`:
       - `Completed` → set `Completed`, emit `StageCompleted`.
       - `Skipped` → set `Skipped`, emit `StageSkipped`.
       - `AwaitingReview(data)` → set `AwaitingReview` with review data, emit `ReviewRequired`. **Do not advance dependents.**
       - `FanOut(subtasks)` → handle in Milestone 4.3.
    4. On execute error → set `Failed` with error message, emit `StageFailed`.
  - After executing all ready stages, call `advance` again recursively to pick up newly-ready stages — **but** only if at least one stage completed or was skipped this round (prevents infinite loop).
- Tests:
  - Linear pipeline with all-completing stages: full traversal in one `advance` call.
  - Pipeline with review gate: advances to review, stops, second `advance` after approval continues.
  - Stage failure: marks failed, doesn't advance dependents.
  - Concurrent-ready stages (diamond DAG): both middle stages execute.

### Milestone 4.3 — Fan-Out Execution

**Files:** `src/executor.rs` (continued)

- When a stage returns `FanOut(subtasks)`:
  1. Emit `FanOutStarted` with subtask names.
  2. For each `SubTask`, store initial `SubTaskStatus` (state: `Pending`).
  3. Mark the parent stage as `Running` (it stays running until all subtasks resolve).
  4. Note: The subtasks in `FanOut` are **declarative** — they declare what parallel work exists. The actual execution of subtasks is delegated to a `SubTaskExecutor` trait (defined here):
     ```rust
     #[async_trait]
     pub trait SubTaskExecutor<W: WorkItem>: Send + Sync {
         async fn execute_subtask(&self, item: &W, stage: &str, subtask: &SubTask, ctx: &StageContext) -> Result<()>;
     }
     ```
  5. Alternatively (simpler v1): subtasks are tracked but executed by re-invoking the parent stage with context indicating which subtask to run. **Decision: go with the simpler approach first** — the `StageContext` gets an `Option<String>` field `subtask_name`. If a stage returned `FanOut`, the executor re-invokes the stage once per subtask with `subtask_name` set. Each invocation should return `Completed` or fail.
  6. When all subtasks complete → mark parent stage `Completed`, emit `StageCompleted`.
  7. If any subtask fails → mark parent stage `Failed`, emit `StageFailed`.
- Tests:
  - Fan-out with 3 subtasks, all complete → stage completes.
  - Fan-out with 1 failure → stage fails.
  - Fan-out subtask statuses persisted and queryable.

### Milestone 4.4 — Advance Idempotency and Edge Cases

**Files:** `src/executor.rs` (continued)

- Ensure `advance` is idempotent:
  - Calling `advance` when no stages are ready is a no-op (returns `Ok(())`).
  - Calling `advance` on a fully-completed pipeline is a no-op.
  - A stage in `Running` state (from a previous crashed run) should be treated as retriable — reset to `Pending` and re-execute. Add a config option for this behavior.
- Add `Workflow::is_complete(&self, item_id: &W::Id, store: &dyn StateStore<W>) -> Result<bool>`:
  - Returns true if all stages are `Completed` or `Skipped`.
- Add `Workflow::is_blocked(&self, item_id: &W::Id, store: &dyn StateStore<W>) -> Result<bool>`:
  - Returns true if no stages are ready and pipeline is not complete (i.e., something is awaiting review or failed).
- Tests for all edge cases.

---

## Phase 5: Status, Visualization, and Polish

**Goal:** Pipeline introspection, documentation, examples, and release readiness.

### Milestone 5.1 — Pipeline Status Helpers

**Files:** `src/status.rs`

- Define `PipelineStatus` struct:
  - `item_id: String`
  - `stages: Vec<StageStatusEntry>` (in topological order)
- Define `StageStatusEntry`:
  - `name: String`
  - `state: StageState`
  - `updated_at: Option<chrono::DateTime<chrono::Utc>>`
  - `error: Option<String>`
  - `review_data: Option<ReviewData>`
  - `subtasks: Vec<SubTaskStatus>`
- Add to `Workflow`:
  ```rust
  pub async fn status(&self, item_id: &W::Id, store: &dyn StateStore<W>) -> Result<PipelineStatus>
  ```
- Implement `Display` for `PipelineStatus` producing a human-readable summary, e.g.:
  ```
  Pipeline status for item "doc-1":
    ✅ scan        Completed   2024-01-15 10:30:00
    ✅ identify    Completed   2024-01-15 10:30:05
    🔄 enrich     Running     2024-01-15 10:30:10
       └─ src1    ✅ Completed
       └─ src2    🔄 Running
       └─ src3    ❌ Failed: timeout
    ⏳ review     Pending
    ⏳ export     Pending
  ```
- Tests: verify Display output for various pipeline states.

### Milestone 5.2 — Tracing Integration

**Files:** across all executor code

- Add `tracing` spans and events throughout the executor:
  - `info_span!("advance", item_id = %item.id())` wrapping the advance call.
  - `info_span!("stage", stage = %name)` for each stage execution.
  - `tracing::info!("stage_completed")`, `tracing::warn!("stage_failed")`, etc.
  - `tracing::debug!` for ready-stage resolution and state transitions.
- No new tests required — this is observability plumbing.

### Milestone 5.3 — Integration Test: Full Pipeline

**Files:** `tests/integration.rs`

- A single integration test file exercising the full API end-to-end:
  1. Define a `Document` work item.
  2. Implement 4 stages: Parse (completes), Enrich (fans out to 3 subtasks), Review (returns AwaitingReview), Export (completes).
  3. Build workflow with builder pattern.
  4. Subscribe to events, collect in a `Vec`.
  5. Create a `SqliteStateStore::open_in_memory()`.
  6. Advance item — verify it stops at review.
  7. Query status — verify pipeline state.
  8. Approve review by setting status to `Completed`.
  9. Advance again — verify export runs.
  10. Verify `is_complete` returns true.
  11. Verify collected events match expected sequence.
- Also test with `MemoryStateStore` to verify backend-agnostic behavior.

### Milestone 5.4 — Documentation and Examples

**Files:** `src/lib.rs` (crate docs), `examples/basic_pipeline.rs`

- Expand crate-level documentation in `src/lib.rs`:
  - Overview section matching the README's "What Is Treadle?" framing.
  - Quick start example (compilable via `cargo test --doc`).
  - Feature flags section documenting `sqlite`.
  - Links to key types.
- Create `examples/basic_pipeline.rs`:
  - A self-contained, runnable example matching the README's "Usage Example" section.
  - Uses `SqliteStateStore` with a temp file.
  - Demonstrates: building a workflow, advancing, handling review, checking status.
- Verify: `cargo test --doc`, `cargo run --example basic_pipeline`.

### Milestone 5.5 — Release Checklist

**Files:** various

- Verify `cargo clippy -- -D warnings` passes.
- Verify `cargo test --all-features` passes.
- Verify `cargo test --no-default-features` passes (no sqlite).
- Verify `cargo doc --no-deps` builds cleanly.
- Ensure `Cargo.toml` metadata is complete (description, license, repository, keywords, categories, readme).
- Add `CHANGELOG.md` with initial 0.1.0 entry.
- Add `LICENSE-MIT` and `LICENSE-APACHE` files.
- Verify the crate can be published: `cargo publish --dry-run`.

---

## Architecture Summary

```
src/
├── lib.rs                  # Crate root, re-exports, crate-level docs
├── error.rs                # TreadleError, Result type alias
├── work_item.rs            # WorkItem trait
├── stage.rs                # StageState, StageOutcome, StageStatus, ReviewData,
│                           #   SubTask, SubTaskStatus, StageContext, Stage trait
├── state_store/
│   ├── mod.rs              # StateStore trait
│   ├── memory.rs           # MemoryStateStore
│   └── sqlite.rs           # SqliteStateStore (#[cfg(feature = "sqlite")])
├── workflow.rs             # Workflow, WorkflowBuilder, DAG management
├── executor.rs             # advance(), fan-out execution, ready-stage resolution
├── event.rs                # WorkflowEvent, broadcast channel setup
└── status.rs               # PipelineStatus, Display impl

tests/
└── integration.rs          # Full end-to-end test

examples/
└── basic_pipeline.rs       # Runnable example
```

---

## Key Design Decisions

1. **`WorkflowEvent` uses `String` for item IDs** — avoids making the event type generic, simplifying event consumers (TUIs, loggers). The `Display` bound on `WorkItem::Id` makes this lossless.

2. **Fan-out re-invokes the parent stage** — rather than introducing a separate `SubTaskExecutor` trait, the parent stage is called again with `StageContext.subtask_name` set. This keeps the trait surface minimal for v1.

3. **`advance` is recursive but bounded** — it loops only while progress is being made (at least one stage completed/skipped per round). Review gates and failures naturally halt the loop.

4. **SQLite access via `spawn_blocking`** — rusqlite is synchronous. All DB operations are wrapped in `tokio::task::spawn_blocking` to avoid blocking the async runtime.

5. **`MemoryStateStore` is generic, `SqliteStateStore` is not** — SQLite stores everything as strings. The `MemoryStateStore` is generic over `W` for type safety in tests.

6. **No retry policy in the engine** — stages implement their own retry logic. The engine records outcomes. This follows the "bring your own resilience" design principle.
