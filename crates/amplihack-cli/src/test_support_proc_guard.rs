//! Issue #2196: catch lib tests that bypass `proc_root()` and read the host
//! process table. This runs with `cargo test --lib`, including inline tests
//! and helpers in test files, rather than relying on the integration guard.

use std::collections::HashMap;
use std::path::Path;
use syn::visit::{self, Visit};

fn test_attrs(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test")
            || (a.path().is_ident("cfg")
                && a.parse_args::<syn::Path>()
                    .is_ok_and(|p| p.is_ident("test")))
    })
}

fn test_file(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "tests")
        || path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| {
            s.starts_with("tests") || s.ends_with("_tests") || s.starts_with("test_support")
        })
}

/// `/proc` itself enumerates strangers. A numeric pid literal names one.
/// Dynamic pid reads are also used for children held by the test; those
/// require the ownership checks described in the guide. Self and global
/// kernel information (loadavg, locks, meminfo, etc.) are not process scans.
fn foreign_process_path(path: &str) -> bool {
    if path.trim_end_matches('/') == "/proc" {
        return true;
    }
    path.strip_prefix("/proc/")
        .and_then(|p| p.split('/').next())
        .is_some_and(|pid| !pid.is_empty() && pid.bytes().all(|c| c.is_ascii_digit()))
}

#[derive(Default)]
struct Guard {
    in_test: bool,
    findings: Vec<String>,
    bindings: HashMap<String, Vec<String>>,
}

impl<'ast> Visit<'ast> for Guard {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let was = self.in_test;
        self.in_test |= test_attrs(&node.attrs);
        visit::visit_item_mod(self, node);
        self.in_test = was;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let was = self.in_test;
        let bindings = std::mem::take(&mut self.bindings);
        self.in_test |= test_attrs(&node.attrs);
        visit::visit_item_fn(self, node);
        self.in_test = was;
        self.bindings = bindings;
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let syn::Pat::Ident(name) = &node.pat
            && let Some(init) = &node.init
        {
            let paths = self.paths(&init.expr);
            self.bindings.insert(name.ident.to_string(), paths);
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if self.in_test
            && let syn::Expr::Path(func) = &*node.func
            && let Some(name) = func.path.segments.last()
            && matches!(
                name.ident.to_string().as_str(),
                "read"
                    | "read_to_string"
                    | "read_dir"
                    | "read_link"
                    | "metadata"
                    | "symlink_metadata"
                    | "open"
            )
        {
            for arg in &node.args {
                let paths = self.paths(arg);
                self.findings.extend(paths);
            }
        }
        visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if self.in_test {
            let paths = match node.method.to_string().as_str() {
                "open" => node.args.first().map(|arg| self.paths(arg)),
                "exists" | "is_file" | "is_dir" | "metadata" | "symlink_metadata" | "read_dir"
                | "read_link" => Some(self.paths(&node.receiver)),
                _ => None,
            };
            self.findings.extend(paths.into_iter().flatten());
        }
        visit::visit_expr_method_call(self, node);
    }

    // Assertions commonly contain the read itself. Parse their expressions
    // too, so wrapping a forbidden read in assert! cannot hide it.
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        use syn::parse::Parser;
        let args = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
        if let Ok(args) = args.parse2(node.tokens.clone()) {
            for arg in &args {
                self.visit_expr(arg);
            }
        }
    }
}

impl Guard {
    fn paths(&self, expr: &syn::Expr) -> Vec<String> {
        let mut paths = ReadPaths {
            found: Vec::new(),
            bindings: &self.bindings,
        };
        paths.visit_expr(expr);
        paths.found
    }
}

struct ReadPaths<'a> {
    found: Vec<String>,
    bindings: &'a HashMap<String, Vec<String>>,
}

impl<'ast> Visit<'ast> for ReadPaths<'_> {
    fn visit_lit_str(&mut self, node: &'ast syn::LitStr) {
        let path = node.value();
        if foreign_process_path(&path) {
            self.found.push(path);
        }
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if let Some(name) = node.path.get_ident()
            && let Some(paths) = self.bindings.get(&name.to_string())
        {
            self.found.extend(paths.iter().cloned());
        }
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        use syn::parse::Parser;
        let args = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated;
        if let Ok(args) = args.parse2(node.tokens.clone()) {
            for arg in &args {
                self.visit_expr(arg);
            }
        }
    }
}

fn scan(src: &str, in_test: bool) -> Vec<String> {
    let file = syn::parse_file(src).expect("parse Rust source for lib-test proc guard");
    let mut guard = Guard {
        in_test,
        ..Guard::default()
    };
    guard.visit_file(&file);
    guard.findings
}

#[test]
fn lib_tests_do_not_read_foreign_host_processes_directly() {
    fn walk(dir: &Path, findings: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, findings);
            } else if path.extension().is_some_and(|s| s == "rs") {
                let src = std::fs::read_to_string(&path).unwrap();
                for finding in scan(&src, test_file(&path)) {
                    findings.push(format!("{}: {finding}", path.display()));
                }
            }
        }
    }
    let mut findings = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut findings,
    );
    assert!(
        findings.is_empty(),
        "lib tests read the host process table directly:\n{}",
        findings.join("\n")
    );
}

#[test]
fn deliberately_introduced_foreign_reads_fail_the_lib_guard() {
    for read in [
        r#"std::fs::read_to_string("/proc/1/stat")"#,
        r#"std::fs::read_dir("/proc")"#,
        r#"std::fs::File::open(Path::new("/proc/1/environ"))"#,
        r##"fs::read(r#"/proc/42/cmdline"#)"##,
        r#"fs::read(format!("/proc/1/{}", "stat"))"#,
        r#"assert!(fs::read("/proc/1/stat").is_ok())"#,
        r#"std::fs::OpenOptions::new().read(true).open("/proc/1/stat")"#,
        r#"Path::new("/proc/1/stat").exists()"#,
    ] {
        let body = format!("fn example() {{ let _ = {read}; }}");
        assert!(!scan(&body, true).is_empty(), "test file missed {read}");
        assert!(
            !scan(&format!("#[cfg(test)] mod tests {{ {body} }}"), false).is_empty(),
            "inline module missed {read}"
        );
        assert!(
            !scan(&format!("#[test] {body}"), false).is_empty(),
            "test function missed {read}"
        );
        assert!(
            scan(&body, false).is_empty(),
            "production reader is outside this guard"
        );
    }
    assert_eq!(
        scan(
            r#"#[test] fn indirect() {
            let root = PathBuf::from("/proc");
            let path = root.join("1/stat");
            let _ = fs::read(path);
        }"#,
            false
        ),
        ["/proc"]
    );
    assert!(
        scan(
            r#"#[test] fn own_process() { let _ = fs::read("/proc/self/stat"); }"#,
            false
        )
        .is_empty()
    );
    assert!(
        scan(
            r#"#[test] fn synthetic() { let _ = fs::read(proc_root().join("1/stat")); }"#,
            false
        )
        .is_empty()
    );
}
