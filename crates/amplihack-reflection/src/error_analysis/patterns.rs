//! Error pattern catalog used by the contextual error analyzer.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternEntry {
    pub keywords: &'static [&'static str],
    pub category: super::ErrorCategory,
    pub severity: super::Severity,
    pub suggestion: &'static str,
    pub steps: &'static [&'static str],
}

use super::{ErrorCategory, Severity};

/// Static, deterministic catalog of error patterns. Order matters: more
/// specific patterns appear before less specific ones.
pub const PATTERNS: &[PatternEntry] = &[
    PatternEntry {
        keywords: &["modulenotfounderror", "no module named"],
        category: ErrorCategory::ImportError,
        severity: Severity::High,
        suggestion: "Install missing Python package or fix import paths",
        steps: &[
            "Add missing package to requirements.txt or pyproject.toml",
            "Install package with pip install <package>",
            "Verify virtual environment activation",
        ],
    },
    PatternEntry {
        keywords: &["importerror", "cannot import name"],
        category: ErrorCategory::ImportError,
        severity: Severity::High,
        suggestion: "Check the symbol exists in the target module and is exported",
        steps: &[
            "Review the module's __all__ or pub exports",
            "Verify versions match",
        ],
    },
    PatternEntry {
        keywords: &["permissionerror", "permission denied", "access denied"],
        category: ErrorCategory::Permission,
        severity: Severity::High,
        suggestion: "Fix file/directory permissions or run with appropriate access rights",
        steps: &[
            "Check file permissions with os.access() before operations",
            "Use appropriate file modes when opening files",
        ],
    },
    PatternEntry {
        keywords: &["filenotfounderror", "no such file", "file not found"],
        category: ErrorCategory::FileMissing,
        severity: Severity::High,
        suggestion: "Check file paths and ensure files exist before operations",
        steps: &[
            "Use pathlib.Path.exists() before file operations",
            "Add try-catch blocks around file I/O operations",
        ],
    },
    PatternEntry {
        keywords: &["timeout", "timed out", "connection timeout"],
        category: ErrorCategory::Network,
        severity: Severity::Medium,
        suggestion: "Increase timeout values or implement retry with exponential backoff",
        steps: &[
            "Increase timeout parameters in network calls",
            "Add connection pooling for better reliability",
        ],
    },
    PatternEntry {
        keywords: &["connectionerror", "connection refused", "network error"],
        category: ErrorCategory::Network,
        severity: Severity::High,
        suggestion: "Check network connectivity and service availability",
        steps: &[
            "Add network connectivity checks before API calls",
            "Implement retry logic with exponential backoff",
        ],
    },
    PatternEntry {
        keywords: &["http error", "api error", "status code"],
        category: ErrorCategory::Network,
        severity: Severity::Medium,
        suggestion: "Add error handling for API responses and implement retry logic",
        steps: &[
            "Add HTTP status code checking",
            "Implement proper error response handling",
        ],
    },
    PatternEntry {
        keywords: &["syntaxerror", "invalid syntax"],
        category: ErrorCategory::Syntax,
        severity: Severity::High,
        suggestion: "Fix syntax errors using IDE or linter tools",
        steps: &["Run code through linter (flake8, pylint, black)"],
    },
    PatternEntry {
        keywords: &["typeerror"],
        category: ErrorCategory::Type,
        severity: Severity::Medium,
        suggestion: "Add type checking and input validation",
        steps: &["Add type hints", "Validate input types before processing"],
    },
    PatternEntry {
        keywords: &["indexerror", "list index out of range"],
        category: ErrorCategory::Index,
        severity: Severity::Medium,
        suggestion: "Add bounds checking before accessing list/array elements",
        steps: &[
            "Check list length before access",
            "Use enumerate() for safer iteration",
        ],
    },
    PatternEntry {
        keywords: &["keyerror"],
        category: ErrorCategory::Key,
        severity: Severity::Medium,
        suggestion: "Use safe dictionary access methods like .get() with defaults",
        steps: &[
            "Use dict.get() with default values",
            "Check key existence with 'in'",
        ],
    },
    PatternEntry {
        keywords: &["valueerror"],
        category: ErrorCategory::Value,
        severity: Severity::Medium,
        suggestion: "Add input validation and handle edge cases",
        steps: &["Add input validation before processing"],
    },
    PatternEntry {
        keywords: &["command not found", "executable not found"],
        category: ErrorCategory::CommandMissing,
        severity: Severity::High,
        suggestion: "Install required command or check PATH environment variable",
        steps: &[
            "Install required system package",
            "Check PATH environment variable",
        ],
    },
    PatternEntry {
        keywords: &["memory error", "out of memory"],
        category: ErrorCategory::Memory,
        severity: Severity::High,
        suggestion: "Optimize memory usage or increase available system memory",
        steps: &[
            "Process data in smaller chunks",
            "Use generators for large datasets",
        ],
    },
];

pub fn match_best(content_lower: &str) -> Option<(&'static PatternEntry, usize)> {
    let mut best: Option<(&PatternEntry, usize)> = None;
    for p in PATTERNS {
        let mut score = 0;
        for kw in p.keywords {
            if content_lower.contains(kw) {
                score += 1;
            }
        }
        if score > 0 && best.map(|(_, s)| score > s).unwrap_or(true) {
            best = Some((p, score));
        }
    }
    best
}
