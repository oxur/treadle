# Treadle

*A persistent, resumable, human-in-the-loop workflow engine backed by a petgraph DAG*

[![Crates.io](https://img.shields.io/crates/v/treadle.svg)](https://crates.io/crates/treadle)
[![Documentation](https://docs.rs/treadle/badge.svg)](https://docs.rs/treadle)
[![License](https://img.shields.io/crates/l/treadle.svg)](LICENSE-MIT)

> **Status: Early Development** — The API is being designed. This README
> describes the intended architecture. Contributions and design feedback are
> welcome.

---

## What Is Treadle?

Treadle is a lightweight workflow engine for Rust that tracks **work items**
as they progress through a **directed acyclic graph (DAG) of stages**, with
**persistent state**, **human review gates**, and **fan-out with per-subtask
visibility**.

It fills a specific gap in the Rust ecosystem: the space between single-shot
DAG executors (define stages, run once, get results) and heavyweight
distributed workflow engines (durable execution, external runtime servers,
replay journals). Treadle is designed for **local, single-process pipelines**
where you need the pipeline to survive restarts, pause for human decisions,
and show you exactly where every item stands.

The name comes from the **treadle** — the foot-operated lever that drives a
loom, spinning wheel, or lathe. The machine has stages and mechanisms, but
without the human pressing the treadle, nothing moves. This captures the core
design: a pipeline engine where human judgment gates the flow.

## Why Treadle?

If you're building a CLI tool or local service that processes items through
multiple stages — and you need persistence, resumability, and human review —
your current options in Rust are:

- **Single-shot DAG executors** (dagrs, dagx, async_dag): Great for
  "define tasks, run them in parallel, get results." But they have no
  persistent state, no pause/resume, no concept of work items progressing
  over time. If your process crashes, you start over.

- **Distributed workflow engines** (Restate, Temporal, Flawless): Powerful
  durable execution with journaled replay. But they require an external
  runtime server, are designed for distributed microservices, and are
  enormous overkill for a personal CLI tool or local pipeline.

- **DAG data structures** (daggy, petgraph): Excellent building blocks, but
  they're data structures, not execution engines. You still need to build
  the state tracking, execution logic, and review workflow yourself.

Treadle occupies the middle ground: a **library** (not a service) that gives
you persistent, resumable, inspectable DAG execution with human-in-the-loop
gates, without requiring any external infrastructure.

## Core Concepts

### Work Items

A work item is anything flowing through your pipeline. It could be a file to
process, a record to enrich, an image to transform — anything that needs to
pass through multiple stages. You define what a work item is by implementing
the `WorkItem` trait:

```rust
pub trait WorkItem: Send + Sync {
    type Id: Clone + Eq + Hash + Display + Send + Sync;
    fn id(&self) -> &Self::Id;
}
```

### Stages

A stage is a single step in the pipeline. You implement the `Stage` trait to
define what happens at each step:

```rust
#[async_trait]
pub trait Stage<W: WorkItem>: Send + Sync {
    fn name(&self) -> &str;
    async fn execute(&self, item: &W, ctx: &StageContext) -> Result<StageOutcome>;
}
```

Stages return a `StageOutcome` indicating what happened:

- **`Completed`** — Stage succeeded. Dependents can now run.
- **`AwaitingReview(ReviewData)`** — Stage produced results that need human
  approval before the pipeline continues.
- **`FanOut(Vec<SubTask>)`** — Stage spawned multiple concurrent subtasks
  (e.g., fetching from several APIs). Each subtask is tracked independently.
- **`Skipped`** — Stage determined it has nothing to do for this item.

### The DAG

Stages are connected in a directed acyclic graph using petgraph. This gives
you topological ordering (stages run in dependency order), cycle detection at
build time, and an inspectable graph structure for status display:

```rust
let workflow = Workflow::builder()
    .stage("scan", ScanStage::new())
    .stage("identify", IdentifyStage::new())
    .stage("enrich", EnrichStage::new(sources))
    .stage("review", ReviewStage::new(rules))
    .stage("export", ExportStage::new())
    .dependency("identify", "scan")
    .dependency("enrich", "identify")
    .dependency("review", "enrich")
    .dependency("export", "review")
    .build()?;
```

### Persistent State

Every work item's progress through the DAG is tracked in a durable state
store. The default implementation uses SQLite, but the `StateStore` trait can
be implemented for any backend:

```rust
#[async_trait]
pub trait StateStore<W: WorkItem>: Send + Sync {
    async fn get_status(&self, item_id: &W::Id, stage: &str) -> Result<Option<StageStatus>>;
    async fn set_status(&self, item_id: &W::Id, stage: &str, status: StageStatus) -> Result<()>;
    async fn query_items(&self, stage: &str, state: StageState) -> Result<Vec<W::Id>>;
}
```

This means:

- If the process crashes, you resume from where you left off.
- You can query "show me all items awaiting review" or "what failed at the
  enrich stage" at any time.
- `treadle status` (in your CLI) can show the full pipeline state.

### Human-in-the-Loop Review Gates

When a stage returns `StageOutcome::AwaitingReview`, the pipeline pauses for
that work item. The item sits in the review state until a human explicitly
approves, edits, or rejects it. This is first-class in the engine, not a
workaround.

### Fan-Out with Per-Subtask Tracking

A stage can fan out into multiple concurrent subtasks — for example, enriching
a record from five different APIs simultaneously. Each subtask is tracked
independently in the state store with its own status, retry count, and error
history. If three of five sources succeed and two fail, you retry only the
two that failed.

### Event Stream

The workflow engine emits structured events via a tokio broadcast channel.
Your TUI, CLI, or logging layer subscribes to these events for real-time
visibility:

```rust
pub enum WorkflowEvent<W: WorkItem> {
    StageStarted { item_id: W::Id, stage: String },
    StageCompleted { item_id: W::Id, stage: String },
    StageFailed { item_id: W::Id, stage: String, error: String },
    ReviewRequired { item_id: W::Id, stage: String, data: ReviewData },
    SubTaskStarted { item_id: W::Id, stage: String, subtask: String },
    SubTaskCompleted { item_id: W::Id, stage: String, subtask: String },
    SubTaskFailed { item_id: W::Id, stage: String, subtask: String, error: String },
}
```

## Design Principles

1. **Library, not a service.** Treadle is a crate you embed in your
   application. No external runtime, no server process, no Docker container.
   Add it to your `Cargo.toml` and go.

2. **The human is part of the pipeline.** Review gates are a first-class
   concept, not an afterthought. The engine is designed around the assumption
   that some stages need human judgment.

3. **Visibility over magic.** Every piece of state is inspectable. You can
   always answer "where is this item in the pipeline, what happened at each
   stage, and why did this fail?" The event stream makes real-time
   observation trivial.

4. **Bring your own resilience.** Treadle tracks state and executes stages,
   but it doesn't impose retry or circuit-breaker policies. Your stage
   implementations use whatever resilience strategy fits (backon, failsafe,
   custom logic). The engine records what happened.

5. **Stages are the unit of abstraction.** Implementing a new stage is
   implementing a trait. Adding a stage to the pipeline is adding a node and
   an edge. The engine handles ordering, state, and concurrency.

6. **Incremental by nature.** The pipeline processes items one at a time (or
   in batches), tracking each independently. New items can enter the pipeline
   at any time. Items at different stages coexist naturally.

## Intended Architecture

```
┌──────────────────────────────────────────────────┐
│            Your Application (CLI, TUI, HTTP)     │
│                    ^ subscribes to events        │
└────────────────────┼─────────────────────────────┘
                     │
┌────────────────────┼─────────────────────────────┐
│  Treadle Engine    │                             │
│                    │                             │
│  ┌─────────────────v──────────────────────────┐  │
│  │  Event Stream (tokio broadcast channel)    │  │
│  └────────────────────────────────────────────┘  │
│                                                  │
│  ┌────────────────────────────────────────────┐  │
│  │  Workflow (petgraph DAG of Stages)         │  │
│  │                                            │  │
│  │  scan ──> identify ──> enrich ──> review   │  │
│  │                          │                 │  │
│  │                   ┌──────┴───────┐         │  │
│  │                   │   fan-out    │         │  │
│  │                   │ src1 src2 …  │         │  │
│  │                   └──────────────┘         │  │
│  └────────────────────────────────────────────┘  │
│                                                  │
│  ┌────────────────────────────────────────────┐  │
│  │  StateStore (SQLite / custom)              │  │
│  │  item × stage × subtask → status           │  │
│  └────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────┘
```

## Usage Example (Planned API)

```rust
use treadle::{Workflow, Stage, StageOutcome, StageContext, StateStore};
use treadle::state::SqliteStateStore;

// Define a work item
#[derive(Clone)]
struct Document {
    id: String,
    path: PathBuf,
}

impl treadle::WorkItem for Document {
    type Id = String;
    fn id(&self) -> &String { &self.id }
}

// Implement stages
struct ParseStage;

#[async_trait]
impl Stage<Document> for ParseStage {
    fn name(&self) -> &str { "parse" }
    async fn execute(&self, item: &Document, ctx: &StageContext) -> Result<StageOutcome> {
        // ... parse the document ...
        Ok(StageOutcome::Completed)
    }
}

struct ReviewStage;

#[async_trait]
impl Stage<Document> for ReviewStage {
    fn name(&self) -> &str { "review" }
    async fn execute(&self, item: &Document, ctx: &StageContext) -> Result<StageOutcome> {
        let data = ReviewData::new("Parsed content ready for review");
        Ok(StageOutcome::AwaitingReview(data))
    }
}

// Build and run
#[tokio::main]
async fn main() -> Result<()> {
    let state = SqliteStateStore::open("pipeline.db").await?;

    let workflow = Workflow::builder()
        .stage("parse", ParseStage)
        .stage("review", ReviewStage)
        .stage("export", ExportStage)
        .dependency("review", "parse")
        .dependency("export", "review")
        .build()?;

    // Subscribe to events for your TUI/CLI
    let mut events = workflow.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            println!("{event:?}");
        }
    });

    // Process an item — advances through all eligible stages
    let doc = Document { id: "doc-1".into(), path: "report.pdf".into() };
    workflow.advance(&doc, &state).await?;

    // Later: approve the review and continue
    state.set_status(&doc.id, "review", StageStatus::completed()).await?;
    workflow.advance(&doc, &state).await?;

    Ok(())
}
```

## Target Use Cases

- **Media processing pipelines** — scan files, identify metadata, enrich
  from external sources, review, export. (This is the motivating use case:
  [tessitura](https://github.com/TODO/tessitura), a musicological library
  cataloging tool.)
- **Data migration / ETL tools** — extract records, transform, validate
  with human review, load.
- **Document processing** — parse, classify, review, archive.
- **Content moderation pipelines** — ingest, auto-classify, flag for human
  review, publish or reject.
- **Any CLI tool** where items flow through stages, some stages need human
  judgment, and you need the pipeline to survive restarts.

## Related Projects

The Rust ecosystem has several DAG and workflow libraries. Here's how they
compare to treadle and why we needed to build something new.

### Single-Shot DAG Executors

These libraries define a DAG of tasks, execute them (often in parallel), and
return results. They're excellent for computational pipelines but lack
persistence, resumability, and human-in-the-loop support.

| Project | Description | What's Similar | What's Missing |
|---------|-------------|----------------|----------------|
| [**dagrs**](https://crates.io/crates/dagrs) | High-performance async task framework following Flow-Based Programming. ~470 stars, the most mature Rust DAG executor. Supports conditional nodes, loop subgraphs, inter-task communication channels, and custom config parsers. | DAG-based task execution with dependency ordering and parallel scheduling. Async-first with tokio. Trait-based task definition. | No persistent state — if the process stops, all progress is lost. No concept of work items progressing over time. No pause/resume or human review gates. No fan-out with per-subtask tracking. Designed for "run once and done" computation, not ongoing pipelines. |
| [**dagx**](https://lib.rs/crates/dagx) | Minimal, type-safe async DAG executor with compile-time cycle prevention. Uses a `#[task]` macro and `DagRunner` for type-safe dependency wiring. | Compile-time type safety for dependencies. Clean, minimal API. True parallel execution with sub-microsecond overhead. | Same fundamental model as dagrs: single-shot execution with no persistence, no state tracking, no pause/resume, no human-in-the-loop. The type-safety focus is compelling but orthogonal to the problems treadle solves. |
| [**async_dag**](https://docs.rs/async_dag) | Maximizes parallel execution when async tasks form a DAG. Automatic scheduling ensures tasks run as soon as their dependencies complete. | Automatic dependency-aware parallel scheduling. | Purely a scheduling optimizer — no persistence, no state, no work items. Executes a fixed computation graph once. |

### DAG Data Structures

These provide the graph primitives but not execution, state, or workflow
semantics.

| Project | Description | What's Similar | What's Missing |
|---------|-------------|----------------|----------------|
| [**daggy**](https://docs.rs/daggy) | A petgraph wrapper exposing a DAG-specific API with `Walker` traversal. Supports serde serialization and stable node indices. | Built on petgraph (as treadle is). DAG-specific API with cycle prevention. | A data structure, not an execution engine. No state tracking, no async execution, no work items, no event stream. Treadle uses petgraph directly for its DAG backbone. |
| [**petgraph**](https://docs.rs/petgraph) | The foundational Rust graph library. Provides generic graph data structures, algorithms (topological sort, shortest path, etc.), and traversal. | Treadle *uses* petgraph internally for its DAG representation and topological ordering. | A graph library, not a workflow engine. petgraph is a dependency of treadle, not an alternative. |

### Heavyweight / Distributed Workflow Engines

These provide durable execution with strong guarantees, but require external
infrastructure and are designed for distributed systems at scale.

| Project | Description | What's Similar | What's Missing (or rather, what's too much) |
|---------|-------------|----------------|----------------------------------------------|
| [**Restate**](https://restate.dev) (restate-sdk) | Low-latency durable execution engine. Journaled execution with replay, distributed state, saga/compensation patterns. Supports Rust, TypeScript, Java, Go, Python. | Durable state that survives crashes. Workflow orchestration with typed handlers. Rust SDK available. | Requires running a separate Restate server process. Designed for distributed microservices, not local CLI tools. The journaled replay model solves distributed consistency problems that don't apply to single-process pipelines. Overkill for personal tools — like using Kubernetes to run a shell script. |
| [**Temporal**](https://temporal.io) | Microservice orchestration platform with durable execution. Industry-standard for distributed workflows. | Workflow-as-code with durable state. Strong guarantees around exactly-once execution. | Requires a Temporal server cluster. Java/Go/TypeScript native; Rust support is minimal. Designed for large-scale distributed systems. Even heavier than Restate for the local CLI use case. |
| [**Flawless**](https://flawless.dev) | Durable execution engine for Rust. Compiles workflows to WebAssembly, runs in a deterministic sandbox, journals all side effects for replay. | Rust-native. Durable execution with crash recovery. Beautiful technical design. | Requires an external engine process. Compiles your code to WASM — clever but heavy. The deterministic replay model, while technically elegant, is unnecessary when stages are already idempotent and state is tracked in SQLite. |

### Adjacent Tools

| Project | Description | What's Similar | What's Missing |
|---------|-------------|----------------|----------------|
| [**acts**](https://docs.rs/acts) | YAML-model-driven workflow engine in Rust with SQLite support, interrupt/resume (`acts.core.irq`), and step/branch/act structure. | Has persistent state via SQLite. Has an interrupt/resume concept for human interaction. Rust-native. | YAML-driven rather than code-first. Stringly-typed API — no compile-time stage validation. Designed for BPMN-style business workflows rather than programmatic data pipelines. No fan-out with per-subtask tracking. The API doesn't feel like idiomatic Rust. |
| [**roxid**](https://github.com/trey-herrington/roxid) | Azure DevOps Pipelines local runner with ratatui TUI. Parses pipeline YAML, builds a DAG, executes stages with real-time progress. Clean three-crate architecture (core library → TUI → CLI). | DAG-based execution with dependency ordering. Event streaming for real-time TUI updates. Excellent crate workspace structure. | A pipeline *runner*, not a workflow *engine*. Executes a defined pipeline once — no persistent state, no work items progressing over time, no pause/resume, no human review gates. Azure DevOps-specific. Useful as an architectural reference but solves a different problem. |
| [**Windmill**](https://windmill.dev) | Open-source workflow engine and developer platform. Rust + PostgreSQL. Supports approval/suspend/resume steps. Very fast. | Persistent state. Approval gates for human-in-the-loop. Rust-based, high performance. | A full platform with a web UI, multi-tenant auth, a script runtime, and a PostgreSQL dependency. Requires running a server. Designed for team-scale automation, not embedding in a CLI tool. |

### Summary

| Need | dagrs/dagx | Restate/Temporal | acts | **Treadle** |
|------|:----------:|:----------------:|:----:|:-----------:|
| DAG execution | ✅ | ✅ | ✅ | ✅ |
| Async/parallel | ✅ | ✅ | ✅ | ✅ |
| Persistent state | ❌ | ✅ | ✅ | ✅ |
| Survives restarts | ❌ | ✅ | ✅ | ✅ |
| Human review gates | ❌ | ❌¹ | ~² | ✅ |
| Fan-out + per-subtask tracking | ❌ | ❌ | ❌ | ✅ |
| Event stream | ❌ | ❌ | ❌ | ✅ |
| No external runtime | ✅ | ❌ | ✅ | ✅ |
| Embeddable library | ✅ | ❌ | ~² | ✅ |
| Work items over time | ❌ | ✅ | ❌ | ✅ |

¹ Restate can model human approval via a durable promise that blocks until
resolved, but it's not a first-class review workflow.
² acts has interrupt/resume and is embeddable, but the YAML-driven,
stringly-typed API limits its use as a general-purpose library.

## Roadmap

- [ ] Core traits: `WorkItem`, `Stage`, `StageOutcome`, `StateStore`
- [ ] petgraph-backed `Workflow` with builder pattern and DAG validation
- [ ] SQLite `StateStore` implementation
- [ ] In-memory `StateStore` for testing
- [ ] Workflow executor with topological stage ordering
- [ ] Fan-out with per-subtask state tracking
- [ ] Event stream via tokio broadcast channel
- [ ] Pipeline status/visualization helpers
- [ ] Documentation and examples

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or
  <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or
  <http://opensource.org/licenses/MIT>)

at your option.
