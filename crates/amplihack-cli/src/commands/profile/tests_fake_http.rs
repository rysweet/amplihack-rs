//! A loopback HTTP/1.1 fake for the issue #1844 direct-read tests. It binds
//! `127.0.0.1:0`, records every request (method, path, headers, body) and
//! answers with whatever the route closure returns. It never reaches the
//! network beyond loopback and never sees a real credential: the tests give
//! it fake tokens only.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One request as the fake saw it. Header names are lower case.
#[derive(Debug, Clone)]
pub(super) struct Seen {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: String,
}

impl Seen {
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// What the fake sends back.
#[derive(Debug, Clone)]
pub(super) struct Reply {
    pub(super) status: u16,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
    /// Waited before the status line is written.
    pub(super) delay: Duration,
}

impl Reply {
    pub(super) fn json(status: u16, body: &serde_json::Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
            delay: Duration::ZERO,
        }
    }

    pub(super) fn raw(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
            delay: Duration::ZERO,
        }
    }

    pub(super) fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    pub(super) fn header(mut self, k: &str, v: &str) -> Self {
        self.headers.push((k.into(), v.into()));
        self
    }
}

type Route = dyn Fn(&Seen, usize) -> Reply + Send + Sync + 'static;

/// A running fake. The accept thread lives for the rest of the test
/// process; each test starts its own fake on its own port.
pub(super) struct FakeHttp {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeHttp {
    /// `route(request, n)` gives the reply; `n` counts earlier requests to
    /// the same path, from 0.
    pub(super) fn start(route: impl Fn(&Seen, usize) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let route: Arc<Route> = Arc::new(route);
        let log = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                let route = Arc::clone(&route);
                std::thread::spawn(move || serve(stream, &log, &*route));
            }
        });
        Self { port, seen }
    }

    pub(super) fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub(super) fn token_url(&self) -> String {
        format!("{}/v1/oauth/token", self.base())
    }

    pub(super) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub(super) fn count(&self, method: &str, path: &str) -> usize {
        self.seen()
            .iter()
            .filter(|s| s.method == method && s.path == path)
            .count()
    }
}

fn serve(stream: TcpStream, log: &Mutex<Vec<Seen>>, route: &Route) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut first = String::new();
    if reader.read_line(&mut first).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0; len];
    let _ = reader.read_exact(&mut body);
    let seen = Seen {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    let n = {
        let mut log = log.lock().unwrap();
        let n = log.iter().filter(|s| s.path == seen.path).count();
        log.push(seen.clone());
        n
    };
    let reply = route(&seen, n);
    std::thread::sleep(reply.delay);
    let mut out = format!(
        "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (k, v) in &reply.headers {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("\r\n");
    let mut stream = stream;
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.write_all(&reply.body);
    let _ = stream.flush();
}

/// A loopback address nothing listens on: binding and dropping a listener
/// frees its port, so a connection to it is refused at once.
pub(super) fn closed_base() -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    format!("http://127.0.0.1:{port}")
}
