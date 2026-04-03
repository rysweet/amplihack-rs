//! Data models for the bundle generator pipeline.
//!
//! Ported from `amplihack/bundle_generator/models.py`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageFormat {
    TarGz,
    Zip,
    Directory,
    Uvx,
}

impl std::fmt::Display for PackageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TarGz => write!(f, "tar.gz"),
            Self::Zip => write!(f, "zip"),
            Self::Directory => write!(f, "directory"),
            Self::Uvx => write!(f, "uvx"),
        }
    }
}

/// A packaged bundle ready for distribution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackagedBundle {
    pub format: PackageFormat,
    pub path: PathBuf,
    #[serde(default)]
    pub size_bytes: u64,
    #[serde(default)]
    pub checksum: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

impl PackagedBundle {
    pub fn new(format: PackageFormat, path: PathBuf) -> Self {
        Self {
            format,
            path,
            size_bytes: 0,
            checksum: String::new(),
            metadata: HashMap::new(),
        }
    }

    /// Populate default UVX metadata if format is Uvx and metadata is empty.
    pub fn fill_uvx_defaults(&mut self, bundle_name: &str) {
        if self.format == PackageFormat::Uvx && self.metadata.is_empty() {
            self.metadata
                .insert("version".to_string(), "1.0.0".to_string());
            self.metadata
                .insert("python_requirement".to_string(), ">=3.11".to_string());
            self.metadata.insert(
                "entry_point".to_string(),
                format!("amplihack.bundle_generator.{bundle_name}"),
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DistributionPlatform {
    Github,
    Pypi,
    Local,
}

/// Outcome of distributing a packaged bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionResult {
    pub success: bool,
    pub platform: DistributionPlatform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<String>,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(default)]
    pub distribution_time_seconds: f64,
}

impl DistributionResult {
    pub fn ok(platform: DistributionPlatform) -> Self {
        Self {
            success: true,
            platform,
            url: None,
            release_tag: None,
            assets: Vec::new(),
            timestamp: String::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            distribution_time_seconds: 0.0,
        }
    }
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestType {
    Agent,
    Bundle,
    Integration,
}

/// A single test failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestFailure {
    pub name: String,
    pub message: String,
}

/// Outcome of running a test suite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub test_type: TestType,
    pub target_name: String,
    pub passed: bool,
    #[serde(default)]
    pub test_count: u32,
    #[serde(default)]
    pub passed_count: u32,
    #[serde(default)]
    pub failed_count: u32,
    #[serde(default)]
    pub skipped_count: u32,
    #[serde(default)]
    pub duration_seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failures: Vec<TestFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_percent: Option<f64>,
}

impl TestResult {
    pub fn passed(test_type: TestType, target_name: impl Into<String>) -> Self {
        Self {
            test_type,
            target_name: target_name.into(),
            passed: true,
            test_count: 0,
            passed_count: 0,
            failed_count: 0,
            skipped_count: 0,
            duration_seconds: 0.0,
            failures: Vec::new(),
            coverage_percent: None,
        }
    }
    pub fn success_rate(&self) -> f64 {
        if self.test_count == 0 {
            return 0.0;
        }
        f64::from(self.passed_count) / f64::from(self.test_count)
    }
}

/// Performance metrics for bundle generation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationMetrics {
    #[serde(default)]
    pub total_duration_seconds: f64,
    #[serde(default)]
    pub parsing_time: f64,
    #[serde(default)]
    pub extraction_time: f64,
    #[serde(default)]
    pub generation_time: f64,
    #[serde(default)]
    pub validation_time: f64,
    #[serde(default)]
    pub packaging_time: f64,
    #[serde(default)]
    pub files_generated: u32,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub agent_count: u32,
    #[serde(default)]
    pub total_size_kb: f64,
    #[serde(default)]
    pub memory_peak_mb: f64,
}

impl GenerationMetrics {
    pub fn average_agent_time(&self) -> f64 {
        if self.agent_count == 0 {
            return 0.0;
        }
        self.generation_time / f64::from(self.agent_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_packaged_bundle_new() {
        let pb = PackagedBundle::new(PackageFormat::TarGz, PathBuf::from("/out/b.tar.gz"));
        assert_eq!(pb.format, PackageFormat::TarGz);
        assert_eq!(pb.size_bytes, 0);
    }

    #[test]
    fn test_uvx_defaults_filled() {
        let mut pb = PackagedBundle::new(PackageFormat::Uvx, PathBuf::from("/out/b.uvx"));
        pb.fill_uvx_defaults("my-agent");
        assert_eq!(pb.metadata["version"], "1.0.0");
        assert!(pb.metadata["entry_point"].contains("my-agent"));
    }

    #[test]
    fn test_uvx_defaults_not_overwritten() {
        let mut pb = PackagedBundle::new(PackageFormat::Uvx, PathBuf::from("/out/b.uvx"));
        pb.metadata
            .insert("version".to_string(), "2.0.0".to_string());
        pb.fill_uvx_defaults("agent");
        assert_eq!(pb.metadata["version"], "2.0.0");
    }

    #[test]
    fn test_non_uvx_no_defaults() {
        let mut pb = PackagedBundle::new(PackageFormat::Zip, PathBuf::from("/out/b.zip"));
        pb.fill_uvx_defaults("agent");
        assert!(pb.metadata.is_empty());
    }

    #[test]
    fn test_distribution_result_ok() {
        let r = DistributionResult::ok(DistributionPlatform::Github);
        assert!(r.success);
        assert!(!r.has_errors());
        assert!(!r.has_warnings());
    }

    #[test]
    fn test_distribution_result_has_errors() {
        let mut r = DistributionResult::ok(DistributionPlatform::Local);
        r.errors.push("disk full".to_string());
        assert!(r.has_errors());
    }

    #[test]
    fn test_test_result_success_rate() {
        let mut tr = TestResult::passed(TestType::Agent, "scanner");
        tr.test_count = 10;
        tr.passed_count = 8;
        assert!((tr.success_rate() - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn test_test_result_zero() {
        let tr = TestResult::passed(TestType::Bundle, "b");
        assert!((tr.success_rate()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_generation_metrics_default() {
        let m = GenerationMetrics::default();
        assert_eq!(m.files_generated, 0);
    }

    #[test]
    fn test_average_agent_time() {
        let m = GenerationMetrics {
            generation_time: 6.0,
            agent_count: 3,
            ..Default::default()
        };
        assert!((m.average_agent_time() - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_average_agent_time_zero() {
        assert!((GenerationMetrics::default().average_agent_time()).abs() < f64::EPSILON);
    }

    #[test]
    fn test_package_format_display() {
        assert_eq!(PackageFormat::TarGz.to_string(), "tar.gz");
        assert_eq!(PackageFormat::Uvx.to_string(), "uvx");
    }

    #[test]
    fn test_serde_roundtrip_packaged_bundle() {
        let mut pb = PackagedBundle::new(PackageFormat::Directory, PathBuf::from("/out"));
        pb.size_bytes = 1024;
        pb.checksum = "abc123".to_string();
        let json = serde_json::to_string(&pb).expect("ser");
        let pb2: PackagedBundle = serde_json::from_str(&json).expect("de");
        assert_eq!(pb2.size_bytes, 1024);
    }

    #[test]
    fn test_serde_roundtrip_test_result() {
        let tr = TestResult {
            test_type: TestType::Integration,
            target_name: "my-bundle".to_string(),
            passed: false,
            test_count: 5,
            passed_count: 3,
            failed_count: 2,
            skipped_count: 0,
            duration_seconds: 1.5,
            failures: vec![TestFailure {
                name: "test_x".to_string(),
                message: "assert failed".to_string(),
            }],
            coverage_percent: Some(85.0),
        };
        let json = serde_json::to_string(&tr).expect("ser");
        let tr2: TestResult = serde_json::from_str(&json).expect("de");
        assert_eq!(tr2.failed_count, 2);
        assert_eq!(tr2.failures.len(), 1);
    }

    #[test]
    fn test_serde_roundtrip_metrics() {
        let m = GenerationMetrics {
            total_duration_seconds: 12.5,
            model: "gpt-4".to_string(),
            tokens_used: 9000,
            ..Default::default()
        };
        let json = serde_json::to_string(&m).expect("ser");
        let m2: GenerationMetrics = serde_json::from_str(&json).expect("de");
        assert_eq!(m2.model, "gpt-4");
        assert_eq!(m2.tokens_used, 9000);
    }
}
