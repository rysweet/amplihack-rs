use amplihack_domain_agents::{
    AuditReport, RiskAssessment, SecurityAuditor, SecurityConfig, Vulnerability,
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
#[should_panic]
fn audit_basic_code() {
    let auditor = SecurityAuditor::with_defaults();
    let _ = auditor.audit("fn main() { println!(\"hello\"); }");
}

#[test]
#[should_panic]
fn audit_empty_code() {
    let auditor = SecurityAuditor::with_defaults();
    let _ = auditor.audit("");
}

// ── scan_vulnerabilities (todo → should_panic) ──────────────────────────────

#[test]
#[should_panic]
fn scan_vulnerabilities_basic() {
    let auditor = SecurityAuditor::with_defaults();
    let _ = auditor.scan_vulnerabilities("unsafe { std::ptr::null::<u8>().read() }");
}

#[test]
#[should_panic]
fn scan_vulnerabilities_safe_code() {
    let auditor = SecurityAuditor::with_defaults();
    let _ = auditor.scan_vulnerabilities("let x: i32 = 42;");
}

// ── risk_assessment (todo → should_panic) ───────────────────────────────────

#[test]
#[should_panic]
fn risk_assessment_basic() {
    let auditor = SecurityAuditor::with_defaults();
    let _ = auditor.risk_assessment("fn safe() {}");
}

#[test]
#[should_panic]
fn risk_assessment_high_risk() {
    let auditor = SecurityAuditor::with_defaults();
    let _ = auditor.risk_assessment("unsafe { libc::system(cmd.as_ptr()) }");
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
