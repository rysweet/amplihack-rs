//! The two keys that switch off text amplihack inserts into a session
//! (issue #3247): `text.routing` and `text.prompt_preferences`.
//!
//! Each takes `on` or `off` and defaults to `on`. The value comes from the
//! first layer that sets it: the key's environment variable, then the
//! project file, then the user file. An empty variable counts as unset, as it
//! does for every other key. A value that is neither `on` nor `off`, in any
//! layer, leaves the default in force, and the layers below it are not
//! consulted. A file that cannot be read or parsed is skipped, and the
//! problem is returned for the caller to report.

use std::path::{Path, PathBuf};

use super::files::{Project, project, same_file, user_file};
use super::read::{parse_document, read_bounded};

/// One of the two switches: its key and its environment variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextSwitch {
    /// `<table>.<key>`, as `amplihack config` writes it.
    pub key: &'static str,
    /// The variable that outranks every file, for one launch.
    pub env: &'static str,
}

/// `text.routing`: the routing reminder on prompt events, the session-start
/// text that sends work to the orchestrator, and the router lines of the
/// launch contract.
pub const ROUTING: TextSwitch = TextSwitch {
    key: "text.routing",
    env: "AMPLIHACK_ROUTING_TEXT",
};

/// `text.prompt_preferences`: the preferences table and the learned-patterns
/// line the prompt-event hook adds.
pub const PROMPT_PREFERENCES: TextSwitch = TextSwitch {
    key: "text.prompt_preferences",
    env: "AMPLIHACK_PROMPT_PREFERENCES",
};

/// The accepted values, in the order `amplihack config` lists them.
pub const VALUES: &[&str] = &["on", "off"];

/// The value when no layer sets one.
pub const DEFAULT: &str = "on";

/// Where a switch's value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Env(&'static str),
    Project(PathBuf),
    User(PathBuf),
    Default,
}

impl Source {
    /// The short label: `env`, `project`, `user` or `default`.
    pub fn label(&self) -> &'static str {
        match self {
            Source::Env(_) => "env",
            Source::Project(_) => "project",
            Source::User(_) => "user",
            Source::Default => "default",
        }
    }
}

/// A value that is neither `on` nor `off`, and the layer that gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unknown {
    /// The value as written: the variable's text, or the file's TOML.
    pub raw: String,
    pub source: Source,
}

/// A file that was skipped because it could not be read or parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileProblem {
    pub path: PathBuf,
    /// Why, without quoting the contents.
    pub reason: String,
}

/// A switch's effective value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setting {
    /// `true` for `on`.
    pub on: bool,
    /// The layer that decided. [`Source::Default`] when no layer set a value
    /// or when the deciding layer's value was unknown.
    pub source: Source,
    /// The unknown value that left the default in force, if any.
    pub unknown: Option<Unknown>,
    /// Files skipped on the way.
    pub problems: Vec<FileProblem>,
}

impl Setting {
    /// The value as `amplihack config get` prints it.
    pub fn word(&self) -> &'static str {
        if self.on { "on" } else { "off" }
    }
}

/// Everything a read depends on, so tests can supply it.
pub struct Layers<'a> {
    /// `$HOME`. `None`: no user file.
    pub home: Option<&'a Path>,
    /// Where the project-file search starts.
    pub cwd: &'a Path,
    /// Whether to read the project layer at all.
    pub read_project: bool,
    /// The environment.
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

/// The project file the layers read, if any: the one found from `cwd` up to
/// the git root, unless it is the user file itself.
pub fn project_file(home: Option<&Path>, cwd: &Path, read_project: bool) -> Option<PathBuf> {
    if !read_project {
        return None;
    }
    let user = home.map(user_file);
    match project(cwd) {
        Project::File { path, found: true }
            if !user.as_deref().is_some_and(|user| same_file(user, &path)) =>
        {
            Some(path)
        }
        _ => None,
    }
}

/// The effective value of `switch`.
pub fn read(switch: TextSwitch, layers: &Layers<'_>) -> Setting {
    let mut problems = Vec::new();
    let mut decided = (layers.env)(switch.env)
        .filter(|raw| !raw.is_empty())
        .map(|raw| (raw.trim().to_string(), Source::Env(switch.env)));
    if decided.is_none()
        && let Some(path) = project_file(layers.home, layers.cwd, layers.read_project)
    {
        decided = file_value(switch, &path, &mut problems).map(|raw| (raw, Source::Project(path)));
    }
    if decided.is_none()
        && let Some(path) = layers.home.map(user_file)
    {
        decided = file_value(switch, &path, &mut problems).map(|raw| (raw, Source::User(path)));
    }
    let (on, source, unknown) = match decided {
        None => (true, Source::Default, None),
        Some((raw, source)) => match raw.as_str() {
            "on" => (true, source, None),
            "off" => (false, source, None),
            _ => (true, Source::Default, Some(Unknown { raw, source })),
        },
    };
    Setting {
        on,
        source,
        unknown,
        problems,
    }
}

/// What `path` sets `switch` to: a string's text, or any other value as TOML.
/// `None` when the file or the key is absent, or the file is skipped.
fn file_value(switch: TextSwitch, path: &Path, problems: &mut Vec<FileProblem>) -> Option<String> {
    let text = match read_bounded(path) {
        Ok(text) => text?,
        Err(reason) => {
            problems.push(FileProblem {
                path: path.to_path_buf(),
                reason,
            });
            return None;
        }
    };
    let doc = match parse_document(&text) {
        Ok(doc) => doc,
        Err(reason) => {
            problems.push(FileProblem {
                path: path.to_path_buf(),
                reason: format!("{reason}; file ignored"),
            });
            return None;
        }
    };
    let (table, name) = switch
        .key
        .split_once('.')
        .expect("a switch key is <table>.<key>");
    let item = doc.get(table)?.as_table_like()?.get(name)?;
    Some(match item.as_str() {
        Some(text) => text.to_string(),
        None => item.to_string().trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn repo(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    fn read_with(home: &Path, cwd: &Path, env: &[(&str, &str)]) -> Setting {
        let env = |name: &str| {
            env.iter()
                .find(|(var, _)| *var == name)
                .map(|(_, value)| (*value).to_string())
        };
        read(
            ROUTING,
            &Layers {
                home: Some(home),
                cwd,
                read_project: true,
                env: &env,
            },
        )
    }

    #[test]
    fn nothing_set_is_the_default() {
        let tmp = tempfile::tempdir().unwrap();
        let setting = read_with(tmp.path(), tmp.path(), &[]);
        assert!(setting.on);
        assert_eq!(setting.source, Source::Default);
        assert!(setting.unknown.is_none() && setting.problems.is_empty());
    }

    #[test]
    fn the_variable_wins_over_both_files() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        repo(&proj);
        write(&user_file(&home), "[text]\nrouting = \"on\"\n");
        write(
            &proj.join(".amplihack/config.toml"),
            "[text]\nrouting = \"on\"\n",
        );
        let setting = read_with(&home, &proj, &[(ROUTING.env, " off ")]);
        assert!(!setting.on);
        assert_eq!(setting.source, Source::Env(ROUTING.env));
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let tmp = tempfile::tempdir().unwrap();
        write(&user_file(tmp.path()), "[text]\nrouting = \"off\"\n");
        let setting = read_with(tmp.path(), tmp.path(), &[(ROUTING.env, "")]);
        assert!(!setting.on);
        assert_eq!(setting.source, Source::User(user_file(tmp.path())));
    }

    #[test]
    fn the_project_file_wins_over_the_user_file() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let proj = tmp.path().join("proj");
        repo(&proj);
        write(&user_file(&home), "[text]\nrouting = \"on\"\n");
        let project_path = proj.join(".amplihack/config.toml");
        write(&project_path, "[text]\nrouting = \"off\"\n");
        let setting = read_with(&home, &proj.join("sub"), &[]);
        assert!(!setting.on);
        assert_eq!(setting.source, Source::Project(project_path));
    }

    #[test]
    fn an_unknown_value_leaves_the_default_and_stops_the_search() {
        let tmp = tempfile::tempdir().unwrap();
        write(&user_file(tmp.path()), "[text]\nrouting = \"off\"\n");
        let setting = read_with(tmp.path(), tmp.path(), &[(ROUTING.env, "OFF")]);
        assert!(setting.on, "the default, not the user file below");
        assert_eq!(setting.source, Source::Default);
        let unknown = setting.unknown.expect("the unknown value is returned");
        assert_eq!(unknown.raw, "OFF");
        assert_eq!(unknown.source, Source::Env(ROUTING.env));

        write(&user_file(tmp.path()), "[text]\nrouting = false\n");
        let setting = read_with(tmp.path(), tmp.path(), &[]);
        assert!(setting.on);
        assert_eq!(setting.unknown.unwrap().raw, "false");
    }

    #[test]
    fn a_bad_file_is_skipped_and_returned_as_a_problem() {
        let tmp = tempfile::tempdir().unwrap();
        write(&user_file(tmp.path()), "[text\nrouting = \"off\"\n");
        let setting = read_with(tmp.path(), tmp.path(), &[]);
        assert!(setting.on);
        assert_eq!(setting.source, Source::Default);
        assert_eq!(setting.problems.len(), 1);

        fs::remove_file(user_file(tmp.path())).unwrap();
        fs::create_dir_all(user_file(tmp.path())).unwrap();
        let setting = read_with(tmp.path(), tmp.path(), &[]);
        assert!(setting.on);
        assert_eq!(setting.problems.len(), 1);
    }
}
