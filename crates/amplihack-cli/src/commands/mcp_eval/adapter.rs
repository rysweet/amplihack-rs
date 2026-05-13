//! MCP tool adapter trait and built-in mock implementation.

use anyhow::Result;
use serde::Serialize;
use std::time::Duration;

/// Trait that all MCP tool adapters must implement.
pub trait McpToolAdapter: Send + Sync {
    /// Human-readable name of the adapter.
    fn name(&self) -> &str;

    /// Enable/activate the MCP tool (e.g., start server, connect).
    fn enable(&self) -> Result<()>;

    /// Disable/deactivate the MCP tool.
    fn disable(&self) -> Result<()>;

    /// Measure the execution of a single operation.
    /// Returns the duration and whether it succeeded.
    fn measure(&self, operation: &str) -> Result<MeasurementResult>;
}

/// Result of a single tool measurement.
#[derive(Debug, Clone, Serialize)]
pub struct MeasurementResult {
    pub operation: String,
    pub duration: Duration,
    pub success: bool,
    pub output: Option<String>,
}

/// Mock adapter for testing without a real MCP server.
pub struct MockAdapter {
    name: String,
    enabled: std::sync::atomic::AtomicBool,
}

impl MockAdapter {
    pub fn new() -> Self {
        Self {
            name: "mock".to_string(),
            enabled: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

impl Default for MockAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl McpToolAdapter for MockAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn enable(&self) -> Result<()> {
        self.enabled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn disable(&self) -> Result<()> {
        self.enabled
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    fn measure(&self, operation: &str) -> Result<MeasurementResult> {
        // Mock measurements with realistic-looking durations
        let duration = match operation {
            op if op.contains("navigate") || op.contains("find") => Duration::from_millis(150),
            op if op.contains("analyze") || op.contains("search") => Duration::from_millis(300),
            op if op.contains("modify") || op.contains("edit") => Duration::from_millis(500),
            _ => Duration::from_millis(200),
        };

        Ok(MeasurementResult {
            operation: operation.to_string(),
            duration,
            success: true,
            output: Some(format!("[mock] Completed: {}", operation)),
        })
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_adapter_has_correct_name() {
        let adapter = MockAdapter::new();
        assert_eq!(adapter.name(), "mock");
    }

    #[test]
    fn mock_adapter_enable_disable_cycle() {
        let adapter = MockAdapter::new();
        assert!(adapter.enable().is_ok());
        assert!(adapter.disable().is_ok());
    }

    #[test]
    fn mock_adapter_measure_returns_success() {
        let adapter = MockAdapter::new();
        adapter.enable().unwrap();

        let result = adapter.measure("navigate_to_definition").unwrap();
        assert!(result.success);
        assert_eq!(result.operation, "navigate_to_definition");
        assert!(result.duration.as_millis() > 0);
        assert!(result.output.is_some());
    }

    #[test]
    fn mock_adapter_measure_varies_by_operation_type() {
        let adapter = MockAdapter::new();
        adapter.enable().unwrap();

        let nav = adapter.measure("find_references").unwrap();
        let analyze = adapter.measure("analyze_code").unwrap();
        let modify = adapter.measure("modify_file").unwrap();

        // Different operation types should produce different durations
        assert!(nav.duration < analyze.duration);
        assert!(analyze.duration < modify.duration);
    }

    #[test]
    fn mock_adapter_default_works() {
        let adapter = MockAdapter::default();
        assert_eq!(adapter.name(), "mock");
    }

    #[test]
    fn measurement_result_serializes_to_json() {
        let result = MeasurementResult {
            operation: "test_op".to_string(),
            duration: Duration::from_millis(100),
            success: true,
            output: Some("done".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("test_op"));
        assert!(json.contains("\"success\":true"));
    }
}
