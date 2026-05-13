//! C# file validation at 4 strictness levels.
//!
//! Level 1: Pure-Rust syntax checks (balanced delimiters, common patterns)
//! Level 2: Syntax + `dotnet build`
//! Level 3: Syntax + build + analyzers
//! Level 4: All + `dotnet format --verify-no-changes`

pub mod build;
pub mod syntax;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ── Exit codes ───────────────────────────────────────────────────────────────

pub const EXIT_PASS: i32 = 0;
pub const EXIT_VALIDATION_FAIL: i32 = 1;
pub const EXIT_CONFIG_ERROR: i32 = 2;
pub const EXIT_TIMEOUT: i32 = 3;
pub const EXIT_MISSING_DEPENDENCY: i32 = 4;

// ── Validation level ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValidationLevel {
    Syntax = 1,
    Build = 2,
    Analyzers = 3,
    Format = 4,
}

impl ValidationLevel {
    pub fn from_u8(n: u8) -> Option<Self> {
        match n {
            1 => Some(Self::Syntax),
            2 => Some(Self::Build),
            3 => Some(Self::Analyzers),
            4 => Some(Self::Format),
            _ => None,
        }
    }
}

// ── Configuration ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CsValidatorConfig {
    pub enabled: bool,
    pub validation_level: u8,
    pub analyzer_severity_threshold: String,
    #[serde(default)]
    pub skip_projects: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    #[serde(default)]
    pub parallel: bool,
    #[serde(default)]
    pub cache_enabled: bool,
    #[serde(default)]
    pub reporting: ReportingConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportingConfig {
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub verbose: bool,
    pub output_file: Option<String>,
}

fn default_timeout() -> u32 {
    30
}
fn default_format() -> String {
    "json".to_string()
}

impl Default for CsValidatorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            validation_level: 2,
            analyzer_severity_threshold: "Error".to_string(),
            skip_projects: Vec::new(),
            timeout_seconds: 30,
            parallel: true,
            cache_enabled: true,
            reporting: ReportingConfig::default(),
        }
    }
}

// ── Validation result ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct ValidationResult {
    pub file: PathBuf,
    pub level: u8,
    pub passed: bool,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

// ── Config loading ───────────────────────────────────────────────────────────

/// Load config with cascading search: workspace `.claude/config/cs-validator.json`
/// first, then `~/.amplihack/.claude/config/cs-validator.json`.
pub fn load_config(config_override: Option<&Path>) -> Result<CsValidatorConfig> {
    if let Some(p) = config_override {
        let content = std::fs::read_to_string(p)?;
        let cfg: CsValidatorConfig = serde_json::from_str(&content)?;
        return Ok(cfg);
    }

    // Workspace config
    let workspace_path = Path::new(".claude/config/cs-validator.json");
    if workspace_path.exists() {
        let content = std::fs::read_to_string(workspace_path)?;
        let cfg: CsValidatorConfig = serde_json::from_str(&content)?;
        return Ok(cfg);
    }

    // Global config
    if let Some(home) = dirs_home() {
        let global_path = home.join(".amplihack/.claude/config/cs-validator.json");
        if global_path.exists() {
            let content = std::fs::read_to_string(&global_path)?;
            let cfg: CsValidatorConfig = serde_json::from_str(&content)?;
            return Ok(cfg);
        }
    }

    Ok(CsValidatorConfig::default())
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

// ── CLI dispatch ─────────────────────────────────────────────────────────────

/// Entry point from the CLI dispatch layer.
pub fn dispatch(
    path: PathBuf,
    level: u8,
    config_path: Option<PathBuf>,
    format: String,
) -> Result<()> {
    let config = load_config(config_path.as_deref())?;

    // Use CLI-provided level if non-zero, otherwise fall back to config
    let effective_level = if level > 0 {
        level
    } else {
        config.validation_level
    };

    let validation_level = ValidationLevel::from_u8(effective_level).ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid validation level: {}. Must be 1-4.",
            effective_level
        )
    })?;

    // Check dotnet availability for levels 2+
    if validation_level >= ValidationLevel::Build && !build::dotnet_available() {
        eprintln!("error: dotnet CLI not found. Levels 2-4 require the .NET SDK.");
        eprintln!("Install from: https://dot.net/download");
        std::process::exit(EXIT_MISSING_DEPENDENCY);
    }

    let results = run_validate(&path, validation_level, &config)?;

    let any_failed = results.iter().any(|r| !r.passed);

    match format.as_str() {
        "json" => {
            let json = serde_json::to_string_pretty(&results)?;
            println!("{json}");
        }
        _ => {
            for r in &results {
                let status = if r.passed { "PASS" } else { "FAIL" };
                println!("[{status}] {} (level {})", r.file.display(), r.level);
                for d in &r.diagnostics {
                    let loc = match (d.line, d.column) {
                        (Some(l), Some(c)) => format!("{}:{}", l, c),
                        (Some(l), None) => format!("{}:", l),
                        _ => String::new(),
                    };
                    println!("  {:?} {}{}", d.severity, loc, d.message);
                }
            }
        }
    }

    if any_failed {
        std::process::exit(EXIT_VALIDATION_FAIL);
    }
    Ok(())
}

// ── Main entry point ─────────────────────────────────────────────────────────

/// Run validation at the given level on the target path.
pub fn run_validate(
    path: &Path,
    level: ValidationLevel,
    config: &CsValidatorConfig,
) -> Result<Vec<ValidationResult>> {
    let files = discover_cs_files(path)?;
    if files.is_empty() {
        anyhow::bail!("No .cs files found at path: {}", path.display());
    }

    let mut results = Vec::new();
    for file in &files {
        let mut result = ValidationResult {
            file: file.clone(),
            level: level as u8,
            passed: true,
            diagnostics: Vec::new(),
        };

        // Level 1: syntax
        let syntax_diags = syntax::check_syntax(file)?;
        if !syntax_diags.is_empty() {
            result.passed = false;
            result.diagnostics.extend(syntax_diags);
        }

        // Levels 2+: build
        if level >= ValidationLevel::Build && result.passed {
            match build::run_dotnet_build(file, config) {
                Ok(diags) => {
                    if !diags.is_empty() {
                        result.passed = false;
                        result.diagnostics.extend(diags);
                    }
                }
                Err(e) => return Err(e),
            }
        }

        // Level 3: analyzers
        if level >= ValidationLevel::Analyzers && result.passed {
            match build::run_dotnet_analyzers(file, config) {
                Ok(diags) => {
                    if !diags.is_empty() {
                        result.passed = false;
                        result.diagnostics.extend(diags);
                    }
                }
                Err(e) => return Err(e),
            }
        }

        // Level 4: format
        if level >= ValidationLevel::Format && result.passed {
            match build::run_dotnet_format(file, config) {
                Ok(diags) => {
                    if !diags.is_empty() {
                        result.passed = false;
                        result.diagnostics.extend(diags);
                    }
                }
                Err(e) => return Err(e),
            }
        }

        results.push(result);
    }

    Ok(results)
}

/// Discover .cs files at the given path (file or directory).
fn discover_cs_files(path: &Path) -> Result<Vec<PathBuf>> {
    if path.is_file() {
        if path.extension().is_some_and(|e| e == "cs") {
            return Ok(vec![path.to_path_buf()]);
        }
        anyhow::bail!("Path is not a .cs file: {}", path.display());
    }

    if path.is_dir() {
        let mut files = Vec::new();
        collect_cs_files(path, &mut files)?;
        files.sort();
        return Ok(files);
    }

    anyhow::bail!("Path does not exist: {}", path.display());
}

fn collect_cs_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_cs_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "cs") {
            out.push(path);
        }
    }
    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── ValidationLevel tests ────────────────────────────────────────────

    #[test]
    fn validation_level_from_u8_valid() {
        assert_eq!(ValidationLevel::from_u8(1), Some(ValidationLevel::Syntax));
        assert_eq!(ValidationLevel::from_u8(2), Some(ValidationLevel::Build));
        assert_eq!(ValidationLevel::from_u8(3), Some(ValidationLevel::Analyzers));
        assert_eq!(ValidationLevel::from_u8(4), Some(ValidationLevel::Format));
    }

    #[test]
    fn validation_level_from_u8_invalid() {
        assert_eq!(ValidationLevel::from_u8(0), None);
        assert_eq!(ValidationLevel::from_u8(5), None);
        assert_eq!(ValidationLevel::from_u8(255), None);
    }

    #[test]
    fn validation_level_ordering() {
        assert!(ValidationLevel::Syntax < ValidationLevel::Build);
        assert!(ValidationLevel::Build < ValidationLevel::Analyzers);
        assert!(ValidationLevel::Analyzers < ValidationLevel::Format);
    }

    // ── Config tests ─────────────────────────────────────────────────────

    #[test]
    fn config_default_has_level_2() {
        let cfg = CsValidatorConfig::default();
        assert_eq!(cfg.validation_level, 2);
        assert!(cfg.enabled);
        assert_eq!(cfg.timeout_seconds, 30);
        assert_eq!(cfg.analyzer_severity_threshold, "Error");
    }

    #[test]
    fn config_deserializes_from_json() {
        let json = r#"{
            "enabled": true,
            "validationLevel": 3,
            "analyzerSeverityThreshold": "Warning",
            "skipProjects": ["Tests/**/*.csproj"],
            "timeoutSeconds": 60,
            "parallel": false,
            "cacheEnabled": true,
            "reporting": {
                "format": "text",
                "verbose": true,
                "outputFile": "/tmp/out.json"
            }
        }"#;
        let cfg: CsValidatorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.validation_level, 3);
        assert_eq!(cfg.analyzer_severity_threshold, "Warning");
        assert_eq!(cfg.skip_projects, vec!["Tests/**/*.csproj"]);
        assert_eq!(cfg.timeout_seconds, 60);
        assert!(!cfg.parallel);
        assert_eq!(cfg.reporting.format, "text");
        assert!(cfg.reporting.verbose);
        assert_eq!(cfg.reporting.output_file, Some("/tmp/out.json".to_string()));
    }

    #[test]
    fn config_loads_from_override_path() {
        let dir = TempDir::new().unwrap();
        let cfg_path = dir.path().join("custom-config.json");
        fs::write(
            &cfg_path,
            r#"{"enabled":false,"validationLevel":4,"analyzerSeverityThreshold":"Info","timeoutSeconds":10}"#,
        )
        .unwrap();

        let cfg = load_config(Some(&cfg_path)).unwrap();
        assert!(!cfg.enabled);
        assert_eq!(cfg.validation_level, 4);
        assert_eq!(cfg.analyzer_severity_threshold, "Info");
    }

    #[test]
    fn config_returns_default_when_no_file_found() {
        // Run from temp dir with no config
        let dir = TempDir::new().unwrap();
        let _guard = SetCwd::new(dir.path());
        let cfg = load_config(None).unwrap();
        assert_eq!(cfg.validation_level, 2);
    }

    // ── File discovery tests ─────────────────────────────────────────────

    #[test]
    fn discover_single_cs_file() {
        let dir = TempDir::new().unwrap();
        let cs_file = dir.path().join("Program.cs");
        fs::write(&cs_file, "class Foo {}").unwrap();

        let files = discover_cs_files(&cs_file).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0], cs_file);
    }

    #[test]
    fn discover_rejects_non_cs_file() {
        let dir = TempDir::new().unwrap();
        let txt_file = dir.path().join("readme.txt");
        fs::write(&txt_file, "hello").unwrap();

        let err = discover_cs_files(&txt_file).unwrap_err();
        assert!(err.to_string().contains("not a .cs file"));
    }

    #[test]
    fn discover_walks_directory_recursively() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("src").join("Models");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.path().join("Program.cs"), "").unwrap();
        fs::write(sub.join("User.cs"), "").unwrap();
        fs::write(sub.join("Order.cs"), "").unwrap();
        // Non-cs file should be skipped
        fs::write(sub.join("README.md"), "").unwrap();

        let files = discover_cs_files(dir.path()).unwrap();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|f| f.extension().unwrap() == "cs"));
    }

    #[test]
    fn discover_empty_directory_returns_error() {
        let dir = TempDir::new().unwrap();
        // run_validate errors on empty dir
        let cfg = CsValidatorConfig::default();
        let err = run_validate(dir.path(), ValidationLevel::Syntax, &cfg).unwrap_err();
        assert!(err.to_string().contains("No .cs files found"));
    }

    #[test]
    fn discover_nonexistent_path_returns_error() {
        let err = discover_cs_files(Path::new("/nonexistent/path/foo.cs")).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    // ── Level 1 syntax validation integration ────────────────────────────

    #[test]
    fn level1_passes_valid_cs_file() {
        let dir = TempDir::new().unwrap();
        let cs = dir.path().join("Valid.cs");
        fs::write(
            &cs,
            r#"
namespace MyApp
{
    public class Valid
    {
        public void Run()
        {
            Console.WriteLine("Hello");
        }
    }
}
"#,
        )
        .unwrap();

        let cfg = CsValidatorConfig::default();
        let results = run_validate(&cs, ValidationLevel::Syntax, &cfg).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
        assert!(results[0].diagnostics.is_empty());
    }

    #[test]
    fn level1_fails_unbalanced_braces() {
        let dir = TempDir::new().unwrap();
        let cs = dir.path().join("Bad.cs");
        fs::write(
            &cs,
            r#"
namespace MyApp
{
    public class Bad
    {
        public void Run()
        {
            // missing closing brace
        }

"#,
        )
        .unwrap();

        let cfg = CsValidatorConfig::default();
        let results = run_validate(&cs, ValidationLevel::Syntax, &cfg).unwrap();
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert!(results[0]
            .diagnostics
            .iter()
            .any(|d| d.message.contains("brace") || d.message.contains("delimiter")));
    }

    #[test]
    fn level1_fails_unbalanced_parentheses() {
        let dir = TempDir::new().unwrap();
        let cs = dir.path().join("Parens.cs");
        fs::write(
            &cs,
            r#"
class Foo {
    void Bar() {
        var x = (1 + 2;
    }
}
"#,
        )
        .unwrap();

        let cfg = CsValidatorConfig::default();
        let results = run_validate(&cs, ValidationLevel::Syntax, &cfg).unwrap();
        assert!(!results[0].passed);
    }

    // ── Exit code contract tests ─────────────────────────────────────────

    #[test]
    fn exit_codes_have_correct_values() {
        assert_eq!(EXIT_PASS, 0);
        assert_eq!(EXIT_VALIDATION_FAIL, 1);
        assert_eq!(EXIT_CONFIG_ERROR, 2);
        assert_eq!(EXIT_TIMEOUT, 3);
        assert_eq!(EXIT_MISSING_DEPENDENCY, 4);
    }

    // ── Serialization tests ──────────────────────────────────────────────

    #[test]
    fn validation_result_serializes_to_json() {
        let result = ValidationResult {
            file: PathBuf::from("src/Foo.cs"),
            level: 1,
            passed: false,
            diagnostics: vec![Diagnostic {
                severity: DiagnosticSeverity::Error,
                message: "Unbalanced braces".to_string(),
                line: Some(5),
                column: Some(1),
            }],
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("Unbalanced braces"));
        assert!(json.contains("\"passed\":false"));
        assert!(json.contains("\"level\":1"));
    }

    // ── Helper: temporarily change cwd for config tests ──────────────────

    struct SetCwd {
        prev: PathBuf,
    }

    impl SetCwd {
        fn new(dir: &Path) -> Self {
            let prev = std::env::current_dir().unwrap();
            std::env::set_current_dir(dir).unwrap();
            Self { prev }
        }
    }

    impl Drop for SetCwd {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.prev);
        }
    }
}
