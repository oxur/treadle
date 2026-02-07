//! Workflow definition and DAG management.
//!
//! This module provides [`Workflow`] and [`WorkflowBuilder`] for constructing
//! and managing workflow DAGs backed by petgraph.

use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashMap;
use std::sync::Arc;

use crate::{Result, Stage, TreadleError};

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
}

impl Workflow {
    /// Creates a new workflow builder.
    pub fn builder() -> WorkflowBuilder {
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

        Ok(Workflow {
            graph: self.graph,
            name_to_index: self.name_to_index,
            topo_order,
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
}
