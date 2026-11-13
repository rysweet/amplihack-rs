//! Standalone Rust executable compiled by support/issue_2550.rs.
//! External-service state is private to each test. Unknown operations fail.
use std::{
    env, fs,
    io::{self, Read, Write},
    path::PathBuf,
    process::{self, Command},
};

fn state() -> PathBuf {
    env::var_os("ISSUE_2550_STATE").unwrap().into()
}
fn read(name: &str) -> String {
    fs::read_to_string(state().join(name)).unwrap_or_default()
}
fn write(name: &str, body: &str) {
    fs::write(state().join(name), body).unwrap();
}
fn event(text: &str) {
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(state().join("events"))
            .unwrap(),
        "{text}"
    )
    .unwrap();
}
fn json(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn value<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == key)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}
fn stdin() -> String {
    let mut s = String::new();
    io::stdin().read_to_string(&mut s).unwrap();
    s
}
fn fail(message: &str) -> ! {
    eprintln!("fixture refused: {message}");
    process::exit(97)
}

fn main() {
    let argv: Vec<String> = env::args().collect();
    let name = PathBuf::from(&argv[0])
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let args = &argv[1..];
    match name.as_str() {
        "timeout" => {
            // The production hosting wrapper uses timeout. Fixtures have
            // deterministic adapters, so execute that operation synchronously
            // without starting a timer or admitting an unknown executable.
            if args.len() < 2
                || !args[0].bytes().all(|b| b.is_ascii_digit())
                || !matches!(args[1].as_str(), "gh" | "az")
            {
                fail("unexpected timeout invocation");
            }
            let status = Command::new(&args[1]).args(&args[2..]).status().unwrap();
            process::exit(status.code().unwrap_or(1));
        }
        "pre-commit" | "pre-push" => {
            event(&name);
            if state().join(format!("reject-{name}")).exists() {
                eprintln!("issue-2550-recording-hook: Failed\nhook id: issue-2550-recording-hook");
                process::exit(1);
            }
        }
        "amplihack" => {
            if args.first().map(String::as_str) == Some("workflow") {
                if state().join("policy-helper-fails").exists() {
                    process::exit(1);
                }
                let rc = Command::new(env::var_os("ISSUE_2550_REAL_AMPLIHACK").unwrap())
                    .args(args)
                    .status()
                    .unwrap();
                process::exit(rc.code().unwrap_or(1));
            }
            if args.first().map(String::as_str) == Some("hygiene") {
                return;
            }
            // A rejected hook cannot be 'repaired' by an external model here.
            eprintln!("fixture: orchestration unavailable");
            process::exit(79);
        }
        "classifier" | "workflow_issue_reference.sh" => {
            event("classifier");
            print!("{}", read("classification"));
            if state().join("classifier-fails").exists() {
                process::exit(1);
            }
        }
        "checkpoint" => {
            event("checkpoint");
            if !state().join("continue").exists() {
                eprintln!("You've hit your session limit · resets 8:30am (America/Los_Angeles)");
                process::exit(1);
            }
            println!("{{\"continued\":true}}");
        }
        "boundary" => {
            let policy = env::var("RECIPE_VAR_pr_issue_link").unwrap_or_else(|_| "MISSING".into());
            event(&format!("boundary:{policy}"));
            match args.first().map(String::as_str) {
                Some("inspect") => println!("{{\"policy\":{}}}", json(&policy)),
                Some("publish") => {
                    for name in ["commit-command", "publish-command"] {
                        let status = Command::new("bash")
                            .args(["-euo", "pipefail", "-c", &read(name)])
                            .status()
                            .unwrap();
                        if !status.success() {
                            process::exit(status.code().unwrap_or(1));
                        }
                    }
                }
                _ => fail("unknown recipe boundary action"),
            }
        }
        "gh" => {
            let operation = args
                .iter()
                .take(2)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            event(&operation);
            // NUL-delimited argv permits exact scope/stdin assertions.
            let mut log = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(state().join("gh.argv"))
                .unwrap();
            for arg in args {
                log.write_all(arg.as_bytes()).unwrap();
                log.write_all(&[0]).unwrap();
            }
            log.write_all(&[0]).unwrap();
            if state()
                .join(format!("stall-{}", operation.replace(' ', "-")))
                .exists()
            {
                // No child commands or real services: only the adapter stalls.
                // The production subprocess deadline must terminate it.
                loop {
                    std::thread::park();
                }
            }
            if state()
                .join(format!("fail-{}", operation.replace(' ', "-")))
                .exists()
            {
                eprintln!("injected GitHub operation failure");
                process::exit(1);
            }
            match operation.as_str() {
                "pr list" => println!("[]"),
                "issue view" => {
                    println!("{{\"title\":\"fix: select issue reference\",\"state\":\"OPEN\"}}")
                }
                "repo view" => println!("main"),
                "pr create" => {
                    let file = value(args, "--body-file")
                        .unwrap_or_else(|| fail("body must use --body-file"));
                    let body = if file == "-" {
                        stdin()
                    } else {
                        fs::read_to_string(file).unwrap()
                    };
                    write("body", &body);
                    write("created", "yes");
                    println!("https://github.com/owner/repo/pull/99");
                }
                "pr view" => {
                    let seen = read("views").parse::<usize>().unwrap_or(0) + 1;
                    write("views", &seen.to_string());
                    if seen > 1 && state().join("stall-readback").exists() {
                        loop {
                            std::thread::park();
                        }
                    }
                    if seen > 1 && state().join("fail-readback").exists() {
                        process::exit(1);
                    }
                    let body = if seen > 1 && state().join("wrong-readback").exists() {
                        "changed concurrently".into()
                    } else {
                        read("body")
                    };
                    if value(args, "--jq").is_some() {
                        print!("{body}");
                    } else {
                        println!(
                            "{{\"title\":\"fix: select issue reference\",\"body\":{},\"number\":99,\"state\":\"OPEN\",\"headRefName\":\"feat/issue-2550-fixture\",\"baseRefName\":\"main\",\"isCrossRepository\":false}}",
                            json(&body)
                        );
                    }
                }
                "pr edit" => {
                    let file = value(args, "--body-file")
                        .unwrap_or_else(|| fail("body must use --body-file"));
                    let body = if file == "-" {
                        stdin()
                    } else {
                        fs::read_to_string(file).unwrap()
                    };
                    write("submitted", &body);
                    write("body", &body);
                }
                "pr merge" => {
                    if value(args, "--match-head-commit") != Some(read("expected-head").as_str()) {
                        fail("merge requires the expected head");
                    }
                    if !args.iter().any(|a| a == "--squash")
                        || value(args, "--body-file") != Some("-")
                    {
                        fail("squash body must arrive on stdin");
                    }
                    write(
                        "merge-subject",
                        value(args, "--subject")
                            .unwrap_or_else(|| fail("missing explicit subject")),
                    );
                    write("merge-body", &stdin());
                }
                _ => fail(&format!("unknown gh operation {args:?}")),
            }
        }
        "az" => {
            event("az");
            if args.starts_with(&["repos".into(), "pr".into(), "list".into()]) {
                println!("[]");
            } else if args.starts_with(&["repos".into(), "pr".into(), "create".into()]) {
                write("az.argv", &format!("{args:?}"));
                println!(
                    "{{\"pullRequestId\":99,\"status\":\"active\",\"url\":\"https://dev.azure.com/owner/project/_git/repo/pullrequest/99\"}}"
                );
            } else {
                fail("unknown az operation");
            }
        }
        _ => fail(&format!("unexpected executable {name}")),
    }
}
