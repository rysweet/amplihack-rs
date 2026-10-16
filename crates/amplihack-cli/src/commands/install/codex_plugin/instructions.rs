//! Portable generated instruction skills.
use super::*;
pub(super) fn portable_instruction(source: &str) -> String {
    let body = if let Some(rest) = source.strip_prefix("---\n") {
        rest.split_once("\n---\n")
            .map(|(_, body)| body)
            .unwrap_or(source)
    } else {
        source
    };
    body.replace(
        "Use GPT-4 to analyze code + comments",
        "Analyze code + comments",
    )
}
pub(super) fn instruction_skills(source: &Path, output: &Path, prefix: &str) -> Result<()> {
    instruction_skills_under(source, source, output, prefix)
}
pub(super) fn instruction_skills_under(
    source: &Path,
    base: &Path,
    output: &Path,
    prefix: &str,
) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    for e in fs::read_dir(source)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            instruction_skills_under(&e.path(), base, output, prefix)?;
            continue;
        }
        if e.path().extension().is_none_or(|x| x != "md") {
            continue;
        }
        let stem = e
            .path()
            .strip_prefix(base)?
            .with_extension("")
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("-");
        ensure!(
            stem.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "invalid instruction skill name"
        );
        let name = format!("amplihack-{prefix}-{stem}");
        let dir = output.join(&name);
        ensure!(!dir.exists(), "generated Codex skill collision: {name}");
        fs::create_dir(&dir)?;
        let source_body = fs::read_to_string(e.path())?;
        let body = portable_instruction(&source_body);
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Amplihack {prefix} instructions for {stem}\n---\n\n{body}"
            ),
        )?;
    }
    Ok(())
}
