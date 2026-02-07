//! Pipeline status reporting and visualization.
//!
//! This module provides [`PipelineStatus`] for inspecting the current
//! state of a workflow for a work item.

use std::fmt;

use crate::{StageState, StageStatus, SubTask};
use chrono::{DateTime, Utc};

/// Status entry for a single stage within a pipeline.
#[derive(Debug, Clone)]
pub struct StageStatusEntry {
    /// The stage name.
    pub name: String,
    /// The current status of this stage.
    pub status: StageStatus,
    /// When this stage was first entered.
    pub started_at: Option<DateTime<Utc>>,
    /// When this stage completed.
    pub completed_at: Option<DateTime<Utc>>,
    /// Number of retry attempts for this stage.
    pub retry_count: u32,
    /// Error message if the stage failed.
    pub error: Option<String>,
    /// Subtasks for fan-out stages.
    pub subtasks: Vec<SubTask>,
}

impl StageStatusEntry {
    /// Creates a new pending status entry.
    pub fn pending(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: StageStatus::Pending,
            started_at: None,
            completed_at: None,
            retry_count: 0,
            error: None,
            subtasks: Vec::new(),
        }
    }

    /// Creates a status entry from a stored stage state.
    pub fn from_stage_state(name: impl Into<String>, state: &StageState) -> Self {
        Self {
            name: name.into(),
            status: state.status.clone(),
            started_at: state.started_at,
            completed_at: state.completed_at,
            retry_count: state.retry_count,
            error: state.error.clone(),
            subtasks: state.subtasks.clone(),
        }
    }

    /// Returns a status indicator character.
    pub fn status_char(&self) -> char {
        match self.status {
            StageStatus::Pending => '⏳',     // Hourglass
            StageStatus::InProgress => '🔄', // Spinning arrows
            StageStatus::Complete => '✅',    // Green check
            StageStatus::Failed => '❌',      // Red X
            StageStatus::Paused => '👀',     // Eyes (awaiting review)
        }
    }

    /// Returns a status indicator for subtasks.
    pub fn subtask_status_char(status: &StageStatus) -> char {
        match status {
            StageStatus::Pending => '⏳',
            StageStatus::InProgress => '🔄',
            StageStatus::Complete => '✅',
            StageStatus::Failed => '❌',
            StageStatus::Paused => '👀',
        }
    }
}

/// The complete status of a workflow pipeline for a work item.
///
/// This provides a snapshot of where a work item is in the workflow,
/// including the status of all stages and their subtasks.
#[derive(Debug, Clone)]
pub struct PipelineStatus {
    /// The work item's identifier.
    pub item_id: String,
    /// Status of each stage in topological order.
    pub stages: Vec<StageStatusEntry>,
}

impl PipelineStatus {
    /// Creates a new pipeline status.
    pub fn new(item_id: impl Into<String>, stages: Vec<StageStatusEntry>) -> Self {
        Self {
            item_id: item_id.into(),
            stages,
        }
    }

    /// Returns true if all stages are completed.
    pub fn is_complete(&self) -> bool {
        self.stages
            .iter()
            .all(|s| matches!(s.status, StageStatus::Complete))
    }

    /// Returns true if any stage has failed.
    pub fn has_failures(&self) -> bool {
        self.stages
            .iter()
            .any(|s| matches!(s.status, StageStatus::Failed))
    }

    /// Returns true if any stage is paused (awaiting review).
    pub fn has_pending_reviews(&self) -> bool {
        self.stages
            .iter()
            .any(|s| matches!(s.status, StageStatus::Paused))
    }

    /// Returns the names of stages that are currently running.
    pub fn running_stages(&self) -> Vec<&str> {
        self.stages
            .iter()
            .filter(|s| matches!(s.status, StageStatus::InProgress))
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Returns the names of stages that have failed.
    pub fn failed_stages(&self) -> Vec<&str> {
        self.stages
            .iter()
            .filter(|s| matches!(s.status, StageStatus::Failed))
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Returns the names of stages awaiting review.
    pub fn review_stages(&self) -> Vec<&str> {
        self.stages
            .iter()
            .filter(|s| matches!(s.status, StageStatus::Paused))
            .map(|s| s.name.as_str())
            .collect()
    }

    /// Returns the overall progress as a percentage.
    pub fn progress_percent(&self) -> f32 {
        if self.stages.is_empty() {
            return 100.0;
        }

        let completed = self
            .stages
            .iter()
            .filter(|s| matches!(s.status, StageStatus::Complete))
            .count();

        (completed as f32 / self.stages.len() as f32) * 100.0
    }
}

impl fmt::Display for PipelineStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Pipeline status for item \"{}\":", self.item_id)?;
        writeln!(f)?;

        for stage in &self.stages {
            // Main stage line
            let status_char = stage.status_char();
            let status_str = format!("{:?}", stage.status);
            let time_str = stage
                .completed_at
                .or(stage.started_at)
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| "-".to_string());

            write!(
                f,
                "  {} {:<15} {:<15} {}",
                status_char, stage.name, status_str, time_str
            )?;

            // Error message if present
            if let Some(ref error) = stage.error {
                write!(f, "  Error: {}", error)?;
            }

            // Retry count if > 0
            if stage.retry_count > 0 {
                write!(f, "  (retry {})", stage.retry_count)?;
            }

            writeln!(f)?;

            // Subtasks if present
            for subtask in &stage.subtasks {
                let sub_char = StageStatusEntry::subtask_status_char(&subtask.status);
                let sub_status = format!("{:?}", subtask.status);

                write!(
                    f,
                    "     └─ {} {:<12} {}",
                    sub_char, subtask.id, sub_status
                )?;

                if let Some(ref error) = subtask.error {
                    write!(f, "  Error: {}", error)?;
                }

                writeln!(f)?;
            }
        }

        // Summary line
        writeln!(f)?;
        writeln!(f, "Progress: {:.0}%", self.progress_percent())?;

        if self.is_complete() {
            writeln!(f, "Status: Complete")?;
        } else if self.has_failures() {
            writeln!(
                f,
                "Status: Failed ({} stage(s))",
                self.failed_stages().len()
            )?;
        } else if self.has_pending_reviews() {
            writeln!(
                f,
                "Status: Awaiting review ({} stage(s))",
                self.review_stages().len()
            )?;
        } else {
            writeln!(f, "Status: In progress")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_status_empty() {
        let status = PipelineStatus::new("item-1", vec![]);
        assert!(status.is_complete());
        assert!(!status.has_failures());
        assert_eq!(status.progress_percent(), 100.0);
    }

    #[test]
    fn test_pipeline_status_all_pending() {
        let stages = vec![
            StageStatusEntry::pending("a"),
            StageStatusEntry::pending("b"),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(!status.is_complete());
        assert_eq!(status.progress_percent(), 0.0);
    }

    #[test]
    fn test_pipeline_status_partial_complete() {
        let mut state_a = StageState::new();
        state_a.mark_complete();

        let stages = vec![
            StageStatusEntry::from_stage_state("a", &state_a),
            StageStatusEntry::pending("b"),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(!status.is_complete());
        assert!((status.progress_percent() - 33.33).abs() < 1.0);
    }

    #[test]
    fn test_pipeline_status_complete() {
        let mut state_a = StageState::new();
        state_a.mark_complete();

        let mut state_b = StageState::new();
        state_b.mark_complete();

        let stages = vec![
            StageStatusEntry::from_stage_state("a", &state_a),
            StageStatusEntry::from_stage_state("b", &state_b),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(status.is_complete());
        assert_eq!(status.progress_percent(), 100.0);
    }

    #[test]
    fn test_pipeline_status_with_failure() {
        let mut state_a = StageState::new();
        state_a.mark_complete();

        let mut state_b = StageState::new();
        state_b.mark_failed("oops".to_string());

        let stages = vec![
            StageStatusEntry::from_stage_state("a", &state_a),
            StageStatusEntry::from_stage_state("b", &state_b),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(!status.is_complete());
        assert!(status.has_failures());
        assert_eq!(status.failed_stages(), vec!["b"]);
    }

    #[test]
    fn test_pipeline_status_with_review() {
        let mut state_a = StageState::new();
        state_a.mark_complete();

        let mut state_b = StageState::new();
        state_b.mark_paused();

        let stages = vec![
            StageStatusEntry::from_stage_state("a", &state_a),
            StageStatusEntry::from_stage_state("b", &state_b),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        assert!(status.has_pending_reviews());
        assert_eq!(status.review_stages(), vec!["b"]);
    }

    #[test]
    fn test_pipeline_status_display() {
        let mut state_scan = StageState::new();
        state_scan.mark_complete();

        let mut state_enrich = StageState::new();
        state_enrich.mark_in_progress();

        let stages = vec![
            StageStatusEntry::from_stage_state("scan", &state_scan),
            StageStatusEntry::from_stage_state("enrich", &state_enrich),
            StageStatusEntry::pending("review"),
        ];
        let status = PipelineStatus::new("doc-1", stages);

        let display = format!("{}", status);
        assert!(display.contains("doc-1"));
        assert!(display.contains("scan"));
        assert!(display.contains("enrich"));
        assert!(display.contains("review"));
        assert!(display.contains("In progress"));
    }

    #[test]
    fn test_stage_status_chars() {
        assert_eq!(StageStatusEntry::pending("a").status_char(), '⏳');

        let mut completed = StageState::new();
        completed.mark_complete();
        let completed_entry = StageStatusEntry::from_stage_state("a", &completed);
        assert_eq!(completed_entry.status_char(), '✅');

        let mut failed = StageState::new();
        failed.mark_failed("err".to_string());
        let failed_entry = StageStatusEntry::from_stage_state("a", &failed);
        assert_eq!(failed_entry.status_char(), '❌');
    }

    #[test]
    fn test_progress_percent_calculation() {
        let mut state_a = StageState::new();
        state_a.mark_complete();

        let mut state_b = StageState::new();
        state_b.mark_complete();

        let stages = vec![
            StageStatusEntry::from_stage_state("a", &state_a),
            StageStatusEntry::from_stage_state("b", &state_b),
            StageStatusEntry::pending("c"),
            StageStatusEntry::pending("d"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        // 2 out of 4 = 50%
        assert_eq!(status.progress_percent(), 50.0);
    }

    #[test]
    fn test_running_stages() {
        let mut state_a = StageState::new();
        state_a.mark_in_progress();

        let mut state_b = StageState::new();
        state_b.mark_in_progress();

        let stages = vec![
            StageStatusEntry::from_stage_state("a", &state_a),
            StageStatusEntry::from_stage_state("b", &state_b),
            StageStatusEntry::pending("c"),
        ];
        let status = PipelineStatus::new("item-1", stages);

        let running = status.running_stages();
        assert_eq!(running.len(), 2);
        assert!(running.contains(&"a"));
        assert!(running.contains(&"b"));
    }
}
