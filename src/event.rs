//! Workflow execution events.
//!
//! This module provides [`WorkflowEvent`] for observing workflow execution.
//! Events are broadcast through a channel that can be subscribed to for
//! monitoring, logging, or building UIs.

use crate::ReviewData;

/// An event emitted during workflow execution.
///
/// Events use `String` for item IDs to keep the event type simple
/// and easy to serialize for logging or transmission.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum WorkflowEvent {
    /// A stage has started executing.
    StageStarted {
        /// The work item's identifier.
        item_id: String,
        /// The stage name.
        stage: String,
    },

    /// A stage completed successfully.
    StageCompleted {
        /// The work item's identifier.
        item_id: String,
        /// The stage name.
        stage: String,
    },

    /// A stage execution failed.
    StageFailed {
        /// The work item's identifier.
        item_id: String,
        /// The stage name.
        stage: String,
        /// Error message describing the failure.
        error: String,
    },

    /// A stage requires human review before proceeding.
    ReviewRequired {
        /// The work item's identifier.
        item_id: String,
        /// The stage name.
        stage: String,
        /// Data for the reviewer.
        data: ReviewData,
    },

    /// A stage was skipped.
    StageSkipped {
        /// The work item's identifier.
        item_id: String,
        /// The stage name.
        stage: String,
    },

    /// A stage was retried after failure.
    StageRetried {
        /// The work item's identifier.
        item_id: String,
        /// The stage name.
        stage: String,
    },

    /// A fan-out stage started executing subtasks.
    FanOutStarted {
        /// The work item's identifier.
        item_id: String,
        /// The stage name.
        stage: String,
        /// The subtask names.
        subtasks: Vec<String>,
    },

    /// A subtask within a fan-out stage started.
    SubTaskStarted {
        /// The work item's identifier.
        item_id: String,
        /// The parent stage name.
        stage: String,
        /// The subtask name.
        subtask: String,
    },

    /// A subtask within a fan-out stage completed.
    SubTaskCompleted {
        /// The work item's identifier.
        item_id: String,
        /// The parent stage name.
        stage: String,
        /// The subtask name.
        subtask: String,
    },

    /// A subtask within a fan-out stage failed.
    SubTaskFailed {
        /// The work item's identifier.
        item_id: String,
        /// The parent stage name.
        stage: String,
        /// The subtask name.
        subtask: String,
        /// Error message describing the failure.
        error: String,
    },

    /// The workflow completed for a work item (all stages done).
    WorkflowCompleted {
        /// The work item's identifier.
        item_id: String,
    },
}

impl WorkflowEvent {
    /// Returns the item ID for this event.
    pub fn item_id(&self) -> &str {
        match self {
            Self::StageStarted { item_id, .. }
            | Self::StageCompleted { item_id, .. }
            | Self::StageFailed { item_id, .. }
            | Self::ReviewRequired { item_id, .. }
            | Self::StageSkipped { item_id, .. }
            | Self::StageRetried { item_id, .. }
            | Self::FanOutStarted { item_id, .. }
            | Self::SubTaskStarted { item_id, .. }
            | Self::SubTaskCompleted { item_id, .. }
            | Self::SubTaskFailed { item_id, .. }
            | Self::WorkflowCompleted { item_id } => item_id,
        }
    }

    /// Returns the stage name for this event, if applicable.
    pub fn stage(&self) -> Option<&str> {
        match self {
            Self::StageStarted { stage, .. }
            | Self::StageCompleted { stage, .. }
            | Self::StageFailed { stage, .. }
            | Self::ReviewRequired { stage, .. }
            | Self::StageSkipped { stage, .. }
            | Self::StageRetried { stage, .. }
            | Self::FanOutStarted { stage, .. }
            | Self::SubTaskStarted { stage, .. }
            | Self::SubTaskCompleted { stage, .. }
            | Self::SubTaskFailed { stage, .. } => Some(stage),
            Self::WorkflowCompleted { .. } => None,
        }
    }

    /// Returns true if this is an error event.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::StageFailed { .. } | Self::SubTaskFailed { .. })
    }

    /// Returns true if this is a completion event.
    pub fn is_completion(&self) -> bool {
        matches!(
            self,
            Self::StageCompleted { .. } | Self::SubTaskCompleted { .. } | Self::WorkflowCompleted { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_item_id() {
        let event = WorkflowEvent::StageStarted {
            item_id: "item-1".to_string(),
            stage: "scan".to_string(),
        };
        assert_eq!(event.item_id(), "item-1");
    }

    #[test]
    fn test_event_stage() {
        let event = WorkflowEvent::StageCompleted {
            item_id: "item-1".to_string(),
            stage: "scan".to_string(),
        };
        assert_eq!(event.stage(), Some("scan"));

        let event = WorkflowEvent::WorkflowCompleted {
            item_id: "item-1".to_string(),
        };
        assert_eq!(event.stage(), None);
    }

    #[test]
    fn test_is_error() {
        let success = WorkflowEvent::StageCompleted {
            item_id: "x".to_string(),
            stage: "s".to_string(),
        };
        assert!(!success.is_error());

        let failure = WorkflowEvent::StageFailed {
            item_id: "x".to_string(),
            stage: "s".to_string(),
            error: "err".to_string(),
        };
        assert!(failure.is_error());
    }

    #[test]
    fn test_is_completion() {
        let stage_complete = WorkflowEvent::StageCompleted {
            item_id: "x".to_string(),
            stage: "s".to_string(),
        };
        assert!(stage_complete.is_completion());

        let workflow_complete = WorkflowEvent::WorkflowCompleted {
            item_id: "x".to_string(),
        };
        assert!(workflow_complete.is_completion());

        let started = WorkflowEvent::StageStarted {
            item_id: "x".to_string(),
            stage: "s".to_string(),
        };
        assert!(!started.is_completion());
    }

    #[test]
    fn test_event_clone() {
        let event = WorkflowEvent::StageStarted {
            item_id: "item-1".to_string(),
            stage: "scan".to_string(),
        };
        let cloned = event.clone();
        assert_eq!(event.item_id(), cloned.item_id());
        assert_eq!(event.stage(), cloned.stage());
    }

    #[test]
    fn test_review_required_event() {
        let review_data = ReviewData::new(
            "item-1".to_string(),
            "review-stage".to_string(),
            "Please check this".to_string(),
        );
        let event = WorkflowEvent::ReviewRequired {
            item_id: "item-1".to_string(),
            stage: "review-stage".to_string(),
            data: review_data,
        };

        assert_eq!(event.item_id(), "item-1");
        assert_eq!(event.stage(), Some("review-stage"));
        assert!(!event.is_error());
    }

    #[test]
    fn test_stage_retried_event() {
        let event = WorkflowEvent::StageRetried {
            item_id: "item-1".to_string(),
            stage: "scan".to_string(),
        };

        assert_eq!(event.item_id(), "item-1");
        assert_eq!(event.stage(), Some("scan"));
        assert!(!event.is_error());
    }
}
