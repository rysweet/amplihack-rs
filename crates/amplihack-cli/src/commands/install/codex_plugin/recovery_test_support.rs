//! Issue #2284 fixtures: real package planning/commit, isolated native client.
use super::*;
use crate::test_support::{EnvGuard, env_lock, write_executable};
use std::{cell::RefCell, sync::MutexGuard, time::SystemTime};

pub(super) const STAMP: &str = "20261110T180000Z";
thread_local! { static ARCHIVE_STAMP: RefCell<Option<String>> = const { RefCell::new(None) }; }

// Implementation seam: the archival clock reads this cfg(test)-only override
// once per operation. Production uses UTC; tests never wait for clock ticks.
pub(super) fn archive_stamp() -> Option<String> {
    ARCHIVE_STAMP.with(|v| v.borrow().clone())
}
pub(super) fn with_archive_stamp<T>(run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ARCHIVE_STAMP.with(|v| *v.borrow_mut() = None);
        }
    }
    ARCHIVE_STAMP.with(|v| {
        assert!(v.borrow().is_none());
        *v.borrow_mut() = Some(STAMP.into());
    });
    let _reset = Reset;
    run()
}

pub(super) struct Fixture {
    pub root: PathBuf,
    pub home: PathBuf,
    pub codex: PathBuf,
    pub source: PathBuf,
    pub binary: PathBuf,
    hooks: PathBuf,
    _env: EnvGuard,
    _dir: tempfile::TempDir,
    _lock: MutexGuard<'static, ()>,
}
impl Fixture {
    pub fn new() -> Self {
        let lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        // Explicit custom CODEX_HOME catches accidental use of ~/.codex.
        let codex = dir.path().join("selected-codex");
        let root = home.join(".amplihack/codex");
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("amplifier-bundle/skills")).unwrap();
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&codex).unwrap();
        fs::write(source.join("amplifier-bundle/skills/marker"), b"base").unwrap();
        let hooks = dir.path().join("amplihack-hooks");
        write_executable(&hooks, "#!/bin/sh\nprintf '{}'\n");
        let binary = dir.path().join("codex");
        write_executable(&binary, FAKE_CODEX);
        let env = EnvGuard::set([
            ("HOME", home.to_str().unwrap()),
            ("CODEX_HOME", codex.to_str().unwrap()),
            ("PATH", "/usr/bin:/bin"),
        ]);
        Self {
            root,
            home,
            codex,
            source,
            binary,
            hooks,
            _env: env,
            _dir: dir,
            _lock: lock,
        }
    }
    pub fn install(&self) -> Result<bool> {
        crate::freshness::with_provider("codex", || {
            with_binary(&self.binary, || super::install(&self.source, &self.hooks))
        })
    }
    pub fn ledger(&self) -> Value {
        regular_json(&self.root.join("ownership.json"))
            .unwrap()
            .unwrap()
    }
    pub fn upgrade(&self, marker: &[u8]) {
        fs::write(self.source.join("amplifier-bundle/skills/marker"), marker).unwrap();
    }
    pub fn journal(&self) -> Value {
        regular_json(&self.root.join("pending.json"))
            .unwrap()
            .unwrap()
    }
    pub fn write_journal(&self, value: &Value) -> Vec<u8> {
        // Deliberately noncanonical JSON formatting; archival must preserve bytes.
        let bytes = format!(" \n{}\n\n", serde_json::to_string(value).unwrap()).into_bytes();
        fs::write(self.root.join("pending.json"), &bytes).unwrap();
        bytes
    }
    pub fn plan(&self) -> Vec<u8> {
        path_scope::with_scope(&self.root, &self.codex, || {
            let _prepared = prepare(
                &self.root,
                &self.codex,
                &self.binary,
                &self.source,
                &self.hooks,
            )?;
            Ok(())
        })
        .unwrap();
        self.write_journal(&self.journal())
    }
    pub fn orphan(&self) -> Vec<u8> {
        assert!(self.install().unwrap());
        self.upgrade(b"abandoned");
        let bytes = self.plan();
        fs::remove_file(self.codex.join("config.toml")).unwrap();
        fs::remove_file(self.codex.join("hooks.json")).unwrap();
        bytes
    }
    pub fn superseded(&self) -> Vec<u8> {
        assert!(self.install().unwrap());
        self.upgrade(b"abandoned");
        let bytes = self.plan();
        fs::remove_file(self.root.join("pending.json")).unwrap();
        self.upgrade(b"replacement");
        let config = self.codex.join("config.toml");
        let mut text = fs::read_to_string(&config).unwrap();
        text.push_str("\n# preferences from the later completed reconciliation\n");
        fs::write(config, text).unwrap();
        let hooks = self.codex.join("hooks.json");
        let mut value = regular_json(&hooks).unwrap().unwrap();
        value["later_reconciliation"] = json!(true);
        fs::write(hooks, json_bytes(&value).unwrap()).unwrap();
        assert!(self.install().unwrap());
        fs::write(self.root.join("pending.json"), &bytes).unwrap();
        bytes
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
    pub fn assert_progress(&self) {
        assert!(!self.root.join("pending.json").exists());
        assert_eq!(
            fs::read(self.root.join("market/plugin/skills/marker")).unwrap(),
            fs::read(self.source.join("amplifier-bundle/skills/marker")).unwrap()
        );
        let ledger = self.ledger();
        assert_eq!(ledger["codex_home"], json!(self.codex));
        assert_eq!(
            ledger["package_digest"],
            digest(&self.root.join("market/plugin")).unwrap()
        );
        let hooks = regular_json(&self.codex.join("hooks.json"))
            .unwrap()
            .unwrap();
        assert!(hooks["hooks"]["SessionStart"].is_array());
        assert!(!self.home.join(".codex").exists());
    }
    pub fn assert_retained(&self, before: &[u8]) -> String {
        let config = snapshot(&self.codex.join("config.toml")).unwrap();
        let hooks = snapshot(&self.codex.join("hooks.json")).unwrap();
        let ledger = snapshot(&self.root.join("ownership.json")).unwrap();
        let package = digest(&self.root.join("market/plugin")).unwrap();
        let error = format!(
            "{:#}",
            self.install().expect_err("ambiguous recovery must refuse")
        );
        assert!(
            error.contains("retained") || error.contains("reconcile manually"),
            "{error}"
        );
        assert_eq!(fs::read(self.root.join("pending.json")).unwrap(), before);
        assert!(self.archives().is_empty());
        assert_eq!(snapshot(&self.codex.join("config.toml")).unwrap(), config);
        assert_eq!(snapshot(&self.codex.join("hooks.json")).unwrap(), hooks);
        assert_eq!(snapshot(&self.root.join("ownership.json")).unwrap(), ledger);
        assert_eq!(digest(&self.root.join("market/plugin")).unwrap(), package);
        error
    }
}

pub(super) fn modified(path: &Path, seconds: u64) {
    let file = fs::OpenOptions::new().write(true).open(path).unwrap();
    file.set_times(
        fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
    )
    .unwrap();
}

const FAKE_CODEX: &str = r#"#!/bin/sh
set -e
config_has() {
 if [ ! -e "$CODEX_HOME/config.toml" ] && [ ! -L "$CODEX_HOME/config.toml" ]; then return 1; fi
 if grep -Fq "$1" "$CODEX_HOME/config.toml"; then return 0; else
  status=$?
  if [ "$status" -eq 1 ]; then return 1; fi
  exit "$status"
 fi
}
case "$1 $2 $3" in
 '--version  ') echo 'codex-cli 0.166.0';;
 'plugin list --json')
  if [ -f "$HOME/registered" ]; then
   printf '{"installed":[{"pluginId":"amplihack@amplihack-local","installed":true,"enabled":true,"source":{"path":"%s/.amplihack/codex/market/plugin"}}]}\n' "$HOME"
  else echo '{"installed":[]}'; fi;;
 'plugin marketplace add')
  if ! config_has '[marketplaces.amplihack-local]'; then
   if [ -s "$CODEX_HOME/config.toml" ]; then printf '\n' >> "$CODEX_HOME/config.toml"; fi
   printf '[marketplaces.amplihack-local]\nsource_type = "local"\nsource = "%s"\n' "$4" >> "$CODEX_HOME/config.toml"
  fi
  echo '{}';;
 'plugin add amplihack@amplihack-local')
  if ! config_has '[plugins."amplihack@amplihack-local"]'; then
   printf '\n[plugins."amplihack@amplihack-local"]\nenabled = true\n' >> "$CODEX_HOME/config.toml"
  fi
  if [ -f "$HOME/edit-during-add" ]; then printf '\n# concurrent foreign edit\n' >> "$CODEX_HOME/config.toml"; fi
  : > "$HOME/registered"; echo '{}';;
 'plugin remove amplihack@amplihack-local')
  sed '/^\[plugins\."amplihack@amplihack-local"\]/,$d' "$CODEX_HOME/config.toml" > "$CODEX_HOME/config.new"
  sed '${/^$/d;}' "$CODEX_HOME/config.new" > "$CODEX_HOME/config.toml"
  rm -f "$CODEX_HOME/config.new" "$HOME/registered"; echo '{}';;
 *) echo '{}';;
esac
"#;
