# Treadle

[![][build-badge]][build]
[![][crate-badge]][crate]
[![][tag-badge]][tag]
[![][docs-badge]][docs]
[![License](https://img.shields.io/crates/l/treadle.svg)](LICENSE-MIT)

[![][logo]][logo-large]

*A persistent, resumable, human-in-the-loop workflow engine backed by a petgraph DAG*

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

For why this project was created and a brief overview of related projects in the Rust ecosystem, be sure to check out:

- [Rust DAG/workflow/pipeline Projects](./docs/related-projects.md)

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

[//]: ---Named-Links---

[logo]: assets/images/logo/v1-x250.png
[logo-large]: assets/images/logo/v1.png
[build]: https://github.com/oxur/treadle/actions/workflows/ci.yml
[build-badge]: https://github.com/oxur/treadle/actions/workflows/ci.yml/badge.svg
[crate]: https://crates.io/crates/treadle
[crate-badge]: https://img.shields.io/crates/v/treadle.svg
[docs]: https://docs.rs/treadle/
[docs-badge]: https://img.shields.io/badge/rust-documentation-blue.svg
[tag-badge]: https://img.shields.io/github/tag/oxur/treadle.svg
[tag]: https://github.com/oxur/treadle/tags
