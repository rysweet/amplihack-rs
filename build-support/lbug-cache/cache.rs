//! Local v1 cache approved by the owning deputy in PM328 comment6616249815.
//! No network, registry dependencies, application code, or archive extraction.
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};

pub const ARCHIVES: [&str; 18] = [
    "lbug",
    "utf8proc",
    "antlr4_cypher",
    "antlr4_runtime",
    "re2",
    "fastpfor",
    "parquet",
    "thrift",
    "snappy",
    "zstd",
    "miniz",
    "mbedtls",
    "brotlidec",
    "brotlicommon",
    "lz4",
    "roaring_bitmap",
    "simsimd",
    "yyjson",
];
const REGISTRY: &str = "7ff191ec195fb257dd7ca2adb76afd9a4b9a9f60fa27f1d87e5a958849fe9e6f";
const HEADERS: [&str; 10] = [
    "common/type_utils.h",
    "common/types/int128_t.h",
    "common/types/types.h",
    "common/types/value/nested.h",
    "common/types/value/node.h",
    "common/types/value/recursive_rel.h",
    "common/types/value/rel.h",
    "common/types/value/value.h",
    "main/lbug.h",
    "storage/storage_version_info.h",
];
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Environment = BTreeMap<String, String>;

#[cfg(target_os = "macos")]
const NOFOLLOW: i32 = 0x100;
#[cfg(not(target_os = "macos"))]
const NOFOLLOW: i32 = 0x20000;
#[cfg(target_os = "macos")]
const NONBLOCK: i32 = 0x4;
#[cfg(not(target_os = "macos"))]
const NONBLOCK: i32 = 0x800;
unsafe extern "C" {
    fn flock(fd: i32, operation: i32) -> i32;
    fn getuid() -> u32;
    fn kill(pid: i32, signal: i32) -> i32;
    fn signal(signal: i32, handler: usize) -> usize;
}
#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn renamex_np(from: *const std::ffi::c_char, to: *const std::ffi::c_char, flags: u32) -> i32;
}
#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn renameat2(
        from_fd: i32,
        from: *const std::ffi::c_char,
        to_fd: i32,
        to: *const std::ffi::c_char,
        flags: u32,
    ) -> i32;
}

/// Install without replacing even an empty competing directory.
fn install(from: &Path, to: &Path) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    // SAFETY: both paths are live NUL-terminated strings; the flags request an
    // atomic no-replacement rename on the two supported OS families.
    #[cfg(target_os = "macos")]
    let result = unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 4) };
    #[cfg(target_os = "linux")]
    let result = unsafe { renameat2(-100, from.as_ptr(), -100, to.as_ptr(), 1) };
    if result != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}
static CHILD: AtomicI32 = AtomicI32::new(0);
extern "C" fn forward(signum: i32) {
    let pid = CHILD.load(Ordering::Relaxed);
    if pid > 0 {
        // SAFETY: kill is async-signal-safe; a negative child pid selects its
        // separate process group, including CMake and compiler descendants.
        unsafe {
            kill(-pid, signum);
        }
    }
}

fn note(message: &str) {
    eprintln!("amplihack lbug: {message}");
}

/// SHA-256, FIPS 180-4. Used for local corruption detection, not authentication.
pub fn digest(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut state = [
        0x6a09e667_u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut bytes = data.to_vec();
    bytes.push(0x80);
    while bytes.len() % 64 != 56 {
        bytes.push(0);
    }
    bytes.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for block in bytes.chunks_exact(64) {
        let mut w = [0_u32; 64];
        for (i, b) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes(b.try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for i in 0..64 {
            let t1 = h
                .wrapping_add(e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25))
                .wrapping_add((e & f) ^ (!e & g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = (a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22))
                .wrapping_add((a & b) ^ (a & c) ^ (b & c));
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, v) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(v);
        }
    }
    state.iter().map(|n| format!("{n:08x}")).collect()
}

fn option(args: &[String], flag: &str) -> Result<Option<String>> {
    let mut values = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if arg == flag {
            values.push(args.get(i + 1).ok_or("incomplete Cargo option")?.clone());
        } else if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
            values.push(value.into());
        }
    }
    if values.len() > 1 {
        return Err("ambiguous Cargo options".into());
    }
    Ok(values.pop())
}

fn probe(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .output()?;
    if !output.status.success() {
        return Err("identification unavailable".into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

struct Request {
    key: String,
    context: String,
    compiler: String,
    profile: PathBuf,
}
fn resolve(repo: &Path, args: &[String], environment: &Environment) -> Result<Request> {
    if environment.iter().any(|(name, value)| {
        (name.contains("FLAGS") || name.starts_with("CMAKE"))
            && ["-march=native", "-mtune=native", "-mcpu=native"]
                .iter()
                .any(|flag| value.contains(flag))
    }) {
        return Err("machine-specific native configuration".into());
    }
    let lock = fs::read_to_string(repo.join("Cargo.lock"))?;
    let packages: Vec<_> = lock
        .split("[[package]]")
        .filter(|p| p.lines().any(|l| l == "name = \"lbug\""))
        .collect();
    if packages.len() != 1
        || !packages[0].contains("version = \"0.15.4\"")
        || !packages[0].contains(&format!("checksum = \"{REGISTRY}\""))
    {
        return Err("unsupported locked source".into());
    }
    // Support Cargo's common build target/target-dir settings; unsupported TOML
    // forms select source. Never guess at target tables or native env overrides.
    let cwd = env::current_dir()?;
    if !fs::canonicalize(&cwd)?.starts_with(repo) {
        return Err("external Cargo workspace".into());
    }
    let mut paths = vec![
        environment
            .get("CARGO_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(environment.get("HOME").map(String::as_str).unwrap_or(""))
                    .join(".cargo")
            }),
    ];
    let mut ancestors: Vec<_> = cwd.ancestors().map(|p| p.join(".cargo")).collect();
    ancestors.reverse();
    paths.extend(ancestors);
    let mut config = BTreeMap::new();
    for dir in paths {
        for name in ["config", "config.toml"] {
            let path = dir.join(name);
            if !path.exists() {
                continue;
            }
            let text = fs::read_to_string(path)?;
            let mut section = "";
            for line in text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
            {
                if line.starts_with('[') {
                    section = line;
                }
                if ![
                    "[build]",
                    "[env]",
                    "[net]",
                    "[term]",
                    "[alias]",
                    "[http]",
                    "[registry]",
                ]
                .contains(&section)
                {
                    return Err("unsupported Cargo configuration".into());
                }
                if section == "[env]" && !line.starts_with('[') {
                    let key = line
                        .split_once('=')
                        .ok_or("unsupported Cargo env")?
                        .0
                        .trim()
                        .trim_matches(['\'', '"']);
                    if !["GIT_TERMINAL_PROMPT", "GIT_ASKPASS", "SSH_ASKPASS"].contains(&key) {
                        return Err("native or unsupported Cargo env".into());
                    }
                }
                if section == "[build]"
                    && let Some((k, v)) = line.split_once('=')
                {
                    let k = k.trim();
                    if ["target", "target-dir"].contains(&k) {
                        let v = v.trim();
                        if !v.starts_with('"')
                            || !v.ends_with('"')
                            || v[1..v.len() - 1].contains(['"', '\\'])
                        {
                            return Err("unsupported Cargo target".into());
                        }
                        let value = &v[1..v.len() - 1];
                        let value = if k == "target-dir" {
                            dir.parent()
                                .ok_or("Cargo config location")?
                                .join(value)
                                .to_string_lossy()
                                .into_owned()
                        } else {
                            value.to_string()
                        };
                        config.insert(k.to_string(), value);
                    }
                }
            }
        }
    }
    if args
        .iter()
        .any(|a| a == "--config" || a.starts_with("--config="))
    {
        return Err("command-line Cargo configuration".into());
    }
    if let Some(manifest) = option(args, "--manifest-path")?
        && fs::canonicalize(manifest)? != repo.join("Cargo.toml")
    {
        return Err("external manifest".into());
    }
    let rustc = probe("rustc", &["-vV"])?;
    let host = rustc
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .ok_or("unknown Rust host")?;
    let explicit = option(args, "--target")?
        .or_else(|| environment.get("CARGO_BUILD_TARGET").cloned())
        .or_else(|| config.get("target").cloned());
    let target = explicit.as_deref().unwrap_or(host);
    if target != host
        || ![
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "aarch64-unknown-linux-gnu",
            "x86_64-unknown-linux-gnu",
        ]
        .contains(&target)
    {
        return Err("unsupported or cross target".into());
    }
    let names = [
        format!("CXX_{target}"),
        format!("CXX_{}", target.replace('-', "_")),
        "HOST_CXX".into(),
        "CXX".into(),
    ];
    let compiler = names
        .iter()
        .find_map(|n| environment.get(n))
        .map(String::as_str)
        .unwrap_or("c++");
    let compiler_path = env::split_paths(environment.get("PATH").ok_or("no PATH")?)
        .map(|p| p.join(compiler))
        .find(|p| p.is_file())
        .ok_or("unknown compiler")?;
    let compiler = fs::canonicalize(compiler_path)?
        .to_string_lossy()
        .into_owned();
    let version = probe(&compiler, &["--version"])?;
    let (family, suffix) = if let Some((_, s)) = version.split_once("Apple clang version ") {
        ("appleclang", s)
    } else if let Some((_, s)) = version.split_once("clang version ") {
        ("clang", s)
    } else {
        (
            "gcc",
            version
                .lines()
                .next()
                .ok_or("compiler version")?
                .split_whitespace()
                .find(|s| s.as_bytes().first().is_some_and(u8::is_ascii_digit) && s.contains('.'))
                .ok_or("compiler major")?,
        )
    };
    let major: String = suffix.chars().take_while(char::is_ascii_digit).collect();
    if major.is_empty() {
        return Err("compiler major".into());
    }
    let key = format!("lbug-0.15.4-{target}-{family}{major}");
    let manifest = fs::read_to_string(repo.join("Cargo.toml"))?;
    let mut native_manifest = String::new();
    let mut native_profile = false;
    for line in manifest
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        if line.starts_with('[') {
            if line.starts_with("[patch") || line.starts_with("[replace") {
                return Err("patched Cargo source".into());
            }
            native_profile = line.starts_with("[profile.dev") || line.starts_with("[profile.test");
        }
        if native_profile {
            native_manifest.push_str(line);
            native_manifest.push('\n');
        }
    }
    // Application version/dependency changes must not invalidate unchanged C++.
    // Bind only native profiles and the locked packages that drive this build.
    for package in lock.split("[[package]]") {
        if package.lines().any(|line| {
            ["lbug", "cc", "cmake", "cxx", "cxx-build"]
                .iter()
                .any(|name| line == format!("name = \"{name}\""))
        }) {
            native_manifest.push_str(package);
        }
    }
    let mut native = format!(
        "v1\n{key}\n{REGISTRY}\n{compiler}\n{version}\n{}\n",
        digest(native_manifest.as_bytes())
    );
    native.push_str(&probe("uname", &["-sr"])?);
    #[cfg(target_os = "macos")]
    native.push_str(&probe("xcrun", &["--show-sdk-path"])?);
    for (k, v) in environment {
        if !["CMAKE", "CMAKE_BUILD_PARALLEL_LEVEL"].contains(&k.as_str())
            && ([
                "CXX",
                "CC",
                "CMAKE",
                "CARGO_PROFILE_",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
            ]
            .iter()
            .any(|prefix| k.starts_with(prefix))
                || ["SDKROOT", "MACOSX_DEPLOYMENT_TARGET", "LDFLAGS"].contains(&k.as_str()))
        {
            native.push_str(&format!("{}:{k}{}:{v}\n", k.len(), v.len()));
        }
    }
    let target_dir = option(args, "--target-dir")?
        .or_else(|| environment.get("CARGO_TARGET_DIR").cloned())
        .or_else(|| environment.get("CARGO_BUILD_TARGET_DIR").cloned())
        .or_else(|| config.get("target-dir").cloned())
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("target"));
    let profile = if explicit.is_some() {
        target_dir.join(target)
    } else {
        target_dir
    }
    .join("debug");
    Ok(Request {
        key,
        context: digest(native.as_bytes()),
        compiler,
        profile,
    })
}

/// Upstream tracks compiler flags but omits LBUG_LIBRARY_DIR/INCLUDE_DIR. A
/// stable semantic marker invalidates managed output on rejection, removal, or
/// mode changes, while valid warm Cargo fingerprints stay reusable.
pub fn selection_env(environment: &Environment, selection: &str) -> Environment {
    let mut result = environment.clone();
    let marker = format!(
        " -DAMPLIHACK_LBUG_SELECTION_{}=1",
        digest(selection.as_bytes())
    );
    let mut names = BTreeSet::from(["CXXFLAGS".to_owned()]);
    names.extend(
        environment
            .keys()
            .filter(|k| {
                k.starts_with("CXXFLAGS_")
                    || ["HOST_CXXFLAGS", "TARGET_CXXFLAGS"].contains(&k.as_str())
            })
            .cloned(),
    );
    for name in names {
        result.insert(
            name.clone(),
            format!(
                "{}{marker}",
                environment.get(&name).map(String::as_str).unwrap_or("")
            ),
        );
    }
    result
}

fn cargo(args: &[String], environment: &Environment) -> Result<i32> {
    use std::os::unix::process::ExitStatusExt;
    // SAFETY: our handler uses only atomic access and async-signal-safe kill.
    let previous = unsafe {
        [
            signal(2, forward as *const () as usize),
            signal(15, forward as *const () as usize),
        ]
    };
    let mut child = Command::new(
        environment
            .get("AMPLIHACK_LBUG_CARGO")
            .map(String::as_str)
            .unwrap_or("cargo"),
    )
    .args(args)
    .env_clear()
    .envs(environment)
    .process_group(0)
    .spawn()?;
    CHILD.store(child.id() as i32, Ordering::SeqCst);
    let status = child.wait();
    CHILD.store(0, Ordering::SeqCst);
    // SAFETY: restore the exact handler values returned by signal.
    unsafe {
        signal(2, previous[0]);
        signal(15, previous[1]);
    }
    let status = status?;
    Ok(status.code().unwrap_or(128 + status.signal().unwrap_or(1)))
}

fn private_dir(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
    {
        return Err("unsafe cache location".into());
    }
    let mut current = PathBuf::from("/");
    for component in path.components().skip(1) {
        current.push(component);
        match fs::DirBuilder::new().mode(0o700).create(&current) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.into()),
        }
        let m = fs::symlink_metadata(&current)?;
        // SAFETY: getuid has no parameters or memory effects.
        if !m.is_dir()
            || ![0, unsafe { getuid() }].contains(&m.uid())
            || (m.mode() & 0o022 != 0 && m.mode() & 0o1000 == 0)
        {
            return Err("unsafe cache ancestor".into());
        }
    }
    let m = fs::symlink_metadata(path)?;
    if m.uid() != unsafe { getuid() } || m.mode() & 0o077 != 0 {
        return Err("cache is not private".into());
    }
    Ok(())
}
use std::os::unix::fs::DirBuilderExt;

fn read_regular(path: &Path) -> Result<Vec<u8>> {
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(NOFOLLOW | NONBLOCK)
        .open(path)?;
    let m = f.metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { getuid() }
        || m.mode() & 0o022 != 0
        || m.mode() & 0o400 == 0
        || m.len() > 512 * 1024 * 1024
    {
        return Err("unsafe payload file".into());
    }
    let mut bytes = Vec::new();
    f.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn files(root: &Path) -> Result<Vec<PathBuf>> {
    fn visit(root: &Path, current: &Path, output: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(current)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            let rel = path
                .strip_prefix(root)?
                .to_str()
                .ok_or("non-UTF8 payload")?;
            if !rel.is_ascii()
                || rel.contains(['\n', '\r', '\t', '\\'])
                || metadata.file_type().is_symlink()
            {
                return Err("unsafe payload path".into());
            }
            if metadata.is_dir() {
                visit(root, &path, output)?;
            } else if metadata.is_file() {
                output.push(path);
            } else {
                return Err("special payload file".into());
            }
            if output.len() > 10000 {
                return Err("payload inventory limit".into());
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    visit(root, root, &mut output)?;
    output.sort();
    let mut seen = BTreeSet::new();
    for p in &output {
        if !seen.insert(
            p.strip_prefix(root)?
                .to_str()
                .ok_or("payload path")?
                .to_lowercase(),
        ) {
            return Err("case collision".into());
        }
    }
    Ok(output)
}

fn inventory(entry: &Path) -> Result<String> {
    let mut out = String::new();
    let mut libs = BTreeSet::new();
    let mut has_header = false;
    for path in files(entry)? {
        let rel = path.strip_prefix(entry)?.to_str().ok_or("payload path")?;
        if rel == "manifest.sha256" {
            continue;
        }
        if !rel.starts_with("lib/") && !rel.starts_with("include/") {
            return Err("unexpected payload".into());
        }
        let bytes = read_regular(&path)?;
        if let Some(name) = rel.strip_prefix("lib/") {
            if !bytes.starts_with(b"!<arch>\n") {
                return Err("thin or invalid archive".into());
            }
            libs.insert(name.to_string());
        }
        has_header |= rel == "include/lbug.hpp";
        out.push_str(&format!("{}  {}  {rel}\n", digest(&bytes), bytes.len()));
    }
    if libs != ARCHIVES.iter().map(|n| format!("lib{n}.a")).collect() || !has_header {
        return Err("incomplete static payload".into());
    }
    Ok(out)
}

// Native trees include upstream's shared-library symlinks and CMake metadata.
// Select only requested regular files, without following unrelated links.
fn inputs(root: &Path, suffixes: &[&str]) -> Result<Vec<PathBuf>> {
    let mut output = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            output.extend(inputs(&path, suffixes)?);
        } else if suffixes.contains(&path.extension().and_then(|s| s.to_str()).unwrap_or("")) {
            if !metadata.is_file() {
                return Err("linked or special native input".into());
            }
            output.push(path);
        }
    }
    output.sort();
    Ok(output)
}

pub fn verify(entry: &Path, context: &str) -> Result<String> {
    if !fs::symlink_metadata(entry)?.is_dir() {
        return Err("unsafe entry".into());
    }
    let bytes = read_regular(&entry.join("manifest.sha256"))?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("manifest limit".into());
    }
    let expected = format!("lbug-local-v1 {context}\n{}", inventory(entry)?);
    if bytes != expected.as_bytes() {
        return Err("checksum or compatibility mismatch".into());
    }
    Ok(digest(&bytes))
}

fn collect(request: &Request, staging: &Path) -> Result<()> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(request.profile.join("build"))? {
        let p = entry?.path();
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("lbug-"))
            && p.join("out/build/src/liblbug.a").is_file()
        {
            candidates.push(p.join("out"));
        }
    }
    if candidates.len() != 1 {
        return Err("native output absent or ambiguous".into());
    }
    let output = &candidates[0];
    let cache = fs::read_to_string(output.join("build/CMakeCache.txt"))?;
    let cmake_value = |name: &str| -> Result<PathBuf> {
        Ok(PathBuf::from(
            cache
                .lines()
                .find(|l| l.starts_with(&format!("{name}:")))
                .and_then(|l| l.split_once('='))
                .ok_or("missing CMake identity")?
                .1,
        ))
    };
    if fs::canonicalize(cmake_value("CMAKE_CXX_COMPILER")?)? != Path::new(&request.compiler) {
        return Err("CMake compiler disagrees".into());
    }
    let source = cmake_value("CMAKE_HOME_DIRECTORY")?;
    if source.file_name().and_then(|n| n.to_str()) != Some("lbug-src")
        || source
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
            != Some("lbug-0.15.4")
    {
        return Err("unrecognized bundled source".into());
    }
    fs::create_dir(staging.join("lib"))?;
    fs::create_dir(staging.join("include"))?;
    let built_files = inputs(&output.join("build"), &["a"])?;
    for name in ARCHIVES {
        let filename = format!("lib{name}.a");
        let found: Vec<_> = built_files
            .iter()
            .filter(|p| p.file_name().is_some_and(|n| n == filename.as_str()))
            .collect();
        if found.len() != 1 {
            return Err("native archive absent or ambiguous".into());
        }
        fs::write(staging.join("lib").join(filename), read_regular(found[0])?)?;
    }
    for (root, optional) in [
        (source.join("src/include"), false),
        (output.join("build/src"), false),
        (output.join("build/src/include"), false),
        (source.join("third_party/nlohmann_json"), true),
        (source.join("third_party/fastpfor"), true),
        (source.join("third_party/alp/include"), true),
    ] {
        // Upstream adds optional include paths even when the registry crate
        // omits those directories. Only absence is optional; preserve errors
        // for unreadable, linked or non-directory inputs.
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.is_dir() => {}
            Err(error) if optional && error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
            Ok(_) => return Err("unsafe header root".into()),
        }
        for path in inputs(&root, &["h", "hpp", "hh", "hxx", "inc", "inl"])? {
            let destination = staging.join("include").join(path.strip_prefix(&root)?);
            let bytes = read_regular(&path)?;
            if destination.exists() && read_regular(&destination)? != bytes {
                return Err("conflicting header roots".into());
            }
            fs::create_dir_all(destination.parent().ok_or("header parent")?)?;
            fs::write(destination, bytes)?;
        }
    }
    fs::write(
        staging.join("include/lbug.hpp"),
        format!(
            "#pragma once\n{}",
            HEADERS
                .iter()
                .map(|h| format!("#include \"{h}\"\n"))
                .collect::<String>()
        ),
    )?;
    let mut child = Command::new(&request.compiler)
        .args([
            "-std=c++20",
            "-DLBUG_STATIC_DEFINE",
            "-DENABLE_RUNTIME_CHECKS=1",
            "-fsyntax-only",
            "-x",
            "c++",
            "-",
        ])
        .arg("-I")
        .arg(staging.join("include"))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("compiler stdin")?
        .write_all(b"#include \"lbug.hpp\"\n")?;
    if !child.wait()?.success() {
        return Err("relocatable header compilation failed".into());
    }
    let manifest = format!("lbug-local-v1 {}\n{}", request.context, inventory(staging)?);
    fs::write(staging.join("manifest.sha256"), manifest)?;
    verify(staging, &request.context)?;
    Ok(())
}

fn managed(repo: &Path, args: &[String], environment: &Environment) -> Result<i32> {
    use std::os::fd::AsRawFd;
    let before = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let build_args = &args[..before];
    let request = resolve(repo, build_args, environment)?;
    let root = environment
        .get("AMPLIHACK_LBUG_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            environment
                .get("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(environment.get("HOME").map(String::as_str).unwrap_or(""))
                        .join(".cache")
                })
                .join("amplihack/lbug")
        });
    private_dir(&root)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(NOFOLLOW | NONBLOCK)
        .open(root.join(format!("{}.lock", request.key)))?;
    let metadata = lock.metadata()?;
    if !metadata.is_file() || metadata.uid() != unsafe { getuid() } || metadata.mode() & 0o077 != 0
    {
        return Err("unsafe key lock".into());
    }
    // Wrapper owns this stable lock through verification, source compilation,
    // installation, and linking. No child reacquires it; locks are never unlinked.
    note("waiting for local key lock");
    if unsafe { flock(lock.as_raw_fd(), 2) } != 0 {
        return Err("key lock unavailable".into());
    }
    let entry = root.join(&request.key);
    if fs::symlink_metadata(&entry).is_ok() {
        match verify(&entry, &request.context) {
            Ok(fingerprint) => {
                let mut selected =
                    selection_env(environment, &format!("{}:{fingerprint}", request.key));
                selected.insert(
                    "LBUG_LIBRARY_DIR".into(),
                    entry.join("lib").to_string_lossy().into(),
                );
                selected.insert(
                    "LBUG_INCLUDE_DIR".into(),
                    entry.join("include").to_string_lossy().into(),
                );
                note(&format!(
                    "verified local hit {}; FFI shim remains per target",
                    request.key
                ));
                return cargo(args, &selected);
            }
            Err(_) => {
                note("local checksum or compatibility rejected; using bundled source");
                return cargo(args, &selection_env(environment, "source"));
            }
        }
    }
    note(&format!("local miss {}; using bundled source", request.key));
    let result = cargo(args, &selection_env(environment, "source"))?;
    if result != 0 {
        return Ok(result);
    }
    let staging = root.join(format!(".staging-{}-{}", std::process::id(), request.key));
    let created = fs::DirBuilder::new().mode(0o700).create(&staging).is_ok();
    let prepared = (|| -> Result<()> {
        if !created {
            return Err("staging already exists or is unavailable".into());
        }
        collect(&request, &staging)?;
        if fs::symlink_metadata(&entry).is_ok() {
            return Err("existing winner".into());
        }
        install(&staging, &entry)?;
        note(&format!("stored local entry {}", request.key));
        Ok(())
    })();
    if prepared.is_err() {
        note("cache preparation failed; successful source output retained");
    }
    // Only this invocation's exclusively-created staging can be removed.
    if created && prepared.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    Ok(result)
}

pub fn run(args: Vec<String>) -> i32 {
    let environment: Environment = env::vars().collect();
    let repo = fs::canonicalize(
        PathBuf::from(env!(
            "CARGO_MANIFEST_DIR",
            "bootstrap must supply CARGO_MANIFEST_DIR"
        ))
        .parent()
        .and_then(Path::parent)
        .unwrap(),
    )
    .unwrap();
    match managed(&repo, &args, &environment) {
        Ok(code) => code,
        Err(_) => {
            note("cache identification or storage unavailable; using bundled source");
            cargo(&args, &selection_env(&environment, "source")).unwrap_or(127)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            digest(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            digest(&vec![b'a'; 1_000_000]),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }
    #[test]
    fn semantic_selection_is_stable_and_preserves_target_flags() {
        let env = BTreeMap::from([("CXXFLAGS_x86_64_unknown_linux_gnu".into(), "-O2".into())]);
        let selected = selection_env(&env, "verified:payload");
        assert_eq!(selected, selection_env(&env, "verified:payload"));
        assert_ne!(selected, selection_env(&env, "source"));
        assert!(selected["CXXFLAGS_x86_64_unknown_linux_gnu"].starts_with("-O2 "));
    }
}
