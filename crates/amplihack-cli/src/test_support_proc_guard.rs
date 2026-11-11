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
    fn visit_file(&mut self, node: &'ast syn::File) {
        self.bind_constants(&node.items);
        visit::visit_file(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        let was = self.in_test;
        let bindings = self.bindings.clone();
        self.in_test |= test_attrs(&node.attrs);
        if let Some((_, items)) = &node.content {
            self.bind_constants(items);
        }
        visit::visit_item_mod(self, node);
        self.in_test = was;
        self.bindings = bindings;
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let was = self.in_test;
        let bindings = self.bindings.clone();
        self.in_test |= test_attrs(&node.attrs);
        visit::visit_item_fn(self, node);
        self.in_test = was;
        self.bindings = bindings;
    }

    fn visit_block(&mut self, node: &'ast syn::Block) {
        let bindings = self.bindings.clone();
        self.bind_constants(node.stmts.iter().filter_map(|stmt| match stmt {
            syn::Stmt::Item(item) => Some(item),
            _ => None,
        }));
        visit::visit_block(self, node);
        self.bindings = bindings;
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        // A type annotation wraps the identifier in Pat::Type.
        let mut pat = &node.pat;
        while let syn::Pat::Type(typed) = pat {
            pat = &typed.pat;
        }
        let binding = if let syn::Pat::Ident(name) = pat
            && let Some(init) = &node.init
        {
            Some((name.ident.to_string(), self.paths(&init.expr)))
        } else {
            None
        };
        // The initializer still sees the previous binding when shadowing.
        visit::visit_local(self, node);
        if let Some((name, paths)) = binding {
            self.bindings.insert(name, paths);
        }
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
    fn bind_constants<'a>(&mut self, items: impl IntoIterator<Item = &'a syn::Item>) {
        let constants: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                syn::Item::Const(item) => Some(item),
                _ => None,
            })
            .collect();
        // Items are in scope before their declaration, and an alias may
        // reference a later constant. At most N passes resolve N constants.
        for item in &constants {
            self.bindings.insert(item.ident.to_string(), Vec::new());
        }
        for _ in 0..constants.len() {
            let mut changed = false;
            for item in &constants {
                let paths = self.paths(&item.expr);
                let binding = self.bindings.get_mut(&item.ident.to_string()).unwrap();
                changed |= *binding != paths;
                *binding = paths;
            }
            if !changed {
                break;
            }
        }
    }

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
fn typed_bindings_and_constants_fail_the_lib_guard() {
    for body in [
        r#"let root: &str = "/proc"; let _ = std::fs::read_dir(root);"#,
        r#"let path: PathBuf = PathBuf::from("/proc/1/stat"); let _ = fs::read(path);"#,
        r#"const ROOT: &str = "/proc"; let _ = std::fs::read_dir(ROOT);"#,
        r#"let _ = std::fs::read_dir(ROOT); const ROOT: &str = "/proc";"#,
    ] {
        let function = format!("fn example() {{ {body} }}");
        for (src, in_test) in [
            (function.clone(), true),
            (format!("#[test] {function}"), false),
            (format!("#[cfg(test)] mod tests {{ {function} }}"), false),
        ] {
            assert_eq!(scan(&src, in_test).len(), 1, "missed {src}");
        }
        assert!(scan(&function, false).is_empty());
    }

    for items in [
        r#"const ROOT: &str = "/proc";
           #[test] fn leaks() { let _ = std::fs::read_dir(ROOT); }"#,
        r#"#[test] fn leaks() { let _ = std::fs::read_dir(ROOT); }
           const ROOT: &str = "/proc";"#,
        r#"const ROOT: &str = ALIAS;
           const ALIAS: &str = "/proc";
           #[test] fn leaks() {
               let root: &str = ROOT;
               let _ = std::fs::read_dir(root);
           }"#,
    ] {
        assert_eq!(scan(items, false), ["/proc"], "missed {items}");
        assert_eq!(
            scan(&format!("#[cfg(test)] mod tests {{ {items} }}"), false),
            ["/proc"],
            "missed inline module: {items}"
        );
    }
    assert_eq!(
        scan(
            r#"const ROOT: &str = "/proc";
               #[cfg(test)] mod tests {
                   use super::*;
                   #[test] fn leaks() { let _ = std::fs::read_dir(ROOT); }
               }"#,
            false
        ),
        ["/proc"]
    );
}

#[test]
fn bindings_and_constants_respect_scopes() {
    assert_eq!(
        scan(
            r#"#[test] fn leaks() {
                   let root: &str = "/proc";
                   { let root: &str = "/tmp/fake"; let _ = fs::read_dir(root); }
                   let _ = fs::read_dir(root);
               }"#,
            false
        ),
        ["/proc"]
    );
    for src in [
        r#"const ROOT: &str = "/proc";
           #[test] fn safe() { const ROOT: &str = "/tmp/fake"; let _ = fs::read_dir(ROOT); }"#,
        r#"const ROOT: &str = "/proc";
           #[test] fn safe() { let _ = fs::read_dir(ROOT); const ROOT: &str = "/tmp/fake"; }"#,
        r#"#[test] fn unused() { let root: &str = "/proc"; }
           #[test] fn safe() { let root: &str = "/tmp/fake"; let _ = fs::read_dir(root); }"#,
        r#"mod unrelated { const ROOT: &str = "/proc"; }
           mod sibling {
               const ROOT: &str = "/tmp/fake";
               #[test] fn safe() { let _ = fs::read_dir(ROOT); }
           }"#,
        r#"const OWN: &str = "/proc/self/stat";
           #[test] fn safe() { let path: &str = OWN; let _ = fs::read(path); }"#,
        r#"const ROOT: &str = "/proc";
           fn production() { let _ = fs::read_dir(ROOT); }"#,
    ] {
        assert!(scan(src, false).is_empty(), "unexpected finding: {src}");
    }
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
