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
//! ## Quick Start
//!
//! Define your work item and stages:
//!
//! ```rust,ignore
//! use treadle::{Stage, StageOutcome, StageContext, Result, WorkItem};
//! use async_trait::async_trait;
//!
//! // Your work item
//! #[derive(Debug, Clone)]
//! struct Document {
//!     id: String,
//!     content: String,
//! }
//!
//! impl WorkItem for Document {
//!     fn id(&self) -> &str {
//!         &self.id
//!     }
//! }
//!
//! // A processing stage
//! #[derive(Debug)]
//! struct ParseStage;
//!
//! #[async_trait]
//! impl Stage for ParseStage {
//!     fn name(&self) -> &str {
//!         "parse"
//!     }
//!
//!     async fn execute(&self, item: &dyn WorkItem, ctx: &mut StageContext) -> Result<StageOutcome> {
//!         println!("Parsing document");
//!         Ok(StageOutcome::Complete)
//!     }
//! }
//! ```
//!
//! Build and run the workflow:
//!
//! ```rust,ignore
//! use treadle::{Workflow, MemoryStateStore};
//!
//! # async fn example() -> treadle::Result<()> {
//! let workflow = Workflow::builder()
//!     .stage("parse", ParseStage)
//!     .stage("process", ProcessStage)
//!     .dependency("process", "parse")
//!     .build()?;
//!
//! let mut store = MemoryStateStore::new();
//! let doc = Document { id: "doc-1".into(), content: "...".into() };
//!
//! workflow.advance(&doc, &mut store).await?;
//!
//! if workflow.is_complete(doc.id(), &store).await? {
//!     println!("Done!");
//! }
//! # Ok(())
//! # }
//! ```
//!
//! ## Human-in-the-Loop
//!
//! Stages can pause for human review:
//!
//! ```rust,ignore
//! #[async_trait]
//! impl Stage for ReviewStage {
//!     fn name(&self) -> &str {
//!         "review"
//!     }
//!
//!     async fn execute(&self, item: &dyn WorkItem, ctx: &mut StageContext) -> Result<StageOutcome> {
//!         // Stage will pause here, waiting for human approval
//!         Ok(StageOutcome::NeedsReview)
//!     }
//! }
//! ```
//!
//! Later, approve or reject:
//!
//! ```rust,ignore
//! // Approve and continue
//! workflow.approve_review(doc.id(), "review", &mut store).await?;
//! workflow.advance(&doc, &mut store).await?;  // Continues to next stage
//!
//! // Or reject with a reason
//! workflow.reject_review(doc.id(), "review", "Quality issues", &mut store).await?;
//! ```
//!
//! ## Fan-Out Execution
//!
//! Stages can spawn parallel subtasks:
//!
//! ```rust,ignore
//! #[async_trait]
//! impl Stage for EnrichStage {
//!     fn name(&self) -> &str {
//!         "enrich"
//!     }
//!
//!     async fn execute(&self, item: &dyn WorkItem, ctx: &mut StageContext) -> Result<StageOutcome> {
//!         if ctx.subtask_name.is_some() {
//!             // Handle individual subtask execution
//!             let source = ctx.subtask_name.as_ref().unwrap();
//!             println!("Fetching data from {}", source);
//!             Ok(StageOutcome::Complete)
//!         } else {
//!             // Declare subtasks on first invocation
//!             Ok(StageOutcome::FanOut(vec![
//!                 SubTask::new("source-a".to_string()),
//!                 SubTask::new("source-b".to_string()),
//!                 SubTask::new("source-c".to_string()),
//!             ]))
//!         }
//!     }
//! }
//! ```
//!
//! The workflow will execute all subtasks and track their progress independently.
//!
//! ## State Persistence
//!
//! Use [`MemoryStateStore`] for testing or [`SqliteStateStore`] for production:
//!
//! ```rust,ignore
//! // In-memory (testing)
//! let mut store = MemoryStateStore::new();
//!
//! // SQLite (production) - survives process restarts
//! # #[cfg(feature = "sqlite")]
//! let mut store = SqliteStateStore::open("workflow.db").await?;
//! ```
//!
//! State is automatically saved after each stage execution. You can restart
//! your process and resume from where you left off:
//!
//! ```rust,ignore
//! // After restart, load the workflow and continue
//! let workflow = Workflow::builder()
//!     .stage("parse", ParseStage)
//!     .stage("process", ProcessStage)
//!     .build()?;
//!
//! # #[cfg(feature = "sqlite")]
//! let mut store = SqliteStateStore::open("workflow.db").await?;
//!
//! // Continue processing - skips already-completed stages
//! workflow.advance(&doc, &mut store).await?;
//! ```
//!
//! ## Event Observation
//!
//! Subscribe to workflow events for logging, monitoring, or building UIs:
//!
//! ```rust,ignore
//! let mut events = workflow.subscribe();
//!
//! tokio::spawn(async move {
//!     while let Ok(event) = events.recv().await {
//!         match event {
//!             WorkflowEvent::StageStarted { item_id, stage } => {
//!                 println!("Stage {} started for {}", stage, item_id);
//!             }
//!             WorkflowEvent::StageCompleted { item_id, stage } => {
//!                 println!("Stage {} completed for {}", stage, item_id);
//!             }
//!             WorkflowEvent::ReviewRequired { item_id, stage, .. } => {
//!                 println!("Review needed for {} at stage {}", item_id, stage);
//!             }
//!             WorkflowEvent::WorkflowCompleted { item_id } => {
//!                 println!("Workflow completed for {}", item_id);
//!             }
//!             _ => {}
//!         }
//!     }
//! });
//! ```
//!
//! ## Pipeline Status
//!
//! Inspect the current state of a workflow at any time:
//!
//! ```rust,ignore
//! let status = workflow.status(doc.id(), &store).await?;
//!
//! println!("Progress: {:.0}%", status.progress_percent());
//!
//! if status.has_pending_reviews() {
//!     for stage in status.review_stages() {
//!         println!("Review needed at stage: {}", stage);
//!     }
//! }
//!
//! // Pretty-print the entire pipeline status
//! println!("{}", status);
//! ```
//!
//! ## Feature Flags
//!
//! - `sqlite` (default): Enables [`SqliteStateStore`] for persistent storage
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
pub mod event;
pub mod stage;
pub mod state_store;
pub mod status;
pub mod work_item;
pub mod workflow;

pub use error::{Result, TreadleError};
pub use event::WorkflowEvent;
pub use stage::{ReviewData, Stage, StageContext, StageOutcome, StageState, StageStatus, SubTask};
pub use state_store::{MemoryStateStore, StateStore};
pub use status::{PipelineStatus, StageStatusEntry};

#[cfg(feature = "sqlite")]
pub use state_store::SqliteStateStore;

pub use work_item::WorkItem;
pub use workflow::{Workflow, WorkflowBuilder};

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
