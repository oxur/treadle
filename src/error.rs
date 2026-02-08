//! Error types for Treadle workflow engine.
//!
//! This module defines the error types used throughout the Treadle crate,
//! following the non-exhaustive enum pattern to allow future error variants
//! without breaking compatibility.

use thiserror::Error;

/// The main error type for Treadle operations.
///
/// This enum uses `#[non_exhaustive]` to allow adding new error variants
/// in the future without breaking backward compatibility.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TreadleError {
    /// Error occurred in the state store layer.
    #[error("State store error: {0}")]
    StateStore(String),

    /// Error occurred during stage execution.
    #[error("Stage execution error: {0}")]
    StageExecution(String),

    /// Workflow configuration or structure is invalid.
    #[error("Invalid workflow: {0}")]
    InvalidWorkflow(String),

    /// Work item not found in state store.
    #[error("Work item not found: {0}")]
    WorkItemNotFound(String),

    /// Stage not found in workflow.
    #[error("Stage not found: {0}")]
    StageNotFound(String),

    /// Duplicate stage name in workflow.
    #[error("Duplicate stage name: {0}")]
    DuplicateStage(String),

    /// Cycle detected in workflow DAG.
    #[error("Cycle detected in workflow DAG")]
    DagCycle,

    /// Serialization or deserialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// I/O error from file or database operations.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Database error (for SQLite state store).
    #[cfg(feature = "sqlite")]
    #[error("Database error: {0}")]
    Database(String),
}

/// A specialized `Result` type for Treadle operations.
///
/// This is a type alias for `std::result::Result<T, TreadleError>` to reduce
/// boilerplate in function signatures throughout the crate.
pub type Result<T> = std::result::Result<T, TreadleError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_state_store() {
        let error = TreadleError::StateStore("connection failed".to_string());
        assert_eq!(error.to_string(), "State store error: connection failed");
    }

    #[test]
    fn test_error_display_stage_execution() {
        let error = TreadleError::StageExecution("timeout".to_string());
        assert_eq!(error.to_string(), "Stage execution error: timeout");
    }

    #[test]
    fn test_error_display_invalid_workflow() {
        let error = TreadleError::InvalidWorkflow("cycle detected".to_string());
        assert_eq!(error.to_string(), "Invalid workflow: cycle detected");
    }

    #[test]
    fn test_error_display_work_item_not_found() {
        let error = TreadleError::WorkItemNotFound("item-123".to_string());
        assert_eq!(error.to_string(), "Work item not found: item-123");
    }

    #[test]
    fn test_error_display_stage_not_found() {
        let error = TreadleError::StageNotFound("scan".to_string());
        assert_eq!(error.to_string(), "Stage not found: scan");
    }

    #[test]
    fn test_error_display_duplicate_stage() {
        let error = TreadleError::DuplicateStage("scan".to_string());
        assert_eq!(error.to_string(), "Duplicate stage name: scan");
    }

    #[test]
    fn test_error_display_dag_cycle() {
        let error = TreadleError::DagCycle;
        assert_eq!(error.to_string(), "Cycle detected in workflow DAG");
    }

    #[test]
    fn test_error_from_serde_json() {
        let json_error = serde_json::from_str::<serde_json::Value>("invalid json").unwrap_err();
        let treadle_error: TreadleError = json_error.into();
        assert!(treadle_error.to_string().contains("Serialization error"));
    }

    #[test]
    fn test_error_from_io() {
        let io_error = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let treadle_error: TreadleError = io_error.into();
        assert!(treadle_error.to_string().contains("I/O error"));
    }

    #[test]
    fn test_result_ok() {
        let result: Result<i32> = Ok(42);
        assert!(result.is_ok());
        assert_eq!(result.ok(), Some(42));
    }

    #[test]
    fn test_result_err() {
        let result: Result<i32> = Err(TreadleError::StateStore("test".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn test_error_debug_format() {
        let error = TreadleError::StateStore("debug test".to_string());
        let debug_output = format!("{:?}", error);
        assert!(debug_output.contains("StateStore"));
        assert!(debug_output.contains("debug test"));
    }
}
