//! Workflow definition and DAG management.
//!
//! This module provides [`Workflow`] and [`WorkflowBuilder`] for constructing
//! and managing workflow DAGs backed by petgraph.

use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{debug, info, info_span, warn, Instrument};

use crate::status::{PipelineStatus, StageStatusEntry};
use crate::{Result, Stage, StageStatus, StateStore, TreadleError, WorkflowEvent};

/// Default channel capacity for workflow events.
const DEFAULT_EVENT_CHANNEL_CAPACITY: usize = 256;

/// A registered stage in the workflow.
struct RegisteredStage {
    name: String,
    #[allow(dead_code)] // Accessed via graph indexing
    stage: Arc<dyn Stage>,
}

/// A workflow definition backed by a directed acyclic graph (DAG).
///
/// The workflow defines the stages that work items pass through and
/// the dependencies between them. Stages are executed in topological
/// order, respecting all dependency relationships.
///
/// # Construction
///
/// Use [`Workflow::builder()`] to create a new workflow:
///
/// ```
/// use treadle::Workflow;
/// # use treadle::{Stage, StageContext, StageOutcome, WorkItem, Result};
/// # use async_trait::async_trait;
/// #
/// # #[derive(Debug)]
/// # struct ScanStage;
/// # #[async_trait]
/// # impl Stage for ScanStage {
/// #     async fn execute(&self, _item: &dyn WorkItem, _ctx: &mut StageContext) -> Result<StageOutcome> {
/// #         Ok(StageOutcome::Complete)
/// #     }
/// #     fn name(&self) -> &str { "scan" }
/// # }
/// # #[derive(Debug)]
/// # struct EnrichStage;
/// # #[async_trait]
/// # impl Stage for EnrichStage {
/// #     async fn execute(&self, _item: &dyn WorkItem, _ctx: &mut StageContext) -> Result<StageOutcome> {
/// #         Ok(StageOutcome::Complete)
/// #     }
/// #     fn name(&self) -> &str { "enrich" }
/// # }
///
/// let workflow = Workflow::builder()
///     .stage("scan", ScanStage)
///     .stage("enrich", EnrichStage)
///     .dependency("enrich", "scan")
///     .build()?;
/// # Ok::<(), treadle::TreadleError>(())
/// ```
///
/// # Thread Safety
///
/// `Workflow` is `Send + Sync` and can be shared across async tasks.
pub struct Workflow {
    /// The underlying directed graph.
    graph: DiGraph<RegisteredStage, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Cached topological order of stage names.
    topo_order: Vec<String>,
    /// Event broadcast channel sender.
    event_tx: broadcast::Sender<WorkflowEvent>,
}

impl Workflow {
    /// Creates a new workflow builder.
    pub fn builder() -> WorkflowBuilder {
        WorkflowBuilder::new()
    }

    /// Subscribes to workflow execution events.
    ///
    /// Returns a receiver that will receive all events broadcast by this
    /// workflow. Events are not persisted; if the receiver is too slow,
    /// events may be dropped.
    ///
    /// # Example
    ///
    /// ```
    /// # use treadle::Workflow;
    /// let workflow = Workflow::builder().build().unwrap();
    /// let mut events = workflow.subscribe();
    ///
    /// // In a separate task:
    /// // while let Ok(event) = events.recv().await {
    /// //     println!("Event: {:?}", event);
    /// // }
    /// ```
    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_tx.subscribe()
    }

    /// Emits an event to all subscribers.
    ///
    /// Ignores send errors (no subscribers or channel full).
    pub(crate) fn emit(&self, event: WorkflowEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Returns the stage names in topological order.
    ///
    /// This is the order in which stages would be executed if all
    /// dependencies are satisfied.
    pub fn stages(&self) -> &[String] {
        &self.topo_order
    }

    /// Returns the number of stages in the workflow.
    pub fn stage_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Returns true if a stage with the given name exists.
    pub fn has_stage(&self, name: &str) -> bool {
        self.name_to_index.contains_key(name)
    }

    /// Returns the immediate dependencies of a stage.
    ///
    /// These are the stages that must complete before this stage can run.
    ///
    /// # Errors
    ///
    /// Returns [`TreadleError::StageNotFound`] if the stage doesn't exist.
    pub fn dependencies(&self, stage: &str) -> Result<Vec<&str>> {
        let node_index = self
            .name_to_index
            .get(stage)
            .ok_or_else(|| TreadleError::StageNotFound(stage.to_string()))?;

        let deps = self
            .graph
            .neighbors_directed(*node_index, petgraph::Direction::Incoming)
            .map(|idx| self.graph[idx].name.as_str())
            .collect();

        Ok(deps)
    }

    /// Returns the stages that depend on the given stage.
    ///
    /// # Errors
    ///
    /// Returns [`TreadleError::StageNotFound`] if the stage doesn't exist.
    pub fn dependents(&self, stage: &str) -> Result<Vec<&str>> {
        let node_index = self
            .name_to_index
            .get(stage)
            .ok_or_else(|| TreadleError::StageNotFound(stage.to_string()))?;

        let deps = self
            .graph
            .neighbors_directed(*node_index, petgraph::Direction::Outgoing)
            .map(|idx| self.graph[idx].name.as_str())
            .collect();

        Ok(deps)
    }

    /// Returns a reference to the stage with the given name.
    ///
    /// This method is intended for internal use by the workflow executor.
    ///
    /// # Errors
    ///
    /// Returns [`TreadleError::StageNotFound`] if the stage doesn't exist.
    #[allow(dead_code)] // Used by executor in Phase 4
    pub(crate) fn get_stage(&self, name: &str) -> Result<&Arc<dyn Stage>> {
        let node_index = self
            .name_to_index
            .get(name)
            .ok_or_else(|| TreadleError::StageNotFound(name.to_string()))?;

        Ok(&self.graph[*node_index].stage)
    }

    /// Returns stages that have no dependencies (root stages).
    pub fn root_stages(&self) -> Vec<&str> {
        self.topo_order
            .iter()
            .filter(|name| {
                self.dependencies(name)
                    .map(|deps| deps.is_empty())
                    .unwrap_or(false)
            })
            .map(|s| s.as_str())
            .collect()
    }

    /// Returns stages that have no dependents (leaf stages).
    pub fn leaf_stages(&self) -> Vec<&str> {
        self.topo_order
            .iter()
            .filter(|name| {
                self.dependents(name)
                    .map(|deps| deps.is_empty())
                    .unwrap_or(false)
            })
            .map(|s| s.as_str())
            .collect()
    }

    /// Returns the stages that are ready to execute for a work item.
    ///
    /// A stage is ready if:
    /// - It is in Pending status (or has no status record)
    /// - All of its dependencies are Complete
    ///
    /// # Errors
    ///
    /// Returns an error if the state store cannot be queried.
    pub async fn ready_stages<S: StateStore>(
        &self,
        work_item_id: &str,
        store: &S,
    ) -> Result<Vec<String>> {
        let all_states = store.get_all_stage_states(work_item_id).await?;
        let mut ready = Vec::new();

        for stage_name in &self.topo_order {
            // Get the stage's current state
            let state = all_states.get(stage_name);

            // Skip if already running, completed, failed, or paused
            match state {
                Some(s) => match s.status {
                    crate::StageStatus::Pending => {
                        // Check dependencies
                    }
                    crate::StageStatus::InProgress
                    | crate::StageStatus::Complete
                    | crate::StageStatus::Failed
                    | crate::StageStatus::Paused => {
                        continue;
                    }
                },
                None => {
                    // No state recorded yet, treat as pending
                }
            }

            // Check if all dependencies are satisfied
            let deps = self.dependencies(stage_name)?;
            let all_deps_satisfied = deps.iter().all(|dep_name| {
                all_states
                    .get(*dep_name)
                    .map(|s| matches!(s.status, crate::StageStatus::Complete))
                    .unwrap_or(false)
            });

            if all_deps_satisfied {
                ready.push(stage_name.clone());
            }
        }

        Ok(ready)
    }

    /// Returns true if the workflow is complete for the given work item.
    ///
    /// Complete means all stages are in Complete status.
    ///
    /// # Errors
    ///
    /// Returns an error if the state store cannot be queried.
    pub async fn is_complete<S: StateStore>(
        &self,
        work_item_id: &str,
        store: &S,
    ) -> Result<bool> {
        let all_states = store.get_all_stage_states(work_item_id).await?;

        // All stages must have a completed status
        for stage_name in &self.topo_order {
            match all_states.get(stage_name) {
                Some(s) if matches!(s.status, crate::StageStatus::Complete) => {
                    continue;
                }
                _ => return Ok(false),
            }
        }

        Ok(true)
    }

    /// Returns true if the workflow is blocked for the given work item.
    ///
    /// Blocked means no stages are ready to execute and the workflow
    /// is not complete (something is paused or failed).
    ///
    /// # Errors
    ///
    /// Returns an error if the state store cannot be queried.
    pub async fn is_blocked<S: StateStore>(
        &self,
        work_item_id: &str,
        store: &S,
    ) -> Result<bool> {
        if self.is_complete(work_item_id, store).await? {
            return Ok(false);
        }

        let ready = self.ready_stages(work_item_id, store).await?;
        Ok(ready.is_empty())
    }

    /// Returns the current status of the pipeline for a work item.
    ///
    /// This provides a complete snapshot of the workflow state, including
    /// all stage statuses and subtask details.
    ///
    /// # Example
    ///
    /// ```
    /// # use treadle::{Workflow, MemoryStateStore, Stage, StageContext, StageOutcome, WorkItem, Result};
    /// # use async_trait::async_trait;
    /// # #[derive(Debug)]
    /// # struct TestStage;
    /// # #[async_trait]
    /// # impl Stage for TestStage {
    /// #     async fn execute(&self, _item: &dyn WorkItem, _ctx: &mut StageContext) -> Result<StageOutcome> {
    /// #         Ok(StageOutcome::Complete)
    /// #     }
    /// #     fn name(&self) -> &str { "test" }
    /// # }
    /// # #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
    /// # struct Item { id: String }
    /// # impl WorkItem for Item { fn id(&self) -> &str { &self.id } }
    /// # async fn example() -> Result<()> {
    /// # let workflow = Workflow::builder().stage("test", TestStage).build()?;
    /// # let mut store = MemoryStateStore::new();
    /// # let item = Item { id: "item-1".to_string() };
    /// let status = workflow.status(&item.id, &store).await?;
    /// println!("{}", status);
    ///
    /// if status.is_complete() {
    ///     println!("All done!");
    /// } else if status.has_pending_reviews() {
    ///     for stage in status.review_stages() {
    ///         println!("Stage {} needs review", stage);
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the state store cannot be queried.
    pub async fn status<S: StateStore>(
        &self,
        work_item_id: &str,
        store: &S,
    ) -> Result<PipelineStatus> {
        let all_states = store.get_all_stage_states(work_item_id).await?;
        let mut stages = Vec::with_capacity(self.topo_order.len());

        for stage_name in &self.topo_order {
            let entry = match all_states.get(stage_name) {
                Some(state) => StageStatusEntry::from_stage_state(stage_name, state),
                None => StageStatusEntry::pending(stage_name),
            };
            stages.push(entry);
        }

        Ok(PipelineStatus::new(work_item_id, stages))
    }

    /// Advances a work item through the workflow.
    ///
    /// This method:
    /// 1. Finds all stages that are ready to execute
    /// 2. Executes each ready stage
    /// 3. Updates state based on outcomes
    /// 4. Recursively advances if progress was made
    ///
    /// Execution stops when:
    /// - No more stages are ready (workflow blocked or complete)
    /// - A stage returns `NeedsReview`
    /// - A stage fails
    ///
    /// # Arguments
    ///
    /// * `item` - The work item to advance
    /// * `store` - The state store for persistence
    ///
    /// # Returns
    ///
    /// `Ok(())` when no more progress can be made in this call.
    ///
    /// # Errors
    ///
    /// Returns an error if stage execution or state persistence fails.
    pub async fn advance<S: StateStore>(
        &self,
        item: &dyn crate::WorkItem,
        store: &mut S,
    ) -> Result<()> {
        let span = info_span!("advance", item_id = %item.id());
        self.advance_internal(item, store, 0).instrument(span).await
    }

    /// Internal recursive advance with depth tracking.
    async fn advance_internal<S: StateStore>(
        &self,
        item: &dyn crate::WorkItem,
        store: &mut S,
        depth: usize,
    ) -> Result<()> {
        // Safety limit to prevent infinite recursion
        const MAX_DEPTH: usize = 100;
        if depth > MAX_DEPTH {
            warn!("maximum recursion depth exceeded");
            return Err(TreadleError::StageExecution(
                "maximum recursion depth exceeded".to_string(),
            ));
        }

        debug!(depth = depth, "checking for ready stages");

        let ready_stages = self.ready_stages(item.id(), store).await?;
        if ready_stages.is_empty() {
            debug!("no ready stages");
            // Check if complete
            if self.is_complete(item.id(), store).await? {
                info!("workflow complete");
                self.emit(WorkflowEvent::WorkflowCompleted {
                    item_id: item.id().to_string(),
                });
            }
            return Ok(());
        }

        debug!(ready = ?ready_stages, "found ready stages");

        let mut made_progress = false;

        for stage_name in ready_stages {
            let outcome = self.execute_stage(item, &stage_name, store).await;

            match outcome {
                Ok(crate::StageOutcome::Complete) => {
                    made_progress = true;
                }
                Ok(crate::StageOutcome::NeedsReview) => {
                    // Stop advancing this path - review required
                }
                Ok(crate::StageOutcome::Retry) => {
                    // For now, treat retry as needing review
                    // Full retry logic will be in Milestone 4.4
                }
                Ok(crate::StageOutcome::Failed) => {
                    // Stage failed - already recorded, stop this path
                }
                Ok(crate::StageOutcome::FanOut(subtasks)) => {
                    // Handle fan-out execution
                    let fanout_result = self
                        .execute_fanout(item, &stage_name, subtasks, store)
                        .await;

                    match fanout_result {
                        Ok(true) => made_progress = true,
                        Ok(false) => {
                            // Some subtasks failed or didn't complete
                        }
                        Err(e) => return Err(e),
                    }
                }
                Err(_) => {
                    // Stage failed - already recorded, stop this path
                }
            }
        }

        // Recurse if we made progress
        if made_progress {
            Box::pin(self.advance_internal(item, store, depth + 1)).await?;
        }

        Ok(())
    }

    /// Executes a single stage.
    async fn execute_stage<S: StateStore>(
        &self,
        item: &dyn crate::WorkItem,
        stage_name: &str,
        store: &mut S,
    ) -> Result<crate::StageOutcome> {
        let span = info_span!("stage", stage = %stage_name);

        async {
            info!("executing stage");

            let stage = self.get_stage(stage_name)?;
            let item_id = item.id();

            // Load existing state or create new one
            let mut state = store
                .get_stage_state(item_id, stage_name)
                .await?
                .unwrap_or_else(crate::StageState::new);

            // Mark as in progress (preserves retry_count from any previous attempts)
            state.mark_in_progress();
            store.save_stage_state(item_id, stage_name, &state).await?;

            // Emit start event
            self.emit(WorkflowEvent::StageStarted {
                item_id: item_id.to_string(),
                stage: stage_name.to_string(),
            });

            // Build context
            let mut ctx = crate::StageContext::new(stage_name.to_string());
            ctx.stage_state = state.clone();

            // Execute the stage
            let result = stage.execute(item, &mut ctx).await;

            match &result {
                Ok(outcome) => {
                    info!(outcome = ?outcome, "stage completed");
                }
                Err(e) => {
                    warn!(error = %e, "stage failed");
                }
            }

            match result {
                Ok(outcome) => {
                    self.handle_outcome(item, stage_name, &outcome, store).await?;
                    Ok(outcome)
                }
                Err(e) => {
                    // Mark as failed
                    let error_msg = e.to_string();
                    state.mark_failed(error_msg.clone());
                    store.save_stage_state(item_id, stage_name, &state).await?;

                    self.emit(WorkflowEvent::StageFailed {
                        item_id: item_id.to_string(),
                        stage: stage_name.to_string(),
                        error: error_msg,
                    });

                    Err(e)
                }
            }
        }
        .instrument(span)
        .await
    }

    /// Handles a successful stage outcome.
    async fn handle_outcome<S: StateStore>(
        &self,
        item: &dyn crate::WorkItem,
        stage_name: &str,
        outcome: &crate::StageOutcome,
        store: &mut S,
    ) -> Result<()> {
        let item_id = item.id();

        // Load the existing state to preserve retry_count and other fields
        let mut state = store
            .get_stage_state(item_id, stage_name)
            .await?
            .unwrap_or_else(crate::StageState::new);

        match outcome {
            crate::StageOutcome::Complete => {
                state.mark_complete();
                store.save_stage_state(item_id, stage_name, &state).await?;

                self.emit(WorkflowEvent::StageCompleted {
                    item_id: item_id.to_string(),
                    stage: stage_name.to_string(),
                });
            }
            crate::StageOutcome::NeedsReview => {
                state.mark_paused();
                store.save_stage_state(item_id, stage_name, &state).await?;

                // Create review data
                let review_data = crate::ReviewData::new(
                    item_id.to_string(),
                    stage_name.to_string(),
                    format!("Review required for stage {}", stage_name),
                );

                self.emit(WorkflowEvent::ReviewRequired {
                    item_id: item_id.to_string(),
                    stage: stage_name.to_string(),
                    data: review_data,
                });
            }
            crate::StageOutcome::Retry => {
                // For now, mark as paused for retry
                // Full retry logic in Milestone 4.4
                state.mark_paused();
                state.increment_retry();
                store.save_stage_state(item_id, stage_name, &state).await?;

                self.emit(WorkflowEvent::StageRetried {
                    item_id: item_id.to_string(),
                    stage: stage_name.to_string(),
                });
            }
            crate::StageOutcome::Failed => {
                state.mark_failed("Stage failed".to_string());
                store.save_stage_state(item_id, stage_name, &state).await?;

                self.emit(WorkflowEvent::StageFailed {
                    item_id: item_id.to_string(),
                    stage: stage_name.to_string(),
                    error: "Stage failed".to_string(),
                });
            }
            crate::StageOutcome::FanOut(_) => {
                // FanOut is handled separately in advance_internal
                // This case should not be reached in normal execution
            }
        }

        Ok(())
    }

    /// Executes a fan-out stage with multiple subtasks.
    ///
    /// Returns `true` if all subtasks completed successfully.
    async fn execute_fanout<S: StateStore>(
        &self,
        item: &dyn crate::WorkItem,
        stage_name: &str,
        subtasks: Vec<crate::SubTask>,
        store: &mut S,
    ) -> Result<bool> {
        let span = info_span!(
            "fanout",
            stage = %stage_name,
            subtask_count = subtasks.len()
        );

        async {
            info!("starting fan-out execution");

            let item_id = item.id();
            let stage = self.get_stage(stage_name)?;

            // Create or load stage state with subtasks
            let mut state = store
                .get_stage_state(item_id, stage_name)
                .await?
                .unwrap_or_else(crate::StageState::new);

            // Initialize subtasks in state if not already present
            if state.subtasks.is_empty() {
                state.subtasks = subtasks.clone();
                state.mark_in_progress();
                store.save_stage_state(item_id, stage_name, &state).await?;
            }

            // Emit fan-out started event
            let subtask_names: Vec<String> = subtasks.iter().map(|s| s.id.clone()).collect();
            self.emit(WorkflowEvent::FanOutStarted {
                item_id: item_id.to_string(),
                stage: stage_name.to_string(),
                subtasks: subtask_names.clone(),
            });

            let mut all_completed = true;
            let mut any_failed = false;

            // Execute each subtask
            for (idx, subtask) in subtasks.iter().enumerate() {
                debug!(subtask = %subtask.id, "executing subtask");

                self.emit(WorkflowEvent::SubTaskStarted {
                    item_id: item_id.to_string(),
                    stage: stage_name.to_string(),
                    subtask: subtask.id.clone(),
                });

                // Build context with subtask name
                let mut ctx = crate::StageContext::new(stage_name.to_string())
                    .with_subtask(&subtask.id);

                // Add a fresh stage state for the subtask context
                ctx.stage_state = crate::StageState::new();

                let result = stage.execute(item, &mut ctx).await;

                match result {
                    Ok(crate::StageOutcome::Complete) => {
                        // Update subtask status in state
                        state.subtasks[idx].status = StageStatus::Complete;
                        store.save_stage_state(item_id, stage_name, &state).await?;

                        self.emit(WorkflowEvent::SubTaskCompleted {
                            item_id: item_id.to_string(),
                            stage: stage_name.to_string(),
                            subtask: subtask.id.clone(),
                        });
                    }
                    Ok(crate::StageOutcome::NeedsReview)
                    | Ok(crate::StageOutcome::Retry)
                    | Ok(crate::StageOutcome::FanOut(_)) => {
                        // Subtasks shouldn't return these outcomes
                        all_completed = false;
                    }
                    Ok(crate::StageOutcome::Failed) => {
                        state.subtasks[idx].status = StageStatus::Failed;
                        state.subtasks[idx].error = Some("Subtask returned Failed outcome".to_string());
                        store.save_stage_state(item_id, stage_name, &state).await?;

                        self.emit(WorkflowEvent::SubTaskFailed {
                            item_id: item_id.to_string(),
                            stage: stage_name.to_string(),
                            subtask: subtask.id.clone(),
                            error: "Subtask returned Failed outcome".to_string(),
                        });
                        any_failed = true;
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        state.subtasks[idx].status = StageStatus::Failed;
                        state.subtasks[idx].error = Some(error_msg.clone());
                        store.save_stage_state(item_id, stage_name, &state).await?;

                        self.emit(WorkflowEvent::SubTaskFailed {
                            item_id: item_id.to_string(),
                            stage: stage_name.to_string(),
                            subtask: subtask.id.clone(),
                            error: error_msg.clone(),
                        });
                        any_failed = true;
                    }
                }
            }

            // Update parent stage status based on subtask outcomes
            if any_failed {
                warn!(failed_count = subtasks.len() - all_completed as usize, "fan-out had failures");
                state.mark_failed("one or more subtasks failed".to_string());
                store.save_stage_state(item_id, stage_name, &state).await?;

                self.emit(WorkflowEvent::StageFailed {
                    item_id: item_id.to_string(),
                    stage: stage_name.to_string(),
                    error: "one or more subtasks failed".to_string(),
                });

                Ok(false)
            } else if all_completed {
                info!("all subtasks completed successfully");
                state.mark_complete();
                store.save_stage_state(item_id, stage_name, &state).await?;

                self.emit(WorkflowEvent::StageCompleted {
                    item_id: item_id.to_string(),
                    stage: stage_name.to_string(),
                });

                Ok(true)
            } else {
                debug!("some subtasks didn't complete normally");
                // Some subtasks didn't complete normally
                Ok(false)
            }
        }
        .instrument(span)
        .await
    }

    /// Resets a failed stage to pending, allowing retry.
    ///
    /// This is useful for implementing retry logic outside the engine.
    ///
    /// # Arguments
    ///
    /// * `work_item_id` - The work item's identifier
    /// * `stage_name` - The stage name to reset
    /// * `store` - The state store
    ///
    /// # Errors
    ///
    /// Returns an error if the stage doesn't exist or the current state
    /// is not `Failed`.
    pub async fn retry_stage<S: StateStore>(
        &self,
        work_item_id: &str,
        stage_name: &str,
        store: &mut S,
    ) -> Result<()> {
        if !self.has_stage(stage_name) {
            return Err(TreadleError::StageNotFound(stage_name.to_string()));
        }

        let current = store.get_stage_state(work_item_id, stage_name).await?;
        match current {
            Some(state) if matches!(state.status, crate::StageStatus::Failed) => {
                let mut new_state = state.clone();
                new_state.status = crate::StageStatus::Pending;
                new_state.increment_retry();
                store
                    .save_stage_state(work_item_id, stage_name, &new_state)
                    .await?;
                Ok(())
            }
            Some(_) => Err(TreadleError::StageExecution(
                "Stage is not in Failed state".to_string(),
            )),
            None => {
                // No status recorded, treat as already pending
                Ok(())
            }
        }
    }

    /// Approves a review, transitioning the stage to completed.
    ///
    /// # Arguments
    ///
    /// * `work_item_id` - The work item's identifier
    /// * `stage_name` - The stage name to approve
    /// * `store` - The state store
    ///
    /// # Errors
    ///
    /// Returns an error if the stage is not in `Paused` state.
    pub async fn approve_review<S: StateStore>(
        &self,
        work_item_id: &str,
        stage_name: &str,
        store: &mut S,
    ) -> Result<()> {
        if !self.has_stage(stage_name) {
            return Err(TreadleError::StageNotFound(stage_name.to_string()));
        }

        let current = store.get_stage_state(work_item_id, stage_name).await?;
        match current {
            Some(state) if matches!(state.status, crate::StageStatus::Paused) => {
                let mut new_state = state.clone();
                new_state.mark_complete();
                store
                    .save_stage_state(work_item_id, stage_name, &new_state)
                    .await?;

                // Emit completion event
                self.emit(WorkflowEvent::StageCompleted {
                    item_id: work_item_id.to_string(),
                    stage: stage_name.to_string(),
                });

                Ok(())
            }
            Some(_) => Err(TreadleError::StageExecution(
                "Stage is not in Paused state".to_string(),
            )),
            None => Err(TreadleError::StageExecution(
                "Stage has no recorded state".to_string(),
            )),
        }
    }

    /// Rejects a review, transitioning the stage to failed.
    ///
    /// # Arguments
    ///
    /// * `work_item_id` - The work item's identifier
    /// * `stage_name` - The stage name to reject
    /// * `reason` - The rejection reason
    /// * `store` - The state store
    ///
    /// # Errors
    ///
    /// Returns an error if the stage is not in `Paused` state.
    pub async fn reject_review<S: StateStore>(
        &self,
        work_item_id: &str,
        stage_name: &str,
        reason: impl Into<String>,
        store: &mut S,
    ) -> Result<()> {
        if !self.has_stage(stage_name) {
            return Err(TreadleError::StageNotFound(stage_name.to_string()));
        }

        let current = store.get_stage_state(work_item_id, stage_name).await?;
        match current {
            Some(state) if matches!(state.status, crate::StageStatus::Paused) => {
                let error_msg = reason.into();
                let mut new_state = state.clone();
                new_state.mark_failed(error_msg.clone());
                store
                    .save_stage_state(work_item_id, stage_name, &new_state)
                    .await?;

                // Emit failure event
                self.emit(WorkflowEvent::StageFailed {
                    item_id: work_item_id.to_string(),
                    stage: stage_name.to_string(),
                    error: error_msg,
                });

                Ok(())
            }
            Some(_) => Err(TreadleError::StageExecution(
                "Stage is not in Paused state".to_string(),
            )),
            None => Err(TreadleError::StageExecution(
                "Stage has no recorded state".to_string(),
            )),
        }
    }
}

impl std::fmt::Debug for Workflow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workflow")
            .field("stages", &self.topo_order)
            .field("stage_count", &self.stage_count())
            .finish()
    }
}

/// A deferred dependency specification.
struct DeferredDependency {
    stage: String,
    depends_on: String,
}

/// Builder for constructing [`Workflow`] instances.
///
/// # Example
///
/// ```
/// use treadle::Workflow;
/// # use treadle::{Stage, StageContext, StageOutcome, WorkItem, Result};
/// # use async_trait::async_trait;
/// #
/// # #[derive(Debug)]
/// # struct StageA;
/// # #[async_trait]
/// # impl Stage for StageA {
/// #     async fn execute(&self, _item: &dyn WorkItem, _ctx: &mut StageContext) -> Result<StageOutcome> {
/// #         Ok(StageOutcome::Complete)
/// #     }
/// #     fn name(&self) -> &str { "a" }
/// # }
/// # #[derive(Debug)]
/// # struct StageB;
/// # #[async_trait]
/// # impl Stage for StageB {
/// #     async fn execute(&self, _item: &dyn WorkItem, _ctx: &mut StageContext) -> Result<StageOutcome> {
/// #         Ok(StageOutcome::Complete)
/// #     }
/// #     fn name(&self) -> &str { "b" }
/// # }
///
/// let workflow = Workflow::builder()
///     .stage("a", StageA)
///     .stage("b", StageB)
///     .dependency("b", "a")  // b depends on a
///     .build()?;
/// # Ok::<(), treadle::TreadleError>(())
/// ```
pub struct WorkflowBuilder {
    /// The graph being built.
    graph: DiGraph<RegisteredStage, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Deferred dependencies to be resolved at build time.
    deferred_deps: Vec<DeferredDependency>,
}

impl WorkflowBuilder {
    /// Creates a new, empty workflow builder.
    fn new() -> Self {
        Self {
            graph: DiGraph::new(),
            name_to_index: HashMap::new(),
            deferred_deps: Vec::new(),
        }
    }

    /// Adds a stage to the workflow.
    ///
    /// # Panics
    ///
    /// Panics if a stage with the same name already exists. Use
    /// [`try_stage`](Self::try_stage) for a fallible version.
    pub fn stage(mut self, name: impl Into<String>, stage: impl Stage + 'static) -> Self {
        let name = name.into();
        if self.name_to_index.contains_key(&name) {
            panic!("duplicate stage name: {}", name);
        }

        let registered = RegisteredStage {
            name: name.clone(),
            stage: Arc::new(stage),
        };
        let index = self.graph.add_node(registered);
        self.name_to_index.insert(name, index);
        self
    }

    /// Adds a stage to the workflow, returning an error on duplicate.
    ///
    /// This is the fallible version of [`stage`](Self::stage).
    pub fn try_stage(
        mut self,
        name: impl Into<String>,
        stage: impl Stage + 'static,
    ) -> Result<Self> {
        let name = name.into();
        if self.name_to_index.contains_key(&name) {
            return Err(TreadleError::DuplicateStage(name));
        }

        let registered = RegisteredStage {
            name: name.clone(),
            stage: Arc::new(stage),
        };
        let index = self.graph.add_node(registered);
        self.name_to_index.insert(name, index);
        Ok(self)
    }

    /// Declares that `stage` depends on `depends_on`.
    ///
    /// This means `depends_on` must complete before `stage` can run.
    ///
    /// Dependencies are validated at build time.
    pub fn dependency(mut self, stage: impl Into<String>, depends_on: impl Into<String>) -> Self {
        self.deferred_deps.push(DeferredDependency {
            stage: stage.into(),
            depends_on: depends_on.into(),
        });
        self
    }

    /// Builds the workflow, validating the DAG.
    ///
    /// # Errors
    ///
    /// - [`TreadleError::StageNotFound`] if a dependency references a
    ///   non-existent stage
    /// - [`TreadleError::DagCycle`] if the dependencies form a cycle
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

            // Edge direction: depends_on → stage (dependency flows forward)
            self.graph.add_edge(*depends_on_idx, *stage_idx, ());
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
        })
    }
}

impl Default for WorkflowBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StageContext, StageOutcome, WorkItem};
    use async_trait::async_trait;
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // Test work item
    #[allow(dead_code)]
    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestItem {
        id: String,
    }

    impl WorkItem for TestItem {
        fn id(&self) -> &str {
            &self.id
        }
    }

    // Simple test stage that always completes
    #[derive(Debug)]
    struct TestStage {
        name: String,
    }

    impl TestStage {
        fn new(name: impl Into<String>) -> Self {
            Self { name: name.into() }
        }
    }

    #[async_trait]
    impl Stage for TestStage {
        fn name(&self) -> &str {
            &self.name
        }

        async fn execute(
            &self,
            _item: &dyn WorkItem,
            _ctx: &mut StageContext,
        ) -> Result<StageOutcome> {
            Ok(StageOutcome::Complete)
        }
    }

    #[test]
    fn test_empty_workflow() {
        let workflow = Workflow::builder().build().unwrap();
        assert_eq!(workflow.stage_count(), 0);
        assert!(workflow.stages().is_empty());
    }

    #[test]
    fn test_single_stage() {
        let workflow = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .build()
            .unwrap();

        assert_eq!(workflow.stage_count(), 1);
        assert!(workflow.has_stage("scan"));
        assert!(!workflow.has_stage("other"));
    }

    #[test]
    fn test_multiple_stages() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .build()
            .unwrap();

        assert_eq!(workflow.stage_count(), 3);
        assert!(workflow.has_stage("a"));
        assert!(workflow.has_stage("b"));
        assert!(workflow.has_stage("c"));
    }

    #[test]
    #[should_panic(expected = "duplicate stage name")]
    fn test_duplicate_stage_panics() {
        let _ = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .stage("scan", TestStage::new("scan")) // Duplicate!
            .build();
    }

    #[test]
    fn test_try_stage_duplicate_returns_error() {
        let result = Workflow::builder()
            .try_stage("scan", TestStage::new("scan"))
            .and_then(|b| b.try_stage("scan", TestStage::new("scan")))
            .and_then(|b| b.build());

        assert!(matches!(result, Err(TreadleError::DuplicateStage(_))));
    }

    #[test]
    fn test_linear_pipeline_topo_order() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a") // b depends on a
            .dependency("c", "b") // c depends on b
            .build()
            .unwrap();

        let stages = workflow.stages();
        assert_eq!(stages.len(), 3);

        // a must come before b, b must come before c
        let a_pos = stages.iter().position(|s| s == "a").unwrap();
        let b_pos = stages.iter().position(|s| s == "b").unwrap();
        let c_pos = stages.iter().position(|s| s == "c").unwrap();

        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_dependencies() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        // a has no dependencies
        assert!(workflow.dependencies("a").unwrap().is_empty());

        // b depends on a
        let b_deps = workflow.dependencies("b").unwrap();
        assert_eq!(b_deps, vec!["a"]);

        // c depends on b
        let c_deps = workflow.dependencies("c").unwrap();
        assert_eq!(c_deps, vec!["b"]);
    }

    #[test]
    fn test_dependents() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        // a has b as dependent
        let a_dependents = workflow.dependents("a").unwrap();
        assert_eq!(a_dependents, vec!["b"]);

        // b has c as dependent
        let b_dependents = workflow.dependents("b").unwrap();
        assert_eq!(b_dependents, vec!["c"]);

        // c has no dependents
        assert!(workflow.dependents("c").unwrap().is_empty());
    }

    #[test]
    fn test_root_and_leaf_stages() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        assert_eq!(workflow.root_stages(), vec!["a"]);
        assert_eq!(workflow.leaf_stages(), vec!["c"]);
    }

    #[test]
    fn test_stage_not_found_in_dependency() {
        let result = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .dependency("b", "a") // b doesn't exist
            .build();

        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    #[test]
    fn test_stage_not_found_in_depends_on() {
        let result = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .dependency("a", "b") // b doesn't exist
            .build();

        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    #[test]
    fn test_stage_not_found_query() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        assert!(matches!(
            workflow.dependencies("nonexistent"),
            Err(TreadleError::StageNotFound(_))
        ));
    }

    #[test]
    fn test_get_stage() {
        let workflow = Workflow::builder()
            .stage("test", TestStage::new("test"))
            .build()
            .unwrap();

        let stage = workflow.get_stage("test").unwrap();
        assert_eq!(stage.name(), "test");
    }

    #[test]
    fn test_workflow_debug() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let debug_output = format!("{:?}", workflow);
        assert!(debug_output.contains("Workflow"));
        assert!(debug_output.contains("stages"));
        assert!(debug_output.contains("stage_count"));
    }

    // Milestone 3.2 tests: Complex DAGs and cycle detection

    #[test]
    fn test_diamond_dag() {
        // Diamond: A → B, A → C, B → D, C → D
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .stage("d", TestStage::new("d"))
            .dependency("b", "a")
            .dependency("c", "a")
            .dependency("d", "b")
            .dependency("d", "c")
            .build()
            .unwrap();

        let stages = workflow.stages();
        assert_eq!(stages.len(), 4);

        // Verify ordering constraints
        let a_pos = stages.iter().position(|s| s == "a").unwrap();
        let b_pos = stages.iter().position(|s| s == "b").unwrap();
        let c_pos = stages.iter().position(|s| s == "c").unwrap();
        let d_pos = stages.iter().position(|s| s == "d").unwrap();

        assert!(a_pos < b_pos);
        assert!(a_pos < c_pos);
        assert!(b_pos < d_pos);
        assert!(c_pos < d_pos);

        // d depends on both b and c
        let d_deps = workflow.dependencies("d").unwrap();
        assert_eq!(d_deps.len(), 2);
        assert!(d_deps.contains(&"b"));
        assert!(d_deps.contains(&"c"));
    }

    #[test]
    fn test_cycle_detection_direct() {
        // Direct cycle: A → B → A
        let result = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .dependency("a", "b") // Creates cycle
            .build();

        assert!(matches!(result, Err(TreadleError::DagCycle)));
    }

    #[test]
    fn test_cycle_detection_indirect() {
        // Indirect cycle: A → B → C → A
        let result = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .dependency("a", "c") // Creates cycle
            .build();

        assert!(matches!(result, Err(TreadleError::DagCycle)));
    }

    #[test]
    fn test_self_dependency() {
        // Self-dependency: A → A
        let result = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .dependency("a", "a") // Self-cycle
            .build();

        assert!(matches!(result, Err(TreadleError::DagCycle)));
    }

    #[test]
    fn test_multiple_roots() {
        // Two independent chains: A → B, C → D
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .stage("d", TestStage::new("d"))
            .dependency("b", "a")
            .dependency("d", "c")
            .build()
            .unwrap();

        let roots = workflow.root_stages();
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&"a"));
        assert!(roots.contains(&"c"));

        let leaves = workflow.leaf_stages();
        assert_eq!(leaves.len(), 2);
        assert!(leaves.contains(&"b"));
        assert!(leaves.contains(&"d"));
    }

    #[test]
    fn test_complex_dag() {
        //       B
        //      / \
        //     A   D → E
        //      \ /
        //       C
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .stage("d", TestStage::new("d"))
            .stage("e", TestStage::new("e"))
            .dependency("b", "a")
            .dependency("c", "a")
            .dependency("d", "b")
            .dependency("d", "c")
            .dependency("e", "d")
            .build()
            .unwrap();

        assert_eq!(workflow.stage_count(), 5);
        assert_eq!(workflow.root_stages(), vec!["a"]);
        assert_eq!(workflow.leaf_stages(), vec!["e"]);

        // Verify full dependency chain for e
        let e_deps = workflow.dependencies("e").unwrap();
        assert_eq!(e_deps, vec!["d"]);

        let d_deps = workflow.dependencies("d").unwrap();
        assert_eq!(d_deps.len(), 2);
    }

    // Milestone 4.2 tests: Workflow execution with advance()

    #[tokio::test]
    async fn test_advance_single_stage() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        // Stage should be complete
        let state = store
            .get_stage_state(&item.id, "a")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state.status, crate::StageStatus::Complete));

        // Workflow should be complete
        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
        assert!(!workflow.is_blocked(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_linear_pipeline() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        // All stages should be complete
        for stage_name in &["a", "b", "c"] {
            let state = store
                .get_stage_state(&item.id, stage_name)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(state.status, crate::StageStatus::Complete),
                "Stage {} should be complete",
                stage_name
            );
        }

        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_ready_stages_fresh_item() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        let store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        // Fresh item: only root stage is ready
        let ready = workflow.ready_stages(item_id, &store).await.unwrap();
        assert_eq!(ready, vec!["a"]);
    }

    #[tokio::test]
    async fn test_ready_stages_after_completion() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        // Complete stage a
        let mut state = crate::StageState::new();
        state.mark_complete();
        store.save_stage_state(item_id, "a", &state).await.unwrap();

        // Now b should be ready
        let ready = workflow.ready_stages(item_id, &store).await.unwrap();
        assert_eq!(ready, vec!["b"]);
    }

    #[tokio::test]
    async fn test_is_complete_and_blocked() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item_id = "item-1";

        // Initially not complete and not blocked (has ready stages)
        assert!(!workflow.is_complete(item_id, &store).await.unwrap());
        assert!(!workflow.is_blocked(item_id, &store).await.unwrap());

        // Mark a as in progress
        let mut state = crate::StageState::new();
        state.mark_in_progress();
        store.save_stage_state(item_id, "a", &state).await.unwrap();

        // Now blocked (no ready stages, not complete)
        assert!(!workflow.is_complete(item_id, &store).await.unwrap());
        assert!(workflow.is_blocked(item_id, &store).await.unwrap());

        // Complete both stages
        let mut state_a = crate::StageState::new();
        state_a.mark_complete();
        store.save_stage_state(item_id, "a", &state_a).await.unwrap();

        let mut state_b = crate::StageState::new();
        state_b.mark_complete();
        store.save_stage_state(item_id, "b", &state_b).await.unwrap();

        // Now complete and not blocked
        assert!(workflow.is_complete(item_id, &store).await.unwrap());
        assert!(!workflow.is_blocked(item_id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_advance_emits_events() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let mut receiver = workflow.subscribe();
        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        // Collect events
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }

        // Should have events for both stages plus workflow completion
        assert!(events.len() >= 4); // At least: Started(a), Completed(a), Started(b), Completed(b)

        // Check that we got events for both stages
        let stage_names: Vec<_> = events.iter().filter_map(|e| e.stage()).collect();
        assert!(stage_names.contains(&"a"));
        assert!(stage_names.contains(&"b"));

        // Should have a WorkflowCompleted event
        assert!(events
            .iter()
            .any(|e| matches!(e, crate::WorkflowEvent::WorkflowCompleted { .. })));
    }

    // Milestone 4.3 tests: Fan-out execution

    // Stage that fans out into 3 subtasks
    #[derive(Debug)]
    struct FanOutStage {
        name: String,
        subtask_count: std::sync::Arc<std::sync::atomic::AtomicU32>,
    }

    impl FanOutStage {
        fn new(name: impl Into<String>) -> Self {
            Self {
                name: name.into(),
                subtask_count: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)),
            }
        }
    }

    #[async_trait]
    impl crate::Stage for FanOutStage {
        fn name(&self) -> &str {
            &self.name
        }

        async fn execute(
            &self,
            _item: &dyn crate::WorkItem,
            ctx: &mut crate::StageContext,
        ) -> Result<crate::StageOutcome> {
            if ctx.subtask_name.is_some() {
                // Subtask execution
                self.subtask_count
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::StageOutcome::Complete)
            } else {
                // Initial invocation - declare subtasks
                Ok(crate::StageOutcome::FanOut(vec![
                    crate::SubTask::new("sub-1".to_string()),
                    crate::SubTask::new("sub-2".to_string()),
                    crate::SubTask::new("sub-3".to_string()),
                ]))
            }
        }
    }

    // Stage where one subtask fails
    #[derive(Debug)]
    struct PartialFailFanOutStage {
        name: String,
    }

    #[async_trait]
    impl crate::Stage for PartialFailFanOutStage {
        fn name(&self) -> &str {
            &self.name
        }

        async fn execute(
            &self,
            _item: &dyn crate::WorkItem,
            ctx: &mut crate::StageContext,
        ) -> Result<crate::StageOutcome> {
            match ctx.subtask_name.as_deref() {
                None => Ok(crate::StageOutcome::FanOut(vec![
                    crate::SubTask::new("ok-1".to_string()),
                    crate::SubTask::new("fail".to_string()),
                    crate::SubTask::new("ok-2".to_string()),
                ])),
                Some("fail") => Err(TreadleError::StageExecution("subtask failed".to_string())),
                Some(_) => Ok(crate::StageOutcome::Complete),
            }
        }
    }

    #[tokio::test]
    async fn test_fanout_all_subtasks_succeed() {
        let stage = FanOutStage::new("fanout");
        let counter = stage.subtask_count.clone();

        let workflow = Workflow::builder().stage("fanout", stage).build().unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        // All 3 subtasks should have been executed
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);

        // Stage should be complete
        let state = store
            .get_stage_state(&item.id, "fanout")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state.status, crate::StageStatus::Complete));

        // Workflow should be complete
        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_fanout_subtask_failure() {
        let workflow = Workflow::builder()
            .stage(
                "partial-fail",
                PartialFailFanOutStage {
                    name: "partial-fail".to_string(),
                },
            )
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        // Stage should have failed due to subtask failure
        let state = store
            .get_stage_state(&item.id, "partial-fail")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state.status, crate::StageStatus::Failed));
        assert!(state.error.is_some());

        // Workflow should be blocked
        assert!(workflow.is_blocked(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_fanout_emits_events() {
        let stage = FanOutStage::new("fanout");
        let workflow = Workflow::builder().stage("fanout", stage).build().unwrap();

        let mut receiver = workflow.subscribe();
        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        // Collect events
        let mut events = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            events.push(event);
        }

        // Should have FanOutStarted event
        assert!(events.iter().any(
            |e| matches!(e, crate::WorkflowEvent::FanOutStarted { subtasks, .. } if subtasks.len() == 3)
        ));

        // Should have SubTaskStarted events
        let subtask_started_count = events
            .iter()
            .filter(|e| matches!(e, crate::WorkflowEvent::SubTaskStarted { .. }))
            .count();
        assert_eq!(subtask_started_count, 3);

        // Should have SubTaskCompleted events
        let subtask_completed_count = events
            .iter()
            .filter(|e| matches!(e, crate::WorkflowEvent::SubTaskCompleted { .. }))
            .count();
        assert_eq!(subtask_completed_count, 3);

        // Should have StageCompleted for the parent stage
        assert!(events
            .iter()
            .any(|e| matches!(e, crate::WorkflowEvent::StageCompleted { stage, .. } if stage == "fanout")));
    }

    #[tokio::test]
    async fn test_fanout_in_pipeline() {
        let stage = FanOutStage::new("fanout");
        let counter = stage.subtask_count.clone();

        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("fanout", stage)
            .stage("c", TestStage::new("c"))
            .dependency("fanout", "a")
            .dependency("c", "fanout")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        // All 3 subtasks should have been executed
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 3);

        // All stages should be complete
        for stage_name in &["a", "fanout", "c"] {
            let state = store
                .get_stage_state(&item.id, stage_name)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(state.status, crate::StageStatus::Complete),
                "Stage {} should be complete",
                stage_name
            );
        }

        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    // Milestone 4.4 tests: Idempotency and edge cases

    #[tokio::test]
    async fn test_advance_idempotent_when_complete() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;

        #[derive(Debug)]
        struct CountingStage {
            name: String,
            counter: Arc<AtomicU32>,
        }

        #[async_trait]
        impl crate::Stage for CountingStage {
            fn name(&self) -> &str {
                &self.name
            }

            async fn execute(
                &self,
                _item: &dyn crate::WorkItem,
                _ctx: &mut crate::StageContext,
            ) -> Result<crate::StageOutcome> {
                self.counter.fetch_add(1, Ordering::SeqCst);
                Ok(crate::StageOutcome::Complete)
            }
        }

        let counter = Arc::new(AtomicU32::new(0));
        let workflow = Workflow::builder()
            .stage(
                "a",
                CountingStage {
                    name: "a".to_string(),
                    counter: counter.clone(),
                },
            )
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // First advance
        workflow.advance(&item, &mut store).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Second advance should be a no-op
        workflow.advance(&item, &mut store).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);

        // Third advance should still be a no-op
        workflow.advance(&item, &mut store).await.unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_advance_empty_workflow() {
        let workflow = Workflow::builder().build().unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // Should not error
        workflow.advance(&item, &mut store).await.unwrap();

        // Empty workflow is complete
        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_retry_failed_stage() {
        #[derive(Debug)]
        struct FailThenSucceedStage {
            name: String,
            attempts: Arc<AtomicU32>,
        }

        #[async_trait]
        impl crate::Stage for FailThenSucceedStage {
            fn name(&self) -> &str {
                &self.name
            }

            async fn execute(
                &self,
                _item: &dyn crate::WorkItem,
                _ctx: &mut crate::StageContext,
            ) -> Result<crate::StageOutcome> {
                let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    Err(TreadleError::StageExecution("first attempt fails".to_string()))
                } else {
                    Ok(crate::StageOutcome::Complete)
                }
            }
        }

        let attempts = Arc::new(AtomicU32::new(0));
        let workflow = Workflow::builder()
            .stage(
                "a",
                FailThenSucceedStage {
                    name: "a".to_string(),
                    attempts: attempts.clone(),
                },
            )
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // First advance should fail
        workflow.advance(&item, &mut store).await.unwrap();

        let state = store
            .get_stage_state(&item.id, "a")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state.status, crate::StageStatus::Failed));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);

        // Reset the stage
        workflow
            .retry_stage(&item.id, "a", &mut store)
            .await
            .unwrap();

        // Second advance should succeed
        workflow.advance(&item, &mut store).await.unwrap();

        let state = store
            .get_stage_state(&item.id, "a")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state.status, crate::StageStatus::Complete));
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(state.retry_count, 1); // Should have incremented retry count
    }

    #[tokio::test]
    async fn test_approve_review() {
        #[derive(Debug)]
        struct ReviewStage {
            name: String,
        }

        #[async_trait]
        impl crate::Stage for ReviewStage {
            fn name(&self) -> &str {
                &self.name
            }

            async fn execute(
                &self,
                _item: &dyn crate::WorkItem,
                _ctx: &mut crate::StageContext,
            ) -> Result<crate::StageOutcome> {
                Ok(crate::StageOutcome::NeedsReview)
            }
        }

        let workflow = Workflow::builder()
            .stage(
                "review",
                ReviewStage {
                    name: "review".to_string(),
                },
            )
            .stage("b", TestStage::new("b"))
            .dependency("b", "review")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // First advance should pause for review
        workflow.advance(&item, &mut store).await.unwrap();

        let state = store
            .get_stage_state(&item.id, "review")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state.status, crate::StageStatus::Paused));

        // Workflow should be blocked
        assert!(workflow.is_blocked(&item.id, &store).await.unwrap());

        // Approve the review
        workflow
            .approve_review(&item.id, "review", &mut store)
            .await
            .unwrap();

        // Second advance should complete stage b
        workflow.advance(&item, &mut store).await.unwrap();

        let state_b = store
            .get_stage_state(&item.id, "b")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state_b.status, crate::StageStatus::Complete));

        assert!(workflow.is_complete(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_reject_review() {
        #[derive(Debug)]
        struct ReviewStage {
            name: String,
        }

        #[async_trait]
        impl crate::Stage for ReviewStage {
            fn name(&self) -> &str {
                &self.name
            }

            async fn execute(
                &self,
                _item: &dyn crate::WorkItem,
                _ctx: &mut crate::StageContext,
            ) -> Result<crate::StageOutcome> {
                Ok(crate::StageOutcome::NeedsReview)
            }
        }

        let workflow = Workflow::builder()
            .stage(
                "review",
                ReviewStage {
                    name: "review".to_string(),
                },
            )
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // First advance should pause for review
        workflow.advance(&item, &mut store).await.unwrap();

        let state = store
            .get_stage_state(&item.id, "review")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state.status, crate::StageStatus::Paused));

        // Reject the review
        workflow
            .reject_review(&item.id, "review", "Not approved", &mut store)
            .await
            .unwrap();

        let state = store
            .get_stage_state(&item.id, "review")
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(state.status, crate::StageStatus::Failed));
        assert_eq!(state.error, Some("Not approved".to_string()));

        // Workflow should be blocked
        assert!(workflow.is_blocked(&item.id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_retry_stage_not_found() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();

        let result = workflow
            .retry_stage("item-1", "nonexistent", &mut store)
            .await;

        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    #[tokio::test]
    async fn test_approve_review_wrong_state() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // Execute stage normally (it completes)
        workflow.advance(&item, &mut store).await.unwrap();

        // Try to approve a completed stage
        let result = workflow
            .approve_review(&item.id, "a", &mut store)
            .await;

        assert!(matches!(result, Err(TreadleError::StageExecution(_))));
    }

    // Milestone 5.1 tests: Pipeline status

    #[tokio::test]
    async fn test_status_empty_workflow() {
        let workflow = Workflow::builder().build().unwrap();
        let store = crate::MemoryStateStore::new();

        let status = workflow.status("item-1", &store).await.unwrap();

        assert!(status.is_complete());
        assert_eq!(status.progress_percent(), 100.0);
        assert_eq!(status.stages.len(), 0);
    }

    #[tokio::test]
    async fn test_status_fresh_item() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        let store = crate::MemoryStateStore::new();

        let status = workflow.status("item-1", &store).await.unwrap();

        assert!(!status.is_complete());
        assert_eq!(status.progress_percent(), 0.0);
        assert_eq!(status.stages.len(), 3);

        // All stages should be pending
        for stage in &status.stages {
            assert!(matches!(stage.status, crate::StageStatus::Pending));
        }
    }

    #[tokio::test]
    async fn test_status_during_execution() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // Execute workflow
        workflow.advance(&item, &mut store).await.unwrap();

        // Check status after completion
        let status = workflow.status(&item.id, &store).await.unwrap();

        assert!(status.is_complete());
        assert_eq!(status.progress_percent(), 100.0);
        assert_eq!(status.failed_stages().len(), 0);
        assert_eq!(status.review_stages().len(), 0);

        // All stages should be complete
        for stage in &status.stages {
            assert!(matches!(stage.status, crate::StageStatus::Complete));
        }
    }

    #[tokio::test]
    async fn test_status_with_failure() {
        #[derive(Debug)]
        struct FailingStage {
            name: String,
        }

        #[async_trait]
        impl crate::Stage for FailingStage {
            fn name(&self) -> &str {
                &self.name
            }

            async fn execute(
                &self,
                _item: &dyn crate::WorkItem,
                _ctx: &mut crate::StageContext,
            ) -> Result<crate::StageOutcome> {
                Err(TreadleError::StageExecution("intentional failure".to_string()))
            }
        }

        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("fail", FailingStage {
                name: "fail".to_string(),
            })
            .stage("c", TestStage::new("c"))
            .dependency("fail", "a")
            .dependency("c", "fail")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // Execute workflow (will fail at "fail" stage)
        workflow.advance(&item, &mut store).await.unwrap();

        // Check status
        let status = workflow.status(&item.id, &store).await.unwrap();

        assert!(!status.is_complete());
        assert!(status.has_failures());
        assert_eq!(status.failed_stages(), vec!["fail"]);
        assert!(status.progress_percent() < 100.0);
    }

    #[tokio::test]
    async fn test_status_with_review() {
        #[derive(Debug)]
        struct ReviewStage {
            name: String,
        }

        #[async_trait]
        impl crate::Stage for ReviewStage {
            fn name(&self) -> &str {
                &self.name
            }

            async fn execute(
                &self,
                _item: &dyn crate::WorkItem,
                _ctx: &mut crate::StageContext,
            ) -> Result<crate::StageOutcome> {
                Ok(crate::StageOutcome::NeedsReview)
            }
        }

        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("review", ReviewStage {
                name: "review".to_string(),
            })
            .stage("c", TestStage::new("c"))
            .dependency("review", "a")
            .dependency("c", "review")
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "item-1".to_string(),
        };

        // Execute workflow (will pause at "review" stage)
        workflow.advance(&item, &mut store).await.unwrap();

        // Check status
        let status = workflow.status(&item.id, &store).await.unwrap();

        assert!(!status.is_complete());
        assert!(status.has_pending_reviews());
        assert_eq!(status.review_stages(), vec!["review"]);
        assert!(!status.has_failures());
    }

    #[tokio::test]
    async fn test_status_display_output() {
        let workflow = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .build()
            .unwrap();

        let mut store = crate::MemoryStateStore::new();
        let item = TestItem {
            id: "doc-123".to_string(),
        };

        workflow.advance(&item, &mut store).await.unwrap();

        let status = workflow.status(&item.id, &store).await.unwrap();
        let display = format!("{}", status);

        // Verify the display contains expected elements
        assert!(display.contains("doc-123"));
        assert!(display.contains("Pipeline status"));
        assert!(display.contains("Progress: 100%"));
        assert!(display.contains("Status: Complete"));
        assert!(display.contains("✅")); // Completion emoji
    }
}
