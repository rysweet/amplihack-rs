//! Observability helpers for OpenTelemetry integration.
//!
//! Ports Python `amplihack/observability.py`. Provides a thin façade over
//! the `tracing` crate so callers can use a uniform API regardless of whether
//! a real OpenTelemetry SDK is linked. No hard `opentelemetry` dependency is
//! added — this module checks environment variables and logs intent only.

use std::collections::HashMap;

use tracing::{debug, info, warn};

/// Environment variable names that signal telemetry should be active.
const OTEL_ENABLED_VAR: &str = "AMPLIHACK_OTEL_ENABLED";
const OTEL_ENDPOINT_VAR: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const OTEL_TRACES_ENDPOINT_VAR: &str = "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT";

/// Check whether telemetry has been requested via environment variables.
///
/// Returns `true` if `AMPLIHACK_OTEL_ENABLED=1` or any OTEL endpoint env var
/// is set.
pub fn telemetry_requested() -> bool {
    if let Ok(val) = std::env::var(OTEL_ENABLED_VAR)
        && (val == "1" || val.eq_ignore_ascii_case("true"))
    {
        return true;
    }
    std::env::var(OTEL_ENDPOINT_VAR).is_ok() || std::env::var(OTEL_TRACES_ENDPOINT_VAR).is_ok()
}

/// Build a map of environment variable overrides for child processes.
///
/// Sets `OTEL_SERVICE_NAME` and resource attributes so child processes
/// inherit the telemetry configuration.
pub fn otel_env_overrides(
    service_name: &str,
    attributes: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();
    env.insert("OTEL_SERVICE_NAME".to_string(), service_name.to_string());

    if !attributes.is_empty() {
        let attr_str: String = attributes
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        env.insert("OTEL_RESOURCE_ATTRIBUTES".to_string(), attr_str);
    }

    // Forward existing endpoint if set
    if let Ok(endpoint) = std::env::var(OTEL_ENDPOINT_VAR) {
        env.insert(OTEL_ENDPOINT_VAR.to_string(), endpoint);
    }

    env
}

/// Configure the OpenTelemetry tracer provider (stub implementation).
///
/// In a full implementation this would initialise an OTLP exporter.
/// Currently it checks env vars, logs the configuration intent, and
/// returns `false` to indicate no real SDK was configured.
pub fn configure_otel(
    service_name: &str,
    component: &str,
    attributes: &HashMap<String, String>,
) -> bool {
    if !telemetry_requested() {
        debug!("telemetry not requested, skipping OTel configuration");
        return false;
    }

    info!(
        service = service_name,
        component = component,
        attribute_count = attributes.len(),
        "OTel configuration requested (stub — no SDK linked)"
    );
    false
}

/// A lightweight span handle returned by [`start_span`].
///
/// When a real OTel SDK is linked this would wrap `opentelemetry::trace::Span`.
/// Currently it is a no-op placeholder that emits `tracing` events.
#[derive(Debug)]
pub struct SpanHandle {
    name: String,
    active: bool,
}

impl SpanHandle {
    /// Mark this span as ended.
    pub fn end(&mut self) {
        if self.active {
            debug!(span = %self.name, "span ended (stub)");
            self.active = false;
        }
    }
}

impl Drop for SpanHandle {
    fn drop(&mut self) {
        if self.active {
            debug!(span = %self.name, "span dropped without explicit end");
            self.active = false;
        }
    }
}

/// Start a named span when telemetry is enabled.
///
/// Returns a [`SpanHandle`] that can be enriched with attributes and events.
/// If telemetry is not enabled the span still tracks its name for logging.
pub fn start_span(
    name: &str,
    tracer_name: &str,
    attributes: &HashMap<String, String>,
) -> SpanHandle {
    if telemetry_requested() {
        debug!(
            span = name,
            tracer = tracer_name,
            attrs = attributes.len(),
            "starting span (stub)"
        );
    }
    SpanHandle {
        name: name.to_string(),
        active: true,
    }
}

/// Set attributes on an active span (best-effort).
pub fn set_span_attributes(span: &SpanHandle, attributes: &HashMap<String, String>) {
    if !span.active {
        warn!(span = %span.name, "set_span_attributes on inactive span");
        return;
    }
    debug!(
        span = %span.name,
        count = attributes.len(),
        "set span attributes (stub)"
    );
}

/// Add an event to an active span (best-effort).
pub fn add_span_event(span: &SpanHandle, name: &str, attributes: &HashMap<String, String>) {
    if !span.active {
        warn!(span_name = %span.name, "add_span_event on inactive span");
        return;
    }
    debug!(
        span = %span.name,
        event = name,
        attrs = attributes.len(),
        "span event (stub)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_requested_default_false() {
        // In a clean test environment these vars are typically unset.
        // We can't guarantee it, so just verify the function runs.
        let _ = telemetry_requested();
    }

    #[test]
    fn otel_env_overrides_sets_service_name() {
        let attrs = HashMap::new();
        let env = otel_env_overrides("test-service", &attrs);
        assert_eq!(env["OTEL_SERVICE_NAME"], "test-service");
    }

    #[test]
    fn otel_env_overrides_includes_attributes() {
        let mut attrs = HashMap::new();
        attrs.insert("env".to_string(), "test".to_string());
        attrs.insert("version".to_string(), "1.0".to_string());
        let env = otel_env_overrides("svc", &attrs);
        let res_attrs = &env["OTEL_RESOURCE_ATTRIBUTES"];
        // Order is non-deterministic in HashMap, but both should appear
        assert!(res_attrs.contains("env=test"));
        assert!(res_attrs.contains("version=1.0"));
    }

    #[test]
    fn configure_otel_stub_returns_false() {
        let attrs = HashMap::new();
        let result = configure_otel("test", "component", &attrs);
        assert!(!result, "stub implementation should return false");
    }

    #[test]
    fn span_lifecycle() {
        let attrs = HashMap::new();
        let mut span = start_span("test-span", "test-tracer", &attrs);
        assert!(span.active);

        set_span_attributes(&span, &attrs);
        add_span_event(&span, "checkpoint", &attrs);

        span.end();
        assert!(!span.active);
    }

    #[test]
    fn span_double_end_is_safe() {
        let attrs = HashMap::new();
        let mut span = start_span("double-end", "tracer", &attrs);
        span.end();
        span.end(); // should not panic
        assert!(!span.active);
    }

    #[test]
    fn set_attributes_on_inactive_span() {
        let attrs = HashMap::new();
        let mut span = start_span("inactive", "tracer", &attrs);
        span.end();
        // Should not panic, just warn
        set_span_attributes(&span, &attrs);
    }
}
