//! Issue #1753: a refused `profile switch` names the refusal once.
//! Reuses the #1753 login-identity fixture: fake credentials, a loopback
//! identity endpoint and an isolated home under the task-owned TMPDIR.

#![cfg(unix)]

use crate::issue_1753_login_identity::{fixture, run};
use crate::support::profile_rotate::{account_json, private_file, text};

#[test]
fn refused_switch_prints_swap_refused_exactly_once() {
    let (f, ep) = fixture();
    let before = f.logins();
    private_file(
        &f.root.join(".auto-switch.d/expected/work.json"),
        &serde_json::json!({"account": "uuid-spare", "org": null}).to_string(),
    );
    private_file(
        &f.home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": account_json("spare")}).to_string(),
    );
    let out = run(&f, &ep, &["profile", "switch", "extra"]);
    assert!(!out.status.success(), "the credential conflict is refused");
    let stderr = text(&out.stderr);
    assert_eq!(
        stderr.matches("swap refused:").count(),
        1,
        "the refusal is named once: {stderr}"
    );
    assert_eq!(f.logins(), before);
}
