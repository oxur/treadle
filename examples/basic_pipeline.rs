//! Basic Treadle pipeline example.
//!
//! This example demonstrates:
//! - Building a workflow with stages and dependencies
//! - Advancing work items through the pipeline
//! - Handling review stages
//! - Checking pipeline status
//! - Observing workflow events
//!
//! Run with: `cargo run --example basic_pipeline`

use async_trait::async_trait;
use treadle::{
    MemoryStateStore, Result, Stage, StageContext, StageOutcome, WorkItem, Workflow,
    WorkflowEvent,
};

/// Our work item - a document to process.
#[derive(Debug, Clone)]
struct Document {
    id: String,
    content: String,
}

impl Document {
    fn new(id: &str, content: &str) -> Self {
        Self {
            id: id.to_string(),
            content: content.to_string(),
        }
    }
}

impl WorkItem for Document {
    fn id(&self) -> &str {
        &self.id
    }
}

/// Stage 1: Scan the document for structure.
#[derive(Debug)]
struct ScanStage;

#[async_trait]
impl Stage for ScanStage {
    fn name(&self) -> &str {
        "scan"
    }

    async fn execute(
        &self,
        item: &dyn WorkItem,
        _ctx: &mut StageContext,
    ) -> Result<StageOutcome> {
        let item_id = item.id().to_string();
        println!("  📄 Scanning document '{}'", item_id);
        // Simulate some processing
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        println!("     ✓ Document structure identified");
        Ok(StageOutcome::Complete)
    }
}

/// Stage 2: Extract entities from the document.
#[derive(Debug)]
struct ExtractStage;

#[async_trait]
impl Stage for ExtractStage {
    fn name(&self) -> &str {
        "extract"
    }

    async fn execute(
        &self,
        item: &dyn WorkItem,
        _ctx: &mut StageContext,
    ) -> Result<StageOutcome> {
        let item_id = item.id().to_string();
        println!("  🔍 Extracting entities from '{}'", item_id);
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        println!("     ✓ Found: 2 people, 1 organization");
        Ok(StageOutcome::Complete)
    }
}

/// Stage 3: Human review of extracted entities.
#[derive(Debug)]
struct ReviewStage;

#[async_trait]
impl Stage for ReviewStage {
    fn name(&self) -> &str {
        "review"
    }

    async fn execute(
        &self,
        item: &dyn WorkItem,
        _ctx: &mut StageContext,
    ) -> Result<StageOutcome> {
        let item_id = item.id().to_string();
        println!("  👀 Review requested for '{}'", item_id);
        println!("     Entities to review:");
        println!("       - Person: John Doe");
        println!("       - Org: Acme Inc");
        // Stage will pause here for human review
        Ok(StageOutcome::NeedsReview)
    }
}

/// Stage 4: Export the finalized document.
#[derive(Debug)]
struct ExportStage;

#[async_trait]
impl Stage for ExportStage {
    fn name(&self) -> &str {
        "export"
    }

    async fn execute(
        &self,
        item: &dyn WorkItem,
        _ctx: &mut StageContext,
    ) -> Result<StageOutcome> {
        let item_id = item.id().to_string();
        println!("  💾 Exporting document '{}'", item_id);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        println!("     ✓ Exported to output.json");
        Ok(StageOutcome::Complete)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    println!("╔═══════════════════════════════════════════╗");
    println!("║  Treadle Basic Pipeline Example          ║");
    println!("╚═══════════════════════════════════════════╝\n");

    // Build the workflow
    println!("📋 Building workflow...");
    let workflow = Workflow::builder()
        .stage("scan", ScanStage)
        .stage("extract", ExtractStage)
        .stage("review", ReviewStage)
        .stage("export", ExportStage)
        .dependency("extract", "scan")
        .dependency("review", "extract")
        .dependency("export", "review")
        .build()?;

    println!("   Stages: {:?}\n", workflow.stages());

    // Subscribe to events (in background)
    let mut event_receiver = workflow.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = event_receiver.recv().await {
            match event {
                WorkflowEvent::StageStarted { stage, .. } => {
                    println!("   [Event] Stage '{}' started", stage);
                }
                WorkflowEvent::StageCompleted { stage, .. } => {
                    println!("   [Event] Stage '{}' completed", stage);
                }
                WorkflowEvent::ReviewRequired { stage, .. } => {
                    println!("   [Event] Review required at stage '{}'", stage);
                }
                WorkflowEvent::WorkflowCompleted { item_id } => {
                    println!("   [Event] Workflow completed for '{}'", item_id);
                }
                _ => {}
            }
        }
    });

    // Create state store (in-memory for this example)
    let mut store = MemoryStateStore::new();

    // Create a document to process
    let doc = Document::new(
        "doc-001",
        "John Doe works at Acme Inc. He manages the engineering team.",
    );

    println!("📄 Processing document: {}", doc.id);
    println!("   Content: \"{}\"\n", doc.content);

    // First advance - will stop at review
    println!("▶️  First Advance (automatic stages)");
    println!("─────────────────────────────────────────");
    workflow.advance(&doc, &mut store).await?;

    // Give events time to print
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Check status
    println!("\n📊 Status after first advance:");
    println!("─────────────────────────────────────────");
    let status = workflow.status(doc.id(), &store).await?;
    println!("{}", status);

    // Check what stages need review
    if status.has_pending_reviews() {
        println!("⏸️  Workflow paused - awaiting human review\n");

        // Simulate human review decision
        println!("👤 Human reviewer checking entities...");
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        println!("   Decision: APPROVED ✓\n");

        // Approve the review
        println!("✅ Approving review stage");
        println!("─────────────────────────────────────────");
        workflow
            .approve_review(doc.id(), "review", &mut store)
            .await?;
    }

    // Second advance - will complete
    println!("\n▶️  Second Advance (remaining stages)");
    println!("─────────────────────────────────────────");
    workflow.advance(&doc, &mut store).await?;

    // Give events time to print
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Final status
    println!("\n📊 Final Status:");
    println!("─────────────────────────────────────────");
    let final_status = workflow.status(doc.id(), &store).await?;
    println!("{}", final_status);

    if workflow.is_complete(doc.id(), &store).await? {
        println!("\n🎉 Success! Document processing complete!\n");
    }

    Ok(())
}
