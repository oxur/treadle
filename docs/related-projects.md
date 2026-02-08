# Related Projects

The Rust ecosystem has several DAG and workflow libraries. Here's how they
compare to treadle and why we needed to build something new.

## Single-Shot DAG Executors

These libraries define a DAG of tasks, execute them (often in parallel), and
return results. They're excellent for computational pipelines but lack
persistence, resumability, and human-in-the-loop support.

| Project | Description | What's Similar | What's Missing |
|---------|-------------|----------------|----------------|
| [**dagrs**](https://crates.io/crates/dagrs) | High-performance async task framework following Flow-Based Programming. ~470 stars, the most mature Rust DAG executor. Supports conditional nodes, loop subgraphs, inter-task communication channels, and custom config parsers. | DAG-based task execution with dependency ordering and parallel scheduling. Async-first with tokio. Trait-based task definition. | No persistent state — if the process stops, all progress is lost. No concept of work items progressing over time. No pause/resume or human review gates. No fan-out with per-subtask tracking. Designed for "run once and done" computation, not ongoing pipelines. |
| [**dagx**](https://lib.rs/crates/dagx) | Minimal, type-safe async DAG executor with compile-time cycle prevention. Uses a `#[task]` macro and `DagRunner` for type-safe dependency wiring. | Compile-time type safety for dependencies. Clean, minimal API. True parallel execution with sub-microsecond overhead. | Same fundamental model as dagrs: single-shot execution with no persistence, no state tracking, no pause/resume, no human-in-the-loop. The type-safety focus is compelling but orthogonal to the problems treadle solves. |
| [**async_dag**](https://docs.rs/async_dag) | Maximizes parallel execution when async tasks form a DAG. Automatic scheduling ensures tasks run as soon as their dependencies complete. | Automatic dependency-aware parallel scheduling. | Purely a scheduling optimizer — no persistence, no state, no work items. Executes a fixed computation graph once. |

## DAG Data Structures

These provide the graph primitives but not execution, state, or workflow
semantics.

| Project | Description | What's Similar | What's Missing |
|---------|-------------|----------------|----------------|
| [**daggy**](https://docs.rs/daggy) | A petgraph wrapper exposing a DAG-specific API with `Walker` traversal. Supports serde serialization and stable node indices. | Built on petgraph (as treadle is). DAG-specific API with cycle prevention. | A data structure, not an execution engine. No state tracking, no async execution, no work items, no event stream. Treadle uses petgraph directly for its DAG backbone. |
| [**petgraph**](https://docs.rs/petgraph) | The foundational Rust graph library. Provides generic graph data structures, algorithms (topological sort, shortest path, etc.), and traversal. | Treadle *uses* petgraph internally for its DAG representation and topological ordering. | A graph library, not a workflow engine. petgraph is a dependency of treadle, not an alternative. |

## Heavyweight / Distributed Workflow Engines

These provide durable execution with strong guarantees, but require external
infrastructure and are designed for distributed systems at scale.

| Project | Description | What's Similar | What's Missing (or rather, what's too much) |
|---------|-------------|----------------|----------------------------------------------|
| [**Restate**](https://restate.dev) (restate-sdk) | Low-latency durable execution engine. Journaled execution with replay, distributed state, saga/compensation patterns. Supports Rust, TypeScript, Java, Go, Python. | Durable state that survives crashes. Workflow orchestration with typed handlers. Rust SDK available. | Requires running a separate Restate server process. Designed for distributed microservices, not local CLI tools. The journaled replay model solves distributed consistency problems that don't apply to single-process pipelines. Overkill for personal tools — like using Kubernetes to run a shell script. |
| [**Temporal**](https://temporal.io) | Microservice orchestration platform with durable execution. Industry-standard for distributed workflows. | Workflow-as-code with durable state. Strong guarantees around exactly-once execution. | Requires a Temporal server cluster. Java/Go/TypeScript native; Rust support is minimal. Designed for large-scale distributed systems. Even heavier than Restate for the local CLI use case. |
| [**Flawless**](https://flawless.dev) | Durable execution engine for Rust. Compiles workflows to WebAssembly, runs in a deterministic sandbox, journals all side effects for replay. | Rust-native. Durable execution with crash recovery. Beautiful technical design. | Requires an external engine process. Compiles your code to WASM — clever but heavy. The deterministic replay model, while technically elegant, is unnecessary when stages are already idempotent and state is tracked in SQLite. |

## Adjacent Tools

| Project | Description | What's Similar | What's Missing |
|---------|-------------|----------------|----------------|
| [**acts**](https://docs.rs/acts) | YAML-model-driven workflow engine in Rust with SQLite support, interrupt/resume (`acts.core.irq`), and step/branch/act structure. | Has persistent state via SQLite. Has an interrupt/resume concept for human interaction. Rust-native. | YAML-driven rather than code-first. Stringly-typed API — no compile-time stage validation. Designed for BPMN-style business workflows rather than programmatic data pipelines. No fan-out with per-subtask tracking. The API doesn't feel like idiomatic Rust. |
| [**roxid**](https://github.com/trey-herrington/roxid) | Azure DevOps Pipelines local runner with ratatui TUI. Parses pipeline YAML, builds a DAG, executes stages with real-time progress. Clean three-crate architecture (core library → TUI → CLI). | DAG-based execution with dependency ordering. Event streaming for real-time TUI updates. Excellent crate workspace structure. | A pipeline *runner*, not a workflow *engine*. Executes a defined pipeline once — no persistent state, no work items progressing over time, no pause/resume, no human review gates. Azure DevOps-specific. Useful as an architectural reference but solves a different problem. |
| [**Windmill**](https://windmill.dev) | Open-source workflow engine and developer platform. Rust + PostgreSQL. Supports approval/suspend/resume steps. Very fast. | Persistent state. Approval gates for human-in-the-loop. Rust-based, high performance. | A full platform with a web UI, multi-tenant auth, a script runtime, and a PostgreSQL dependency. Requires running a server. Designed for team-scale automation, not embedding in a CLI tool. |

## Summary

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
