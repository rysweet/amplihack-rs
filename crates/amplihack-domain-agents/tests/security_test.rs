use amplihack_domain_agents::{
    AuditReport, SecurityAuditor, SecurityConfig, Vulnerability,
};
use chrono::Utc;

// ── Construction & accessors (PASS) ─────────────────────────────────────────

#[test]
fn new_with_config_stores_config() {
    let cfg = SecurityConfig {
        scan_depth: 5,
        severity_threshold: "high".to_string(),
        include_info: true,
    };
    let auditor = SecurityAuditor::new(cfg.clone());
    let got = auditor.config();
    assert_eq!(got.scan_depth, 5);
    assert_eq!(got.severity_threshold, "high");
    assert!(got.include_info);
}

#[test]
fn with_defaults_uses_default_config() {
    let auditor = SecurityAuditor::with_defaults();
    let cfg = auditor.config();
    assert_eq!(cfg.scan_depth, 3);
    assert_eq!(cfg.severity_threshold, "medium");
    assert!(!cfg.include_info);
}

#[test]
fn config_accessor_returns_config() {
    let cfg = SecurityConfig {
        scan_depth: 1,
        severity_threshold: "low".to_string(),
        include_info: false,
    };
    let auditor = SecurityAuditor::new(cfg);
    let got = auditor.config();
    assert_eq!(got.scan_depth, 1);
    assert_eq!(got.severity_threshold, "low");
    assert!(!got.include_info);
}

// ── audit (todo → should_panic) ─────────────────────────────────────────────

#[test]
fn audit_basic_code() {
    let auditor = SecurityAuditor::with_defaults();
    let report = auditor.audit("fn main() { println!(\"hello\"); }").unwrap();
    assert!(report.vulnerabilities.is_empty());
    assert!((report.risk_score - 0.0).abs() < f64::EPSILON);
}

#[test]
fn audit_empty_code() {
    let auditor = SecurityAuditor::with_defaults();
    let report = auditor.audit("").unwrap();
    assert!(report.vulnerabilities.is_empty());
}

#[test]
fn audit_detects_unsafe() {
    let auditor = SecurityAuditor::with_defaults();
    let report = auditor.audit("unsafe { std::ptr::null::<u8>().read() }").unwrap();
    assert!(!report.vulnerabilities.is_empty());
    assert!(report.risk_score > 0.0);
}

// ── scan_vulnerabilities (todo → should_panic) ──────────────────────────────

#[test]
fn scan_vulnerabilities_basic() {
    let auditor = SecurityAuditor::with_defaults();
    let vulns = auditor.scan_vulnerabilities("unsafe { std::ptr::null::<u8>().read() }").unwrap();
    assert!(!vulns.is_empty());
    assert!(vulns.iter().any(|v| v.description.contains("unsafe")));
}

#[test]
fn scan_vulnerabilities_safe_code() {
    let auditor = SecurityAuditor::with_defaults();
    let vulns = auditor.scan_vulnerabilities("let x: i32 = 42;").unwrap();
    assert!(vulns.is_empty());
}

// ── risk_assessment (todo → should_panic) ───────────────────────────────────

#[test]
fn risk_assessment_basic() {
    let auditor = SecurityAuditor::with_defaults();
    let assessment = auditor.risk_assessment("fn safe() {}").unwrap();
    assert_eq!(assessment.overall_risk, "low");
    assert!((assessment.risk_score - 0.0).abs() < f64::EPSILON);
}

#[test]
fn risk_assessment_high_risk() {
    let auditor = SecurityAuditor::with_defaults();
    let code = "unsafe { libc::system(cmd.as_ptr()) }; password = \"secret\"; eval(input); innerHTML = data; let query = format!(\"SELECT * FROM users\")";
    let assessment = auditor.risk_assessment(code).unwrap();
    assert!(assessment.risk_score > 0.0);
    assert_ne!(assessment.overall_risk, "low");
}

// ── serde roundtrip (PASS) ──────────────────────────────────────────────────

#[test]
fn security_config_serde_roundtrip() {
    let cfg = SecurityConfig {
        scan_depth: 7,
        severity_threshold: "critical".to_string(),
        include_info: true,
    };
    let json = serde_json::to_string(&cfg).expect("serialize");
    let back: SecurityConfig = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(cfg, back);
}

#[test]
fn vulnerability_serde_roundtrip() {
    let v = Vulnerability {
        id: "VULN-001".to_string(),
        severity: "high".to_string(),
        description: "Buffer overflow in parser".to_string(),
        location: "src/parser.rs:42".to_string(),
        recommendation: "Use bounds-checked indexing".to_string(),
    };
    let json = serde_json::to_string(&v).expect("serialize");
    let back: Vulnerability = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(v, back);
}

#[test]
fn audit_report_serde_roundtrip() {
    let report = AuditReport {
        vulnerabilities: vec![Vulnerability {
            id: "VULN-002".to_string(),
            severity: "medium".to_string(),
            description: "SQL injection risk".to_string(),
            location: "src/db.rs:10".to_string(),
            recommendation: "Use parameterized queries".to_string(),
        }],
        risk_score: 6.5,
        summary: "Moderate risk detected".to_string(),
        scanned_at: Utc::now(),
    };
    let json = serde_json::to_string(&report).expect("serialize");
    let back: AuditReport = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(report.vulnerabilities, back.vulnerabilities);
    assert!((report.risk_score - back.risk_score).abs() < f64::EPSILON);
    assert_eq!(report.summary, back.summary);
}
