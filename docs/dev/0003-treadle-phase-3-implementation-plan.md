# Treadle Phase 3 Implementation Plan

## Phase 3: Workflow Builder and DAG

**Goal:** A `Workflow` struct backed by petgraph that validates the DAG and provides topological ordering.

**Prerequisites:**
- Phase 1 completed (all traits and types defined)
- Phase 2 completed (StateStore implementations working)
- `cargo build` and `cargo test` pass

---

## Milestone 3.1 — Workflow Struct and Builder: Stage Registration

### Files to Create/Modify
- `src/workflow.rs` (new)
- `src/lib.rs` (update exports)

### Implementation Details

#### 3.1.1 Create `src/workflow.rs`

```rust
//! Workflow definition and DAG management.
//!
//! This module provides [`Workflow`] and [`WorkflowBuilder`] for constructing
//! and managing workflow DAGs.

use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use std::sync::Arc;

use crate::{Result, Stage, TreadleError, WorkItem};

/// A registered stage in the workflow.
struct RegisteredStage<W: WorkItem> {
    name: String,
    stage: Arc<dyn Stage<W>>,
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
/// ```rust,ignore
/// use treadle::Workflow;
///
/// let workflow = Workflow::builder()
///     .stage("scan", ScanStage)
///     .stage("enrich", EnrichStage)
///     .stage("review", ReviewStage)
///     .dependency("enrich", "scan")
///     .dependency("review", "enrich")
///     .build()?;
/// ```
///
/// # Thread Safety
///
/// `Workflow` is `Send + Sync` and can be shared across async tasks.
pub struct Workflow<W: WorkItem> {
    /// The underlying directed graph.
    graph: DiGraph<RegisteredStage<W>, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Cached topological order of stage names.
    topo_order: Vec<String>,
}

impl<W: WorkItem> Workflow<W> {
    /// Creates a new workflow builder.
    pub fn builder() -> WorkflowBuilder<W> {
        WorkflowBuilder::new()
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
    /// # Errors
    ///
    /// Returns [`TreadleError::StageNotFound`] if the stage doesn't exist.
    pub(crate) fn get_stage(&self, name: &str) -> Result<&Arc<dyn Stage<W>>> {
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
}

// Implement Send + Sync (the Arc<dyn Stage<W>> inside is already Send + Sync)
unsafe impl<W: WorkItem> Send for Workflow<W> {}
unsafe impl<W: WorkItem> Sync for Workflow<W> {}

impl<W: WorkItem> std::fmt::Debug for Workflow<W> {
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
/// ```rust,ignore
/// let workflow = Workflow::builder()
///     .stage("a", StageA)
///     .stage("b", StageB)
///     .stage("c", StageC)
///     .dependency("b", "a")  // b depends on a
///     .dependency("c", "b")  // c depends on b
///     .build()?;
/// ```
pub struct WorkflowBuilder<W: WorkItem> {
    /// The graph being built.
    graph: DiGraph<RegisteredStage<W>, ()>,
    /// Mapping from stage name to node index.
    name_to_index: HashMap<String, NodeIndex>,
    /// Deferred dependencies to be resolved at build time.
    deferred_deps: Vec<DeferredDependency>,
}

impl<W: WorkItem> WorkflowBuilder<W> {
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
    pub fn stage(mut self, name: impl Into<String>, stage: impl Stage<W> + 'static) -> Self {
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
        stage: impl Stage<W> + 'static,
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
    pub fn build(mut self) -> Result<Workflow<W>> {
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

        Ok(Workflow {
            graph: self.graph,
            name_to_index: self.name_to_index,
            topo_order,
        })
    }
}

impl<W: WorkItem> Default for WorkflowBuilder<W> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::test_helpers::TestItem;
    use crate::{StageContext, StageOutcome};
    use async_trait::async_trait;

    // Simple test stage that always completes
    struct TestStage {
        name: String,
    }

    impl TestStage {
        fn new(name: impl Into<String>) -> Self {
            Self { name: name.into() }
        }
    }

    #[async_trait]
    impl Stage<TestItem> for TestStage {
        fn name(&self) -> &str {
            &self.name
        }

        async fn execute(&self, _item: &TestItem, _ctx: &StageContext) -> Result<StageOutcome> {
            Ok(StageOutcome::Completed)
        }
    }

    #[test]
    fn test_empty_workflow() {
        let workflow: Workflow<TestItem> = Workflow::builder().build().unwrap();
        assert_eq!(workflow.stage_count(), 0);
        assert!(workflow.stages().is_empty());
    }

    #[test]
    fn test_single_stage() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .build()
            .unwrap();

        assert_eq!(workflow.stage_count(), 1);
        assert!(workflow.has_stage("scan"));
        assert!(!workflow.has_stage("other"));
    }

    #[test]
    fn test_multiple_stages() {
        let workflow: Workflow<TestItem> = Workflow::builder()
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
        let _: Workflow<TestItem> = Workflow::builder()
            .stage("scan", TestStage::new("scan"))
            .stage("scan", TestStage::new("scan")) // Duplicate!
            .build()
            .unwrap();
    }

    #[test]
    fn test_try_stage_duplicate_returns_error() {
        let result: Result<Workflow<TestItem>> = Workflow::builder()
            .try_stage("scan", TestStage::new("scan"))
            .and_then(|b| b.try_stage("scan", TestStage::new("scan")))
            .and_then(|b| b.build());

        assert!(matches!(result, Err(TreadleError::DuplicateStage(_))));
    }

    #[test]
    fn test_linear_pipeline_topo_order() {
        let workflow: Workflow<TestItem> = Workflow::builder()
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
        let workflow: Workflow<TestItem> = Workflow::builder()
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
        let workflow: Workflow<TestItem> = Workflow::builder()
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
        let workflow: Workflow<TestItem> = Workflow::builder()
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
        let result: Result<Workflow<TestItem>> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .dependency("b", "a") // b doesn't exist
            .build();

        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    #[test]
    fn test_stage_not_found_in_depends_on() {
        let result: Result<Workflow<TestItem>> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .dependency("a", "b") // b doesn't exist
            .build();

        assert!(matches!(result, Err(TreadleError::StageNotFound(_))));
    }

    #[test]
    fn test_stage_not_found_query() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        assert!(matches!(
            workflow.dependencies("nonexistent"),
            Err(TreadleError::StageNotFound(_))
        ));
    }
}
```

**Design Decisions:**
- `Arc<dyn Stage<W>>` for shared stage ownership (allows cloning workflows if needed)
- Deferred dependency resolution (all dependencies resolved at build time)
- Panic on duplicate in `stage()`, Result in `try_stage()` (ergonomic vs explicit)
- Topological order cached at build time
- `pub(crate)` for `get_stage` (internal use by executor)

#### 3.1.2 Update `src/lib.rs`

```rust
mod error;
mod stage;
mod state_store;
mod work_item;
mod workflow;

// ... existing exports ...

pub use workflow::{Workflow, WorkflowBuilder};
```

### Verification Commands
```bash
cargo build
cargo test workflow::tests
cargo clippy
```

---

## Milestone 3.2 — Workflow Builder: Dependencies and DAG Validation

This milestone is covered by the implementation in 3.1, which already includes:
- Dependency declaration with `dependency(stage, depends_on)`
- Cycle detection with `is_cyclic_directed`
- Stage existence validation
- Topological sort with `toposort`

### Additional Tests

Add to `src/workflow.rs` tests:

```rust
    #[test]
    fn test_diamond_dag() {
        // Diamond: A → B, A → C, B → D, C → D
        let workflow: Workflow<TestItem> = Workflow::builder()
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
        let result: Result<Workflow<TestItem>> = Workflow::builder()
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
        let result: Result<Workflow<TestItem>> = Workflow::builder()
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
        let result: Result<Workflow<TestItem>> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .dependency("a", "a") // Self-cycle
            .build();

        assert!(matches!(result, Err(TreadleError::DagCycle)));
    }

    #[test]
    fn test_multiple_roots() {
        // Two independent chains: A → B, C → D
        let workflow: Workflow<TestItem> = Workflow::builder()
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
        let workflow: Workflow<TestItem> = Workflow::builder()
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
```

### Verification Commands
```bash
cargo test workflow::tests
cargo clippy
```

---

## Milestone 3.3 — Workflow: Ready-Stage Resolution

### Files to Modify
- `src/workflow.rs` (add ready_stages method)

### Implementation Details

#### 3.3.1 Add Ready-Stage Resolution

Add to `src/workflow.rs`:

```rust
use crate::{StageState, StateStore};

impl<W: WorkItem> Workflow<W> {
    /// Returns stages that are ready to execute for the given work item.
    ///
    /// A stage is ready when:
    /// - All its dependencies are `Completed` or `Skipped`
    /// - Its own status is `Pending` or not yet recorded
    ///
    /// This is the core scheduling logic that determines what can run next.
    ///
    /// # Arguments
    ///
    /// * `item_id` - The work item's identifier
    /// * `store` - The state store to query
    ///
    /// # Returns
    ///
    /// Stage names that are ready to execute, in topological order.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ready = workflow.ready_stages(&item.id(), &store).await?;
    /// for stage_name in ready {
    ///     // Execute stage...
    /// }
    /// ```
    pub async fn ready_stages<S: StateStore<W>>(
        &self,
        item_id: &W::Id,
        store: &S,
    ) -> Result<Vec<String>> {
        let all_statuses = store.get_all_statuses(item_id).await?;
        let mut ready = Vec::new();

        for stage_name in &self.topo_order {
            // Get the stage's current status
            let status = all_statuses.get(stage_name);

            // Skip if already running, completed, failed, or awaiting review
            match status {
                Some(s) => match s.state {
                    StageState::Pending => {
                        // Check dependencies
                    }
                    StageState::Running
                    | StageState::Completed
                    | StageState::Failed
                    | StageState::AwaitingReview
                    | StageState::Skipped => {
                        continue;
                    }
                },
                None => {
                    // No status recorded yet, treat as pending
                }
            }

            // Check if all dependencies are satisfied
            let deps = self.dependencies(stage_name)?;
            let all_deps_satisfied = deps.iter().all(|dep_name| {
                all_statuses
                    .get(*dep_name)
                    .map(|s| s.state == StageState::Completed || s.state == StageState::Skipped)
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
    /// Complete means all stages are either `Completed` or `Skipped`.
    pub async fn is_complete<S: StateStore<W>>(
        &self,
        item_id: &W::Id,
        store: &S,
    ) -> Result<bool> {
        let all_statuses = store.get_all_statuses(item_id).await?;

        // All stages must have a completed or skipped status
        for stage_name in &self.topo_order {
            match all_statuses.get(stage_name) {
                Some(s) if s.state == StageState::Completed || s.state == StageState::Skipped => {
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
    /// is not complete (something is awaiting review or failed).
    pub async fn is_blocked<S: StateStore<W>>(
        &self,
        item_id: &W::Id,
        store: &S,
    ) -> Result<bool> {
        if self.is_complete(item_id, store).await? {
            return Ok(false);
        }

        let ready = self.ready_stages(item_id, store).await?;
        Ok(ready.is_empty())
    }
}
```

#### 3.3.2 Add Tests for Ready-Stage Resolution

Add to `src/workflow.rs` tests:

```rust
mod ready_stage_tests {
    use super::*;
    use crate::{MemoryStateStore, StageStatus};

    type TestStore = MemoryStateStore<TestItem>;

    #[tokio::test]
    async fn test_fresh_item_root_stages_ready() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Fresh item: only root stage is ready
        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert_eq!(ready, vec!["a"]);
    }

    #[tokio::test]
    async fn test_after_root_completion() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .dependency("b", "a")
            .dependency("c", "b")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Complete stage a
        store
            .set_status(&item_id, "a", StageStatus::completed())
            .await
            .unwrap();

        // Now b should be ready
        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert_eq!(ready, vec!["b"]);
    }

    #[tokio::test]
    async fn test_diamond_convergence() {
        // Diamond: A → B, A → C, B → D, C → D
        let workflow: Workflow<TestItem> = Workflow::builder()
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

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Complete a: both b and c should be ready
        store
            .set_status(&item_id, "a", StageStatus::completed())
            .await
            .unwrap();

        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert_eq!(ready.len(), 2);
        assert!(ready.contains(&"b".to_string()));
        assert!(ready.contains(&"c".to_string()));

        // Complete only b: d is not yet ready (needs c too)
        store
            .set_status(&item_id, "b", StageStatus::completed())
            .await
            .unwrap();

        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert_eq!(ready, vec!["c"]);

        // Complete c: d is now ready
        store
            .set_status(&item_id, "c", StageStatus::completed())
            .await
            .unwrap();

        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert_eq!(ready, vec!["d"]);
    }

    #[tokio::test]
    async fn test_running_stage_not_ready() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Stage is running
        store
            .set_status(&item_id, "a", StageStatus::running())
            .await
            .unwrap();

        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert!(ready.is_empty());
    }

    #[tokio::test]
    async fn test_completed_stage_not_ready() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        store
            .set_status(&item_id, "a", StageStatus::completed())
            .await
            .unwrap();

        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert!(ready.is_empty());
    }

    #[tokio::test]
    async fn test_awaiting_review_not_ready() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Stage a is awaiting review - blocks b
        store
            .set_status(
                &item_id,
                "a",
                StageStatus::awaiting_review(crate::ReviewData::new("check")),
            )
            .await
            .unwrap();

        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert!(ready.is_empty());
    }

    #[tokio::test]
    async fn test_skipped_dependency_satisfies() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Stage a was skipped - should still allow b
        store
            .set_status(&item_id, "a", StageStatus::skipped())
            .await
            .unwrap();

        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert_eq!(ready, vec!["b"]);
    }

    #[tokio::test]
    async fn test_failed_dependency_blocks() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Stage a failed - should block b
        store
            .set_status(&item_id, "a", StageStatus::failed("error"))
            .await
            .unwrap();

        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert!(ready.is_empty());
    }

    #[tokio::test]
    async fn test_is_complete() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Not complete initially
        assert!(!workflow.is_complete(&item_id, &store).await.unwrap());

        // Complete a
        store
            .set_status(&item_id, "a", StageStatus::completed())
            .await
            .unwrap();
        assert!(!workflow.is_complete(&item_id, &store).await.unwrap());

        // Complete b
        store
            .set_status(&item_id, "b", StageStatus::completed())
            .await
            .unwrap();
        assert!(workflow.is_complete(&item_id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_is_complete_with_skipped() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        store
            .set_status(&item_id, "a", StageStatus::skipped())
            .await
            .unwrap();
        store
            .set_status(&item_id, "b", StageStatus::completed())
            .await
            .unwrap();

        assert!(workflow.is_complete(&item_id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_is_blocked() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Not blocked initially (a is ready)
        assert!(!workflow.is_blocked(&item_id, &store).await.unwrap());

        // Blocked when a is awaiting review
        store
            .set_status(
                &item_id,
                "a",
                StageStatus::awaiting_review(crate::ReviewData::new("check")),
            )
            .await
            .unwrap();
        assert!(workflow.is_blocked(&item_id, &store).await.unwrap());

        // Not blocked when a is completed
        store
            .set_status(&item_id, "a", StageStatus::completed())
            .await
            .unwrap();
        assert!(!workflow.is_blocked(&item_id, &store).await.unwrap());

        // Not blocked when complete
        store
            .set_status(&item_id, "b", StageStatus::completed())
            .await
            .unwrap();
        assert!(!workflow.is_blocked(&item_id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_is_blocked_on_failure() {
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .dependency("b", "a")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        store
            .set_status(&item_id, "a", StageStatus::failed("error"))
            .await
            .unwrap();

        assert!(workflow.is_blocked(&item_id, &store).await.unwrap());
    }

    #[tokio::test]
    async fn test_multiple_independent_roots() {
        // Two independent chains
        let workflow: Workflow<TestItem> = Workflow::builder()
            .stage("a", TestStage::new("a"))
            .stage("b", TestStage::new("b"))
            .stage("c", TestStage::new("c"))
            .stage("d", TestStage::new("d"))
            .dependency("b", "a")
            .dependency("d", "c")
            .build()
            .unwrap();

        let store = TestStore::new();
        let item_id = "item-1".to_string();

        // Both roots should be ready
        let ready = workflow.ready_stages(&item_id, &store).await.unwrap();
        assert_eq!(ready.len(), 2);
        assert!(ready.contains(&"a".to_string()));
        assert!(ready.contains(&"c".to_string()));
    }
}
```

### Verification Commands
```bash
cargo test workflow::ready_stage_tests
cargo test workflow
cargo clippy
```

---

## Phase 3 Completion Checklist

- [ ] `cargo build` succeeds
- [ ] `cargo test workflow` passes all tests
- [ ] `cargo clippy -- -D warnings` clean
- [ ] All public types have rustdoc comments

### Public API After Phase 3

```rust
// Previously from Phases 1-2
treadle::TreadleError
treadle::Result<T>
treadle::WorkItem
treadle::Stage
treadle::StageContext
treadle::StageState
treadle::StageStatus
treadle::StageOutcome
treadle::SubTask
treadle::SubTaskStatus
treadle::ReviewData
treadle::StateStore
treadle::MemoryStateStore
treadle::SqliteStateStore  // (feature = "sqlite")

// New in Phase 3
treadle::Workflow
treadle::WorkflowBuilder
```

---

## Architecture After Phase 3

```
src/
├── lib.rs                  # Crate root, re-exports
├── error.rs                # TreadleError, Result
├── work_item.rs            # WorkItem trait
├── stage.rs                # Stage types and trait
├── state_store/
│   ├── mod.rs              # StateStore trait, re-exports
│   ├── memory.rs           # MemoryStateStore
│   └── sqlite.rs           # SqliteStateStore
└── workflow.rs             # Workflow, WorkflowBuilder, ready_stages
```
