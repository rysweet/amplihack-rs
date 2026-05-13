//! Level 2-4 validation: shell out to `dotnet` for build, analyzers, and format checks.

use super::{CsValidatorConfig, Diagnostic, DiagnosticSeverity, EXIT_MISSING_DEPENDENCY};
use anyhow::{bail, Result};
use std::path::Path;
use std::process::Command;

/// Check if `dotnet` is available on PATH.
pub fn dotnet_available() -> bool {
    Command::new("dotnet")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Level 2: Run `dotnet build --no-restore --nologo`.
pub fn run_dotnet_build(path: &Path, config: &CsValidatorConfig) -> Result<Vec<Diagnostic>> {
    if !dotnet_available() {
        bail!("dotnet CLI not found (exit code {})", EXIT_MISSING_DEPENDENCY);
    }

    let project_dir = if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };

    let output = Command::new("dotnet")
        .args(["build", "--no-restore", "--nologo", "-p:GenerateFullPaths=true"])
        .current_dir(project_dir)
        .output()?;

    if output.status.success() {
        return Ok(Vec::new());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}\n{}", stdout, stderr);

    Ok(parse_dotnet_diagnostics(&combined, config))
}

/// Level 3: Run build with analyzers enabled.
pub fn run_dotnet_analyzers(path: &Path, config: &CsValidatorConfig) -> Result<Vec<Diagnostic>> {
    if !dotnet_available() {
        bail!("dotnet CLI not found (exit code {})", EXIT_MISSING_DEPENDENCY);
    }

    let project_dir = if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };

    let output = Command::new("dotnet")
        .args([
            "build",
            "--no-restore",
            "--nologo",
            "-p:GenerateFullPaths=true",
            "-p:RunAnalyzers=true",
            "-p:EnforceCodeStyleInBuild=true",
        ])
        .current_dir(project_dir)
        .output()?;

    if output.status.success() {
        return Ok(Vec::new());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}\n{}", stdout, stderr);

    Ok(parse_dotnet_diagnostics(&combined, config))
}

/// Level 4: Run `dotnet format --verify-no-changes`.
pub fn run_dotnet_format(path: &Path, _config: &CsValidatorConfig) -> Result<Vec<Diagnostic>> {
    if !dotnet_available() {
        bail!("dotnet CLI not found (exit code {})", EXIT_MISSING_DEPENDENCY);
    }

    let project_dir = if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };

    let output = Command::new("dotnet")
        .args(["format", "--verify-no-changes"])
        .current_dir(project_dir)
        .output()?;

    if output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(vec![Diagnostic {
        severity: DiagnosticSeverity::Warning,
        message: format!("Format check failed: {}", stdout.trim().chars().take(200).collect::<String>()),
        line: None,
        column: None,
    }])
}

/// Parse MSBuild-style diagnostic output into structured diagnostics.
fn parse_dotnet_diagnostics(output: &str, config: &CsValidatorConfig) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let threshold = &config.analyzer_severity_threshold;

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.contains(": error ") {
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Error,
                message: trimmed.to_string(),
                line: extract_line_number(trimmed),
                column: None,
            });
        } else if trimmed.contains(": warning ") && threshold != "Error" {
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Warning,
                message: trimmed.to_string(),
                line: extract_line_number(trimmed),
                column: None,
            });
        } else if trimmed.contains(": info ") && threshold == "Info" {
            diagnostics.push(Diagnostic {
                severity: DiagnosticSeverity::Info,
                message: trimmed.to_string(),
                line: extract_line_number(trimmed),
                column: None,
            });
        }
    }

    diagnostics
}

/// Extract line number from MSBuild format like `Foo.cs(12,5): error CS1234: msg`
fn extract_line_number(s: &str) -> Option<u32> {
    if let Some(paren_start) = s.find('(')
        && let Some(comma_or_paren) = s[paren_start + 1..].find([',', ')']) {
            let num_str = &s[paren_start + 1..paren_start + 1 + comma_or_paren];
            return num_str.parse().ok();
        }
    None
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_lines() {
        let output = r#"
Program.cs(5,10): error CS1002: ; expected
Program.cs(12,1): error CS0246: The type or namespace name 'Foo' could not be found
"#;
        let config = CsValidatorConfig::default();
        let diags = parse_dotnet_diagnostics(output, &config);
        assert_eq!(diags.len(), 2);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Error);
        assert_eq!(diags[0].line, Some(5));
        assert_eq!(diags[1].line, Some(12));
    }

    #[test]
    fn parse_warnings_only_when_threshold_allows() {
        let output = "Foo.cs(3,1): warning CS0168: unused variable\n";

        // Default threshold is "Error" — warnings should be filtered
        let config = CsValidatorConfig::default();
        let diags = parse_dotnet_diagnostics(output, &config);
        assert_eq!(diags.len(), 0);

        // With "Warning" threshold — should include
        let mut config2 = CsValidatorConfig::default();
        config2.analyzer_severity_threshold = "Warning".to_string();
        let diags = parse_dotnet_diagnostics(output, &config2);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Warning);
    }

    #[test]
    fn parse_info_only_with_info_threshold() {
        let output = "Foo.cs(1,1): info IDE0001: simplify name\n";

        let config = CsValidatorConfig::default();
        let diags = parse_dotnet_diagnostics(output, &config);
        assert_eq!(diags.len(), 0);

        let mut config2 = CsValidatorConfig::default();
        config2.analyzer_severity_threshold = "Info".to_string();
        let diags = parse_dotnet_diagnostics(output, &config2);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].severity, DiagnosticSeverity::Info);
    }

    #[test]
    fn extract_line_number_from_msbuild_format() {
        assert_eq!(extract_line_number("Foo.cs(5,10): error CS1002: ; expected"), Some(5));
        assert_eq!(extract_line_number("Bar.cs(123,1): warning CS0168: unused"), Some(123));
        assert_eq!(extract_line_number("no parens here"), None);
    }

    #[test]
    fn dotnet_available_does_not_panic() {
        // This test verifies the function doesn't panic regardless of env
        let _ = dotnet_available();
    }

    use super::super::CsValidatorConfig;
}
