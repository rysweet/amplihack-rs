//! All subprocesses use the repository's isolated binary helper and temp HOME.
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

pub const PRESERVED: &str = "amplihack: preserved Codex recovery journal: ";
pub const NO_PENDING: &str = "amplihack: no pending Codex recovery journal";

pub struct LaunchFixture {
    pub home: PathBuf,
    pub codex: PathBuf,
    pub root: PathBuf,
    pub tools: PathBuf,
    started_at: i64,
    _dir: tempfile::TempDir,
}
impl LaunchFixture {
    pub fn new() -> Self {
        Self::new_named("home")
    }
    pub fn new_named(name: &str) -> Self {
        let started_at = chrono::Utc::now().timestamp();
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(name);
        let codex = dir.path().join("custom-codex");
        let root = home.join(".amplihack/codex");
        let tools = dir.path().join("tools");
        for path in [&root, &codex, &tools] {
            fs::create_dir_all(path).unwrap();
        }
        for tool in [
            "sh", "bash", "cat", "mkdir", "cp", "rm", "mv", "chmod", "git", "uname", "grep", "awk",
            "sed", "touch", "dirname",
        ] {
            let found = Command::new("/bin/sh")
                .args(["-c", &format!("command -v {tool}")])
                .output()
                .unwrap();
            assert!(found.status.success(), "fixture requires {tool}");
            std::os::unix::fs::symlink(
                String::from_utf8(found.stdout).unwrap().trim(),
                tools.join(tool),
            )
            .unwrap();
        }
        let write = |name: &str, body: &str| {
            crate::support::exec_stub::write_executable(&tools.join(name), body, 0o700);
        };
        write("codex", FAKE_CODEX);
        write("node", "#!/bin/sh\necho v24.0.0\n");
        write(
            "amplihack-hooks",
            &format!("#!/bin/sh\necho '{{}}'\nexit 0\n# {}\n", "x".repeat(1100)),
        );
        write(
            "recipe-runner-rs",
            &format!(
                "#!/bin/sh\necho '{{\"schema_version\":1,\"version\":\"fixture\",\"capabilities\":[\"codex_exec\"]}}'\nexit 0\n# {}\n",
                "x".repeat(1100)
            ),
        );
        fs::write(
            codex.join("config.toml"),
            "# user configuration\napproval_policy = \"never\"\n",
        )
        .unwrap();
        fs::write(codex.join("hooks.json"), b"{\"hooks\":{}}\n").unwrap();
        Self {
            home,
            codex,
            root,
            tools,
            started_at,
            _dir: dir,
        }
    }
    pub fn command(&self) -> Command {
        let mut command =
            crate::isolated_amplihack::isolated(crate::support::amplihack_bin::amplihack_bin());
        command
            .env("HOME", &self.home)
            .env("CODEX_HOME", &self.codex)
            .env("AMPLIHACK_HOME", self.home.join(".amplihack"))
            .env("PATH", &self.tools)
            .env("TZ", "Pacific/Honolulu")
            .env("CODEX_BINARY_PATH", self.tools.join("codex"))
            .env("AMPLIHACK_CODEX_BINARY_PATH", self.tools.join("codex"))
            .env(
                "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH",
                self.tools.join("amplihack-hooks"),
            )
            .env("RECIPE_RUNNER_RS_PATH", self.tools.join("recipe-runner-rs"))
            .env("AMPLIHACK_AGENT_BINARY", "codex")
            .env("AMPLIHACK_NONINTERACTIVE", "1")
            .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
            .env("AMPLIHACK_NO_UPDATE_CHECK", "1")
            .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
            .current_dir(&self.home);
        command
    }
    pub fn launch(&self, before: &[&str], native: &[&str]) -> Output {
        self.command()
            .args(["codex", "--no-reflection", "--subprocess-safe"])
            .args(before)
            .arg("--")
            .args(native)
            .output()
            .unwrap()
    }
    pub fn pending(&self, bytes: &[u8]) {
        fs::write(self.root.join("pending.json"), bytes).unwrap();
    }
    pub fn archives(&self) -> Vec<PathBuf> {
        let mut paths = fs::read_dir(&self.root)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("pending.json.stale-")
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }
    pub fn assert_report(&self, out: &Output, bytes: &[u8]) -> PathBuf {
        let archives = self.archives();
        assert_eq!(archives.len(), 1, "{}", diagnostic(out));
        let archive = &archives[0];
        assert_eq!(fs::read(archive).unwrap(), bytes);
        let name = archive.file_name().unwrap().to_str().unwrap();
        let stamp = name
            .strip_prefix("pending.json.stale-")
            .unwrap()
            .split('.')
            .next()
            .unwrap();
        let utc = chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%dT%H%M%SZ")
            .expect("UTC archive name")
            .and_utc()
            .timestamp();
        assert!(
            (self.started_at..=chrono::Utc::now().timestamp()).contains(&utc),
            "archive timestamp must use UTC even when TZ selects Honolulu"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let lines = stderr
            .lines()
            .chain(stdout.lines())
            .filter(|l| l.starts_with(PRESERVED))
            .collect::<Vec<_>>();
        assert_eq!(
            lines,
            vec![format!("{PRESERVED}{}", archive.display())],
            "{}",
            diagnostic(out)
        );
        assert!(!self.root.join("pending.json").exists());
        archive.clone()
    }
    pub fn assert_launched(&self, out: &Output) -> Vec<String> {
        assert!(out.status.success(), "{}", diagnostic(out));
        let argv = fs::read_to_string(self.home.join("launch-argv"))
            .unwrap_or_else(|e| panic!("fake client not reached: {e}: {}", diagnostic(out)));
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("RECOVERY_LAUNCH_OK"),
            "{}",
            diagnostic(out)
        );
        argv.lines().map(str::to_owned).collect()
    }
    pub fn orphan(&self) -> Vec<u8> {
        // Valid schema-2 legacy journal, with no owned package to adopt.
        // Both recorded Codex targets must exist in every permitted state.
        let config = fs::read(self.codex.join("config.toml")).unwrap();
        let hooks = fs::read(self.codex.join("hooks.json")).unwrap();
        let pending: Value = json!({
            "schema_version":2,"transaction":"abandoned","codex_home":self.codex,
            "installed":false,"had_package":false,"ledger":null,"config":config,
            "config_states":[config],"target_digest":format!("v2:{}", "0".repeat(64)),
            "snapshots":{"hooks":hooks,"marketplace":null,"ledger":null},
            "expected":{"config":config,"hooks":hooks,"marketplace":null,"ledger":null}
        });
        let bytes = format!("\n{}\n", serde_json::to_string(&pending).unwrap()).into_bytes();
        self.pending(&bytes);
        fs::remove_file(self.codex.join("config.toml")).unwrap();
        fs::remove_file(self.codex.join("hooks.json")).unwrap();
        bytes
    }
}
pub fn diagnostic(out: &Output) -> String {
    format!(
        "status={}\nstdout={}\nstderr={}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

const FAKE_CODEX: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$HOME/client-calls"
if [ -f "$HOME/.amplihack/codex/pending.json" ]; then echo pending >> "$HOME/probe-order"; else echo clear >> "$HOME/probe-order"; fi
case "$1 $2 $3" in
 '--version  ') echo 'codex-cli 0.166.0';;
 'plugin list --json')
  if [ -f "$HOME/registered" ]; then
   printf '{"installed":[{"pluginId":"amplihack@amplihack-local","installed":true,"enabled":true,"source":{"path":"%s/.amplihack/codex/market/plugin"}}]}\n' "$HOME"
  else echo '{"installed":[]}'; fi;;
 'plugin marketplace add')
  if ! grep -Fq '[marketplaces.amplihack-local]' "$CODEX_HOME/config.toml" 2>/dev/null; then
   [ ! -s "$CODEX_HOME/config.toml" ] || printf '\n' >> "$CODEX_HOME/config.toml"
   printf '[marketplaces.amplihack-local]\nsource_type = "local"\nsource = "%s"\n' "$4" >> "$CODEX_HOME/config.toml"
  fi
  echo '{}';;
 'plugin add amplihack@amplihack-local')
  if ! grep -Fq '[plugins."amplihack@amplihack-local"]' "$CODEX_HOME/config.toml" 2>/dev/null; then
   printf '\n[plugins."amplihack@amplihack-local"]\nenabled = true\n' >> "$CODEX_HOME/config.toml"
  fi
  : > "$HOME/registered"; echo '{}';;
 'plugin remove amplihack@amplihack-local') rm -f "$HOME/registered"; echo '{}';;
 *) printf '%s\n' "$@" > "$HOME/launch-argv"; echo RECOVERY_LAUNCH_OK;;
esac
"#;
