//! # Treadle
//!
//! A persistent, resumable, human-in-the-loop workflow engine backed by a
//! [petgraph](https://docs.rs/petgraph) DAG.
//!
//! Treadle fills the gap between single-shot DAG executors (like
//! [dagrs](https://crates.io/crates/dagrs)) and heavyweight distributed
//! workflow engines (like [Restate](https://restate.dev) or
//! [Temporal](https://temporal.io)). It is designed for **local,
//! single-process pipelines** where:
//!
//! - Work items progress through a DAG of stages over time
//! - Each item's state is tracked persistently (survives restarts)
//! - Stages can pause for human review and resume later
//! - Fan-out stages (e.g., enriching from multiple sources) track each
//!   subtask independently with per-subtask retry
//! - The full pipeline is inspectable at any moment
//!
//! ## Quick Example
//!
//! ```rust,ignore
//! use treadle::{Workflow, Stage, StageOutcome, StageContext};
//!
//! // Define your stages by implementing the Stage trait
//! struct Scan;
//! struct Enrich;
//! struct Review;
//!
//! // Build the workflow DAG
//! let workflow = Workflow::builder()
//!     .stage("scan", Scan)
//!     .stage("enrich", Enrich)
//!     .stage("review", Review)
//!     .dependency("enrich", "scan")
//!     .dependency("review", "enrich")
//!     .build()?;
//!
//! // Advance a work item through the pipeline
//! workflow.advance(&my_item, &state_store).await?;
//! ```
//!
//! ## Design Philosophy
//!
//! The name comes from the **treadle** — the foot-operated lever that drives
//! a loom, spinning wheel, or lathe. The machine has stages and mechanisms,
//! but without the human pressing the treadle, nothing moves. This captures
//! the core design: a pipeline engine where human judgment gates the flow.

#![warn(missing_docs)]
#![warn(rustdoc::missing_crate_level_docs)]
#![forbid(unsafe_code)]

pub mod error;
pub mod stage;
pub mod state_store;
pub mod work_item;

pub use error::{Result, TreadleError};
pub use stage::{ReviewData, Stage, StageContext, StageOutcome, StageState, StageStatus, SubTask};
pub use state_store::StateStore;
pub use work_item::WorkItem;

/// Treadle is under active development. See the README for the design
/// and roadmap.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_returns_valid_semver() {
        let version = version();
        assert!(!version.is_empty());
        assert!(version.contains('.'));
    }
}
