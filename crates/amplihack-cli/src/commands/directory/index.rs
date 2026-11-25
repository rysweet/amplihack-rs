//! The index of installed skills and agents (issue #3220).
//!
//! `amplihack install` stages amplihack's own bundle whole under
//! `<home>/.amplihack/.claude` and then calls [`write_index`], which writes
//! one tab-separated line for each skill and agent found there to
//! `<staged>/install/skills-and-agents-index.tsv`, beside the skill ownership
//! manifest. `amplihack directory` reads the same file through
//! [`index_path`], so the writer and the reader always agree on its place.
//!
//! Each line is `type \t name \t absolute path \t description \n`. Lines are
//! sorted by type, then name, then path, and the file holds nothing else, so
//! two installs of the same bundle write the same bytes.
//!
//! The index is built from the staged `skills` and `agents` trees only. The
//! user's own skills live in `<home>/.claude/skills` and commands are never
//! read, so neither can appear in it.

use anyhow::{Context, Result, bail};
use std::fs;
use std::path::{Path, PathBuf};

/// File name of the index, in the `install` folder of the staged framework.
pub const INDEX_FILE_NAME: &str = "skills-and-agents-index.tsv";

/// Whether an entry is a skill or an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    // Declared in byte order of their labels, so the derived order matches
    // the order of the lines in the file.
    Agent,
    Skill,
}

impl EntryKind {
    pub fn label(self) -> &'static str {
        match self {
            EntryKind::Agent => "agent",
            EntryKind::Skill => "skill",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        match label {
            "agent" => Some(EntryKind::Agent),
            "skill" => Some(EntryKind::Skill),
            _ => None,
        }
    }
}

/// One line of the index.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub kind: EntryKind,
    pub name: String,
    pub path: PathBuf,
    pub description: String,
}

impl Entry {
    /// True when every word occurs in the name or the description, without
    /// regard to letter case. The path is not searched.
    pub fn matches_all(&self, words: &[String]) -> bool {
        let haystack = format!("{} {}", self.name, self.description).to_lowercase();
        words
            .iter()
            .all(|word| haystack.contains(&word.to_lowercase()))
    }
}

/// Where the index of the staged framework at `staged` lives.
pub fn index_path(staged: &Path) -> PathBuf {
    staged.join("install").join(INDEX_FILE_NAME)
}

/// Collect every skill and agent under `staged`, sorted in index order.
///
/// A folder that contains a `SKILL.md` is a skill, named after that folder.
/// Every `*.md` file under `agents` is an agent, named by its front matter
/// (`name`, then `meta.name`) or else by its file stem. A file whose front
/// matter cannot be read still gets an entry, with an empty description, and
/// a warning on stderr.
pub fn collect_entries(staged: &Path) -> Result<Vec<Entry>> {
    let staged = std::path::absolute(staged)
        .with_context(|| format!("failed to make {} absolute", staged.display()))?;
    let mut entries = Vec::new();
    collect_skills(&staged.join("skills"), &mut entries)?;
    collect_agents(&staged.join("agents"), &mut entries)?;
    entries.sort();
    Ok(entries)
}

/// The bytes of the index for `entries`, in the order given.
///
/// Fails when a path holds a tab or a line break, since such a path cannot
/// be written as one field of one line.
pub fn render(entries: &[Entry]) -> Result<String> {
    let mut out = String::new();
    for entry in entries {
        let path = entry
            .path
            .to_str()
            .with_context(|| format!("path is not valid UTF-8: {}", entry.path.display()))?;
        if path.contains(['\t', '\n', '\r']) {
            bail!(
                "path holds a tab or a line break: {}",
                super::super::install::paths::sanitize_text(path)
            );
        }
        out.push_str(entry.kind.label());
        out.push('\t');
        out.push_str(&entry.name);
        out.push('\t');
        out.push_str(path);
        out.push('\t');
        out.push_str(&entry.description);
        out.push('\n');
    }
    Ok(out)
}

/// Build the index of the framework staged at `staged` and write it to
/// [`index_path`]. The file is replaced through a rename, so a reader never
/// sees half of it, and it is not rewritten when its bytes already match.
///
/// When the index cannot be built or written, any index left from an earlier
/// install is removed before the error is returned. Its paths point into the
/// staging the install has just replaced, so keeping it would let
/// `amplihack directory` serve stale paths; without it the command reports
/// the missing index and names `amplihack install`.
pub fn write_index(staged: &Path) -> Result<PathBuf> {
    let result = build_and_write_index(staged);
    if result.is_err() {
        let path = index_path(staged);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return result.with_context(|| {
                    format!(
                        "the previous index at {} could not be removed either ({error}) \
                         and may list stale paths",
                        path.display()
                    )
                });
            }
        }
    }
    result
}

fn build_and_write_index(staged: &Path) -> Result<PathBuf> {
    let entries = collect_entries(staged)?;
    let rendered = render(&entries)?;
    let path = index_path(staged);
    if fs::read(&path).is_ok_and(|current| current == rendered.as_bytes()) {
        return Ok(path);
    }
    let parent = path.parent().context("index path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temporary = parent.join(format!(".{INDEX_FILE_NAME}.{}", uuid::Uuid::new_v4()));
    fs::write(&temporary, rendered.as_bytes())
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    if let Err(error) = fs::rename(&temporary, &path) {
        let _ = fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("failed to replace {}", path.display()));
    }
    Ok(path)
}

/// Read the index at `path`. Returns `Ok(None)` when the file does not exist.
///
/// A line without exactly four fields, or with an unknown type, is skipped
/// with a warning on stderr that gives its line number.
pub fn read_index(path: &Path) -> Result<Option<Vec<Entry>>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    Ok(Some(parse_index(&text)))
}

fn parse_index(text: &str) -> Vec<Entry> {
    let mut entries = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let fields: Vec<&str> = line.split('\t').collect();
        let kind = match fields.as_slice() {
            [kind, _, _, _] => EntryKind::parse(kind),
            _ => None,
        };
        let Some(kind) = kind else {
            eprintln!(
                "amplihack: warning: skipping malformed line {} of the skills and agents index",
                number + 1
            );
            continue;
        };
        entries.push(Entry {
            kind,
            name: fields[1].to_owned(),
            path: PathBuf::from(fields[2]),
            description: fields[3].to_owned(),
        });
    }
    entries
}

fn collect_skills(root: &Path, entries: &mut Vec<Entry>) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut skill_file = None;
    for entry in sorted_dir_entries(root)? {
        let file_type = entry.file_type()?;
        if file_type.is_file() && entry.file_name() == "SKILL.md" {
            skill_file = Some(entry.path());
        } else if file_type.is_dir() {
            collect_skills(&entry.path(), entries)?;
        }
    }
    if let Some(file) = skill_file {
        let name = root
            .file_name()
            .map(|name| one_line(&name.to_string_lossy()))
            .unwrap_or_default();
        let description = read_front_matter(&file)
            .map(|front| front.description)
            .unwrap_or_default();
        entries.push(Entry {
            kind: EntryKind::Skill,
            name,
            path: file,
            description,
        });
    }
    Ok(())
}

fn collect_agents(root: &Path, entries: &mut Vec<Entry>) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in sorted_dir_entries(root)? {
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_agents(&path, entries)?;
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "md") {
            let front = read_front_matter(&path).unwrap_or_default();
            let name = if front.name.is_empty() {
                path.file_stem()
                    .map(|stem| one_line(&stem.to_string_lossy()))
                    .unwrap_or_default()
            } else {
                front.name
            };
            entries.push(Entry {
                kind: EntryKind::Agent,
                name,
                path,
                description: front.description,
            });
        }
    }
    Ok(())
}

fn sorted_dir_entries(dir: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("failed to read {}", dir.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    Ok(entries)
}

#[derive(Debug, Default)]
struct FrontMatter {
    name: String,
    description: String,
}

/// The `name` and `description` of the YAML front matter of `file`, each on
/// one line. Top-level keys win over the same keys under `meta`. A file with
/// no front matter gives empty fields; one whose front matter cannot be read
/// gives `None` after a warning on stderr.
fn read_front_matter(file: &Path) -> Option<FrontMatter> {
    let warn = |reason: &str| {
        eprintln!(
            "amplihack: warning: {reason} in {}; its index entry has no description",
            super::super::install::paths::sanitize_path(file)
        );
    };
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(_) => {
            warn("cannot read the file");
            return None;
        }
    };
    let Some(yaml) = front_matter_block(&text) else {
        return Some(FrontMatter::default());
    };
    let value: serde_yaml::Value = match serde_yaml::from_str(yaml) {
        Ok(value) => value,
        Err(_) => {
            warn("cannot parse the front matter");
            return None;
        }
    };
    let field = |key: &str| -> String {
        let top = value.get(key).and_then(serde_yaml::Value::as_str);
        let meta = value
            .get("meta")
            .and_then(|meta| meta.get(key))
            .and_then(serde_yaml::Value::as_str);
        top.or(meta).map(one_line).unwrap_or_default()
    };
    Some(FrontMatter {
        name: field("name"),
        description: field("description"),
    })
}

/// The text between an opening `---` line at the very start of `text` and
/// the next `---` line, or `None` when the file has no front matter.
fn front_matter_block(text: &str) -> Option<&str> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Some(&rest[..offset]);
        }
        offset += line.len();
    }
    None
}

/// `text` with every run of whitespace or control characters made one space,
/// and trimmed.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn front_matter_name_and_description_fall_back_to_meta() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path();
        write(
            &staged.join("agents/top.md"),
            "---\nname: top-agent\ndescription: >\n  Folded\n  text.\n---\nbody\n",
        );
        write(
            &staged.join("agents/core/meta.md"),
            "---\nmeta:\n  name: meta-agent\n  description: From meta.\n---\nbody\n",
        );
        write(&staged.join("agents/plain.md"), "# No front matter\n");
        write(
            &staged.join("agents/broken.md"),
            "---\nname: [unclosed\n---\n",
        );

        let entries = collect_entries(staged).unwrap();
        let by_name = |name: &str| entries.iter().find(|e| e.name == name).unwrap();
        assert_eq!(by_name("top-agent").description, "Folded text.");
        assert_eq!(by_name("meta-agent").description, "From meta.");
        assert_eq!(by_name("plain").description, "");
        assert_eq!(by_name("broken").description, "");
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn nested_skills_take_their_leaf_folder_name_and_lines_are_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path();
        write(
            &staged.join("skills/zeta/SKILL.md"),
            "---\nname: other\ndescription: Z skill\n---\n",
        );
        write(
            &staged.join("skills/group/alpha/SKILL.md"),
            "---\ndescription: \"A\\tskill\\nwith breaks\"\n---\n",
        );
        write(
            &staged.join("agents/b.md"),
            "---\nname: b\ndescription: B\n---\n",
        );

        let entries = collect_entries(staged).unwrap();
        let rendered = render(&entries).unwrap();
        let lines: Vec<Vec<&str>> = rendered
            .lines()
            .map(|line| line.split('\t').collect())
            .collect();
        let keys: Vec<(&str, &str)> = lines.iter().map(|f| (f[0], f[1])).collect();
        assert_eq!(
            keys,
            [("agent", "b"), ("skill", "alpha"), ("skill", "zeta")]
        );
        assert!(lines.iter().all(|fields| fields.len() == 4));
        assert_eq!(lines[1][3], "A skill with breaks");
        assert!(Path::new(lines[1][2]).ends_with("skills/group/alpha/SKILL.md"));
        assert_eq!(parse_index(&rendered), entries);
    }

    #[test]
    fn render_rejects_a_path_with_a_tab() {
        let entry = Entry {
            kind: EntryKind::Skill,
            name: "x".into(),
            path: PathBuf::from("/a\tb/SKILL.md"),
            description: String::new(),
        };
        assert!(render(&[entry]).is_err());
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let entries = parse_index("skill\ta\t/p/SKILL.md\td\nbad line\nthing\tb\t/p\td\n");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a");
    }

    #[test]
    fn matching_ignores_case_needs_every_word_and_skips_the_path() {
        let entry = Entry {
            kind: EntryKind::Skill,
            name: "azure-admin".into(),
            path: PathBuf::from("/x/kubernetes/SKILL.md"),
            description: "Manage Entra ID and RBAC".into(),
        };
        let words = |list: &[&str]| list.iter().map(|w| (*w).to_owned()).collect::<Vec<_>>();
        assert!(entry.matches_all(&words(&["AZURE", "rbac"])));
        assert!(entry.matches_all(&words(&["entra id"])));
        assert!(!entry.matches_all(&words(&["azure", "nomatch"])));
        assert!(!entry.matches_all(&words(&["kubernetes"])));
    }

    #[test]
    fn write_index_is_stable_and_absent_index_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path();
        assert!(read_index(&index_path(staged)).unwrap().is_none());
        write(
            &staged.join("skills/one/SKILL.md"),
            "---\ndescription: One\n---\n",
        );
        let path = write_index(staged).unwrap();
        let first = fs::read(&path).unwrap();
        write_index(staged).unwrap();
        assert_eq!(fs::read(&path).unwrap(), first);
        assert_eq!(read_index(&path).unwrap().unwrap().len(), 1);
    }

    #[test]
    fn a_failed_rewrite_removes_the_previous_index() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path();
        write(
            &staged.join("skills/one/SKILL.md"),
            "---\ndescription: One\n---\n",
        );
        let path = write_index(staged).unwrap();
        assert!(path.is_file());
        // A skill folder whose name holds a tab cannot be rendered, so the
        // rewrite fails, as it would on an update that cannot write the file.
        write(
            &staged.join("skills/bad\tname/SKILL.md"),
            "---\ndescription: Bad\n---\n",
        );
        assert!(write_index(staged).is_err());
        assert!(
            !path.exists(),
            "the stale index from the earlier install survived"
        );
        assert!(read_index(&path).unwrap().is_none());
    }
}
