//! Sets `<root>/.default` directly, as `profile switch NAME` did before
//! issue #1691. Since #1691 `switch NAME` also moves the live login and pins
//! rotation, so a test that only needs a default pointer writes it here.

#![allow(dead_code)]

use std::fs;
use std::path::Path;

/// Writes `<name>\n` to `<root>/.default`, mode 0600 on Unix, the way
/// `pointer::write` does. The profile directory must already exist.
pub fn set_default(root: &Path, name: &str) {
    assert!(
        root.join(name).is_dir(),
        "profile {name} must exist before it is made the default"
    );
    let path = root.join(".default");
    fs::write(&path, format!("{name}\n")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

/// Distinct task-owned fake logins plus a bounded loopback identity reader.
/// Keep the returned guard alive and configure each command after env_clear.
#[cfg(unix)]
pub fn give_logins(home: &Path, root: &Path, live: &str, stored: &[&str]) -> LoginIdentities {
    use std::os::unix::fs::PermissionsExt;
    assert!(home.starts_with(std::env::temp_dir()));
    assert!(root.starts_with(home));
    let canonical_home = home.canonicalize().unwrap();
    assert!(canonical_home.starts_with(std::env::temp_dir().canonicalize().unwrap()));
    assert!(root.canonicalize().unwrap().starts_with(&canonical_home));
    let private = |p: &Path, body: &serde_json::Value| {
        fs::write(p, body.to_string()).unwrap();
        fs::set_permissions(p, fs::Permissions::from_mode(0o600)).unwrap();
    };
    let names: Vec<_> = std::iter::once(live)
        .chain(stored.iter().copied())
        .collect();
    let identities = LoginIdentities::new(&names);
    let account = |name: &str| {
        serde_json::json!({
            "accountUuid": format!("uuid-{name}"), "emailAddress": format!("{name}@example.com"),
            "organizationUuid": "fake-default1753-organization"
        })
    };
    let login = |name: &str| {
        serde_json::json!({"claudeAiOauth": {
            "accessToken": format!("fake-default1753-{name}"),
            "refreshToken": "fake-default1753-unused-refresh", "expiresAt": 4_102_444_800_000_i64
        }})
    };
    fs::set_permissions(home, fs::Permissions::from_mode(0o700)).unwrap();
    let dot_claude = home.join(".claude");
    fs::create_dir_all(&dot_claude).unwrap();
    fs::set_permissions(&dot_claude, fs::Permissions::from_mode(0o700)).unwrap();
    private(&dot_claude.join(".credentials.json"), &login(live));
    private(
        &home.join(".claude.json"),
        &serde_json::json!({"oauthAccount": account(live)}),
    );
    let expected = root.join(".auto-switch.d/expected");
    fs::create_dir_all(&expected).unwrap();
    for dir in [root.join(".auto-switch.d"), expected.clone()] {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for name in names {
        assert!(root.join(name).is_dir(), "profile {name} must exist");
        private(
            &root.join(name).join(".claude.json"),
            &serde_json::json!({"oauthAccount": account(name)}),
        );
        private(
            &expected.join(format!("{name}.json")),
            &serde_json::json!({
                "account": format!("uuid-{name}"), "org": "fake-default1753-organization"
            }),
        );
    }
    for name in stored {
        private(&root.join(name).join(".credentials.json"), &login(name));
    }
    identities
}

/// Owned by the fixture; dropping it stops and joins the local server.
#[cfg(unix)]
#[must_use = "keep the mock identity server alive for fixture commands"]
pub struct LoginIdentities {
    base: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(unix)]
impl LoginIdentities {
    pub fn configure(&self, command: &mut std::process::Command) {
        command
            .env("AMPLIHACK_PROFILE_USAGE_API_BASE", &self.base)
            .env(
                "AMPLIHACK_PROFILE_USAGE_TOKEN_URL",
                format!("{}/v1/oauth/token", self.base),
            );
    }

    fn new(names: &[&str]) -> Self {
        use std::io::{BufRead, Read, Write};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};
        let answers: std::collections::HashMap<_, _> = names
            .iter()
            .map(|name| {
                (
                    format!("Bearer fake-default1753-{name}"),
                    serde_json::json!({"account": {"uuid": format!("uuid-{name}"),
                "email": format!("{name}@example.com")},
                "organization": {"uuid": "fake-default1753-organization"}}),
                )
            })
            .collect();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(120);
            while !stopping.load(Ordering::Relaxed) && Instant::now() < until {
                let (mut stream, _) = match listener.accept() {
                    Ok(found) => found,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("owned fake identity listener: {e}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap()).take(65_536);
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut token = String::new();
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        _ if line == "\r\n" => break,
                        _ => {}
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("authorization")
                    {
                        token = value.trim().to_string();
                    }
                }
                let answer = answers
                    .get(&token)
                    .filter(|_| first.starts_with("GET /api/oauth/profile "));
                let (status, body) = answer.map_or_else(
                    || ("401 Unauthorized", serde_json::json!({"error":"fake-only"})),
                    |body| ("200 OK", body.clone()),
                );
                let body = body.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self {
            base,
            stop,
            worker: Some(worker),
        }
    }
}

#[cfg(unix)]
impl Drop for LoginIdentities {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap();
    }
}
