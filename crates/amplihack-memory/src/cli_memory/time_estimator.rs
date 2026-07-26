use super::scip_indexing::{
    LANGUAGE_ORDER, language_for_path, normalize_languages, should_ignore_dir,
};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct IndexTimeEstimate {
    pub total_seconds: f64,
    pub by_language: BTreeMap<String, f64>,
    pub file_counts: BTreeMap<String, usize>,
}

pub fn estimate_indexing_time(project_path: &Path, languages: &[String]) -> IndexTimeEstimate {
    let requested_languages = if languages.is_empty() {
        LANGUAGE_ORDER
            .iter()
            .map(|language| (*language).to_string())
            .collect::<Vec<_>>()
    } else {
        normalize_languages(languages)
    };

    let file_counts = count_files_by_language(project_path, &requested_languages);
    let mut by_language = BTreeMap::new();
    let mut total_seconds = 0.0;
    for language in &requested_languages {
        let file_count = file_counts.get(language).copied().unwrap_or(0);
        let seconds = if file_count == 0 {
            0.0
        } else {
            file_count as f64 / indexing_rate(language)
        };
        by_language.insert(language.clone(), seconds);
        total_seconds += seconds;
    }

    IndexTimeEstimate {
        total_seconds,
        by_language,
        file_counts,
    }
}

fn count_files_by_language(project_path: &Path, languages: &[String]) -> BTreeMap<String, usize> {
    let mut counts = languages
        .iter()
        .cloned()
        .map(|language| (language, 0usize))
        .collect::<BTreeMap<_, _>>();
    scan_language_counts(project_path, &mut counts);
    counts
}

fn scan_language_counts(path: &Path, counts: &mut BTreeMap<String, usize>) {
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        let entry_path = entry.path();
        let file_name = entry.file_name().to_string_lossy().to_string();
        if entry_path.is_dir() {
            if should_ignore_dir(&file_name) {
                continue;
            }
            scan_language_counts(&entry_path, counts);
            continue;
        }
        if !entry_path.is_file() {
            continue;
        }
        let Some(language) = language_for_path(&entry_path) else {
            continue;
        };
        if let Some(count) = counts.get_mut(language) {
            *count += 1;
        }
    }
}

fn indexing_rate(language: &str) -> f64 {
    match language {
        "python" => 20.0,
        "typescript" => 15.0,
        "javascript" => 15.0,
        "go" => 25.0,
        "rust" => 10.0,
        "csharp" => 15.0,
        "cpp" => 15.0,
        _ => 15.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn estimate_indexing_time_reports_language_breakdown() {
        let project = tempfile::tempdir().unwrap();
        fs::create_dir_all(project.path().join("src")).unwrap();
        fs::write(project.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(project.path().join("src/lib.rs"), "pub fn x() {}\n").unwrap();
        fs::write(project.path().join("src/app.py"), "print('hi')\n").unwrap();
        fs::write(project.path().join("src/app.ts"), "export {};\n").unwrap();
        fs::create_dir_all(project.path().join("node_modules/pkg")).unwrap();
        fs::write(
            project.path().join("node_modules/pkg/ignored.js"),
            "console.log('ignored')\n",
        )
        .unwrap();

        let estimate = estimate_indexing_time(project.path(), &[]);

        assert_eq!(estimate.file_counts.get("python"), Some(&1));
        assert_eq!(estimate.file_counts.get("typescript"), Some(&1));
        assert_eq!(estimate.file_counts.get("rust"), Some(&2));
        assert_eq!(estimate.file_counts.get("javascript"), Some(&0));
        assert_eq!(estimate.by_language.get("rust"), Some(&(2.0 / 10.0)));
        assert!(estimate.total_seconds > 0.0);
    }
}
