// Issue #1642: the one case table for both GitHub origin parsers.
//
// `include!`d by the integration tests (bash parser,
// amplifier-bundle/tools/workflow_github_repo.sh) and by the unit tests of the
// Rust twin (crates/amplihack-cli/src/commands/multitask/
// issue_1642_repo_identity.rs). If either parser is changed without a matching
// row, one of the two suites fails, so they cannot drift apart.
//
// Plain `//` comments only: this file is `include!`d as items, where inner
// doc comments are not allowed. Every token here is fake.
//
// `Some("o/r")`: the parser prints exactly that owner/repo.
// `None`: the parser prints nothing and fails (bash: rc 1, no stderr).

#[allow(dead_code)]
pub const ORIGIN_CASES: &[(&str, Option<&str>)] = &[
    // ---- Real origin forms from the issue: must keep parsing. ----
    ("https://github.com/o/r", Some("o/r")),
    ("https://github.com/o/r.git", Some("o/r")),
    ("http://github.com/o/r", Some("o/r")),
    ("git@github.com:o/r", Some("o/r")),
    ("git@github.com:o/r.git", Some("o/r")),
    ("ssh://git@github.com/o/r", Some("o/r")),
    ("ssh://git@github.com/o/r.git", Some("o/r")),
    ("https://tok@github.com/o/r", Some("o/r")),
    ("https://x:tok@github.com/o/r", Some("o/r")),
    ("http://x:tok@github.com/o/r", Some("o/r")),
    // Real token shapes fit the userinfo allow-list.
    (
        "https://x-access-token:ghp_FAKE1642@github.com/o/r.git",
        Some("o/r"),
    ),
    ("https://tok%40x@github.com/o/r", Some("o/r")),
    // ---- Normalisation: cut ?, cut #, one trailing /, one .git. ----
    ("https://github.com/o/r/", Some("o/r")),
    ("https://github.com/o/r.git/", Some("o/r")),
    ("git@github.com:o/r/", Some("o/r")),
    ("https://github.com/o/r?tab=readme", Some("o/r")),
    ("https://github.com/o/r#readme", Some("o/r")),
    ("https://github.com/o/r.git.git", Some("o/r.git")),
    // Validation runs after .git stripping: `..git` gives `.`, rejected;
    // `.git.git` strips once to `.git`, a valid segment (SR3).
    ("https://github.com/o/..git", None),
    ("https://github.com/o/.git.git", Some("o/.git")),
    (
        "https://github.com/my-org/my.project_x.git",
        Some("my-org/my.project_x"),
    ),
    // ---- Path shape: exactly owner/repo, or owner/repo/pull/<n> with <n>
    // matching ^[1-9][0-9]{0,9}$. The PR form is accepted because callers
    // pass PR URLs; anything else is rejected so the reported identity
    // always equals the repository git and curl fetch after removing dot
    // segments. ----
    ("https://github.com/o/r/pull/1", Some("o/r")),
    ("https://github.com/o/r/pull/5", Some("o/r")),
    ("https://tok@github.com/o/r/pull/5", Some("o/r")),
    ("git@github.com:o/r/pull/5", Some("o/r")),
    ("ssh://git@github.com/o/r/pull/5", Some("o/r")),
    ("https://github.com/o/r/pull/5/", Some("o/r")),
    ("https://github.com/o/r/pull/5?w=1", Some("o/r")),
    ("https://github.com/o/r/pull/5#issuecomment-1", Some("o/r")),
    ("https://github.com/o/r/pull/1234567890", Some("o/r")),
    ("https://github.com/o/r/tree/main", None),
    ("https://github.com/o/r/extra", None),
    ("https://github.com/o/r/../../x/y", None),
    ("https://github.com/o/r/./x", None),
    ("https://github.com/o/r/pull/../../x/y", None),
    ("https://github.com/o/r/pull/5/../../../x/y", None),
    ("https://github.com/o/r/pull/5/files", None),
    ("https://github.com/o/r/pull/", None),
    ("https://github.com/o/r/pull", None),
    ("https://github.com/o/r/pull/0", None),
    ("https://github.com/o/r/pull/05", None),
    ("https://github.com/o/r/pull/-5", None),
    ("https://github.com/o/r/pull/5a", None),
    ("https://github.com/o/r/pull/12345678901", None),
    ("https://github.com/o/r/pulls/5", None),
    ("https://github.com/o/r/issues/5", None),
    ("https://github.com/o/r/Pull/5", None),
    ("https://github.com/o/r//", None),
    ("https://github.com/o//r", None),
    ("git@github.com:o/r/extra", None),
    ("ssh://git@github.com/o/r/../../x/y", None),
    ("https://tok@github.com/o/r/../../x/y", None),
    // `.git` is stripped only from the end of the path, so in the PR form it
    // would survive in the repo segment; that form is rejected rather than
    // reported as `o/r.git`.
    ("https://github.com/o/r.git/pull/5", None),
    ("https://tok@github.com/o/r.git/pull/5", None),
    // ---- Accepted but safe: git still contacts github.com. ----
    ("https://github.com/o/r?x=@evil.example/a/b", Some("o/r")),
    ("https://github.com/o/r#@evil.example", Some("o/r")),
    // Extra segments are now a path-shape rejection, wherever the `@` sits.
    ("https://github.com/o/r/x@github.com/a/b", None),
    // %2f stays percent-encoded inside the userinfo; the host is github.com.
    ("https://evil.example%2f@github.com/o/r", Some("o/r")),
    // ---- The four URLs from issue #1642: git contacts evil.example. ----
    ("https://evil.example/x@github.com/a/b", None),
    ("https://evil.example#@github.com/o/r", None),
    ("https://evil.example?@github.com/o/r", None),
    (r"https://evil.example\@github.com/o/r", None),
    (r"https://x:tok@evil.example\@github.com/o/r", None),
    // ---- Userinfo: text before the FIRST @github.com/, allow-list only. ----
    ("https://a@github.com/x@github.com/o/r", None),
    ("https://a@b@github.com/o/r", None),
    ("https://@github.com/o/r", None),
    ("https://ev il@github.com/o/r", None),
    ("https://evil\t@github.com/o/r", None),
    ("https://evil\n@github.com/o/r", None),
    ("https://evil\r@github.com/o/r", None),
    ("https://\u{e9}vil@github.com/o/r", None),
    // ---- Segments. ----
    ("https://github.com/./r", None),
    ("https://github.com/o/..", None),
    ("https://github.com/../r", None),
    ("https://github.com/o/.", None),
    ("https://github.com/o", None),
    ("https://github.com/o/", None),
    ("https://github.com/o/.git", None),
    ("https://github.com//r", None),
    ("git@github.com:/o/r", None),
    ("https://github.com/o$x/r", None),
    ("https://github.com/o;x/r", None),
    ("https://github.com/~o/r", None),
    ("https://github.com/o/r$(id)", None),
    ("https://github.com/-o/r", None),
    ("git@github.com:-o/r", None),
    ("https://github.com/\u{f3}wner/r", None),
    // Newlines never split one origin into two.
    ("https://github.com/o/r\nhttps://github.com/x/y", None),
    ("https://github.com/o/r\n", None),
    ("https://github.com/o\n/r", None),
    // ---- No trim; exact, case-sensitive prefixes; github.com only. ----
    (" https://github.com/o/r", None),
    ("https://github.com/o/r ", None),
    ("HTTPS://github.com/o/r", None),
    ("https://GitHub.com/o/r", None),
    ("https://www.github.com/o/r", None),
    ("https://github.com.evil.example/o/r", None),
    ("https://evil.example/github.com/o/r", None),
    ("git@github.com.evil.example:o/r", None),
    ("ssh://git@evil.example/o/r", None),
    ("https://gitlab.com/o/r", None),
    ("file:///github.com/o/r", None),
    // GitHub Enterprise hosts are rejected by design.
    ("https://github.example.com/o/r", None),
    ("git@github.example.com:o/r", None),
    // Ports are rejected.
    ("https://github.com:443/o/r", None),
    ("https://tok@github.com:443/o/r", None),
    ("ssh://git@github.com:22/o/r", None),
    ("", None),
];
