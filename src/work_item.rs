//! Work item trait for the Treadle workflow engine.
//!
//! This module defines the `WorkItem` trait that all work items must implement
//! to be processed through the workflow pipeline.

use std::fmt::Debug;

/// A trait representing an item that can be processed through the workflow.
///
/// Each work item must have a unique identifier that persists across the
/// workflow. While the trait itself only requires `Debug` for inspection,
/// **implementations should also derive or implement `Clone`, `Serialize`,
/// and `Deserialize`** to support persistence and state management.
///
/// # Object Safety
///
/// This trait is object-safe, allowing for dynamic dispatch with
/// `dyn WorkItem`. This enables workflows to process heterogeneous work items.
///
/// # Implementation Guidelines
///
/// Implementations should:
/// - Implement `Clone` to allow work items to be duplicated
/// - Implement `Serialize` and `Deserialize` (via serde) for persistence
/// - Ensure the `id()` returns a stable, unique identifier
///
/// # Examples
///
/// ```
/// use treadle::WorkItem;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct ScanTask {
///     id: String,
///     target: String,
/// }
///
/// impl WorkItem for ScanTask {
///     fn id(&self) -> &str {
///         &self.id
///     }
/// }
/// ```
pub trait WorkItem: Debug + Send + Sync {
    /// Returns the unique identifier for this work item.
    ///
    /// This ID is used to track the item's progress through the workflow
    /// and to store/retrieve its state. The ID must be stable across
    /// serialization and deserialization.
    ///
    /// # Returns
    ///
    /// A string slice containing the unique identifier.
    fn id(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestItem {
        id: String,
        data: String,
    }

    impl WorkItem for TestItem {
        fn id(&self) -> &str {
            &self.id
        }
    }

    #[test]
    fn test_work_item_id() {
        let item = TestItem {
            id: "test-123".to_string(),
            data: "test data".to_string(),
        };
        assert_eq!(item.id(), "test-123");
    }

    #[test]
    fn test_work_item_clone() {
        let item = TestItem {
            id: "test-456".to_string(),
            data: "original".to_string(),
        };
        let cloned = item.clone();
        assert_eq!(item, cloned);
        assert_eq!(cloned.id(), "test-456");
    }

    #[test]
    fn test_work_item_debug() {
        let item = TestItem {
            id: "test-789".to_string(),
            data: "debug test".to_string(),
        };
        let debug_output = format!("{:?}", item);
        assert!(debug_output.contains("TestItem"));
        assert!(debug_output.contains("test-789"));
    }

    #[test]
    fn test_work_item_serialize() {
        let item = TestItem {
            id: "test-serialize".to_string(),
            data: "serialize test".to_string(),
        };
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("test-serialize"));
        assert!(json.contains("serialize test"));
    }

    #[test]
    fn test_work_item_deserialize() {
        let json = r#"{"id":"test-deserialize","data":"deserialize test"}"#;
        let item: TestItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.id(), "test-deserialize");
        assert_eq!(item.data, "deserialize test");
    }

    #[test]
    fn test_work_item_round_trip() {
        let original = TestItem {
            id: "test-round-trip".to_string(),
            data: "round trip test".to_string(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: TestItem = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_work_item_object_safety() {
        let item = TestItem {
            id: "test-dyn".to_string(),
            data: "dynamic dispatch".to_string(),
        };
        let dyn_item: &dyn WorkItem = &item;
        assert_eq!(dyn_item.id(), "test-dyn");
    }

    #[test]
    fn test_work_item_empty_id() {
        let item = TestItem {
            id: String::new(),
            data: "empty id test".to_string(),
        };
        assert_eq!(item.id(), "");
    }

    #[test]
    fn test_work_item_unicode_id() {
        let item = TestItem {
            id: "测试-🔧-αβγ".to_string(),
            data: "unicode test".to_string(),
        };
        assert_eq!(item.id(), "测试-🔧-αβγ");
    }

    #[test]
    fn test_work_item_long_id() {
        let long_id = "x".repeat(1000);
        let item = TestItem {
            id: long_id.clone(),
            data: "long id test".to_string(),
        };
        assert_eq!(item.id(), long_id);
        assert_eq!(item.id().len(), 1000);
    }
}
