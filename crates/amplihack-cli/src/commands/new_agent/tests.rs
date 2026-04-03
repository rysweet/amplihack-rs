use super::*;
use super::{distributor, packager, update_manager};
use std::collections::HashMap;
use tempfile::TempDir;

fn write_prompt(dir: &Path, content: &str) -> PathBuf {
    let p = dir.join("prompt.md");
    fs::write(&p, content).unwrap();
    p
}

#[test]
fn test_sanitize_bundle_name_basic() {
    assert_eq!(sanitize_bundle_name("my agent", "-agent"), "my-agent-agent");
    assert_eq!(sanitize_bundle_name("TEST_NAME", ""), "test-name");
    assert_eq!(sanitize_bundle_name("", "-agent"), "agent-agent");
}

#[test]
fn test_sanitize_bundle_name_long() {
    let long = "a".repeat(60);
    let result = sanitize_bundle_name(&long, "");
    assert!(result.len() <= 50, "got len={}", result.len());
    assert!(result.len() >= 3);
}

#[test]
fn test_sanitize_bundle_name_special_chars() {
    let result = sanitize_bundle_name("Test@#$Name!!!", "-agent");
    assert!(!result.contains('@'));
    assert!(!result.contains('#'));
    assert!(!result.contains('$'));
    assert!(!result.contains('!'));
}

#[test]
fn test_analyze_prompt_basic() {
    let prompt = "# Build a data pipeline\n\nProcess CSV files and generate reports.";
    let goal = analysis::analyze_prompt(prompt).unwrap();
    assert!(!goal.goal.is_empty());
    assert!(!goal.domain.is_empty());
    assert!(!goal.complexity.is_empty());
}

#[test]
fn test_analyze_prompt_empty_fails() {
    assert!(analysis::analyze_prompt("").is_err());
    assert!(analysis::analyze_prompt("   ").is_err());
}

#[test]
fn test_domain_classification() {
    assert_eq!(
        analysis::classify_domain("scan for security vulnerabilities"),
        "security-analysis"
    );
    assert_eq!(
        analysis::classify_domain("deploy the application to production"),
        "deployment"
    );
    assert_eq!(
        analysis::classify_domain("test the API endpoints"),
        "testing"
    );
    assert_eq!(
        analysis::classify_domain("process and transform data"),
        "data-processing"
    );
    assert_eq!(
        analysis::classify_domain("something completely generic"),
        "general"
    );
}

#[test]
fn test_complexity_detection() {
    assert_eq!(
        analysis::determine_complexity("simple one step task"),
        "simple"
    );
    assert_eq!(
        analysis::determine_complexity("complex distributed multi-stage pipeline"),
        "complex"
    );
    // word count heuristic
    let long_prompt = "word ".repeat(200);
    assert_eq!(analysis::determine_complexity(&long_prompt), "complex");
}

#[test]
fn test_e2e_creates_expected_files() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "# Automate deployment\n\nDeploy to production.");
    let out = tmp.path().join("out");

    run_new(
        &prompt_path,
        Some(&out),
        None,
        None,
        false,
        false,
        "copilot",
        false,
        false,
    )
    .unwrap();

    // find the agent dir (name is auto-generated)
    let entries: Vec<_> = fs::read_dir(&out).unwrap().collect();
    assert_eq!(entries.len(), 1, "expected exactly one agent directory");
    let agent_dir = entries[0].as_ref().unwrap().path();

    assert!(agent_dir.join("prompt.md").exists(), "prompt.md missing");
    assert!(agent_dir.join("main.py").exists(), "main.py missing");
    assert!(agent_dir.join("README.md").exists(), "README.md missing");
    assert!(
        agent_dir.join("agent_config.json").exists(),
        "agent_config.json missing"
    );
    assert!(
        agent_dir.join("requirements.txt").exists(),
        "requirements.txt missing"
    );
    assert!(
        agent_dir
            .join(".claude")
            .join("context")
            .join("goal.json")
            .exists()
    );
    assert!(
        agent_dir
            .join(".claude")
            .join("context")
            .join("execution_plan.json")
            .exists()
    );
}

#[test]
fn test_memory_mode_writes_artifacts() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "# Process data\n\nAnalyze CSV files.");
    let out = tmp.path().join("out");

    run_new(
        &prompt_path,
        Some(&out),
        Some("mem-agent"),
        None,
        false,
        true,
        "copilot",
        false,
        false,
    )
    .unwrap();

    let agent_dir = out.join("mem-agent");
    assert!(
        agent_dir.join("memory_config.yaml").exists(),
        "memory_config.yaml missing"
    );
    assert!(
        agent_dir.join("memory").join(".gitignore").exists(),
        "memory/.gitignore missing"
    );
    let reqs = fs::read_to_string(agent_dir.join("requirements.txt")).unwrap();
    assert!(
        reqs.contains("amplihack-memory-lib"),
        "memory dep missing from requirements.txt"
    );
}

#[test]
fn test_multi_agent_mode_writes_sub_agent_configs() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(
        tmp.path(),
        "# Orchestrate multiple services\n\nCoordinate deployment.",
    );
    let out = tmp.path().join("out");

    run_new(
        &prompt_path,
        Some(&out),
        Some("multi-agent"),
        None,
        false,
        false,
        "claude",
        true,
        false,
    )
    .unwrap();

    let agent_dir = out.join("multi-agent");
    assert!(
        agent_dir
            .join("sub_agents")
            .join("coordinator.yaml")
            .exists()
    );
    assert!(
        agent_dir
            .join("sub_agents")
            .join("memory_agent.yaml")
            .exists()
    );
    assert!(agent_dir.join("sub_agents").join("spawner.yaml").exists());
    assert!(agent_dir.join("sub_agents").join("__init__.py").exists());
    let reqs = fs::read_to_string(agent_dir.join("requirements.txt")).unwrap();
    assert!(
        reqs.contains("pyyaml"),
        "pyyaml missing from requirements.txt"
    );
}

#[test]
fn test_enable_spawning_implies_multi_agent() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "# Spawn workers\n\nDynamic task spawning.");
    let out = tmp.path().join("out");

    // enable_spawning=true, multi_agent=false — should auto-enable multi-agent
    run_new(
        &prompt_path,
        Some(&out),
        Some("spawner-test"),
        None,
        false,
        false,
        "copilot",
        false,
        true,
    )
    .unwrap();

    let agent_dir = out.join("spawner-test");
    // sub_agents directory should exist (multi-agent was auto-enabled)
    assert!(agent_dir.join("sub_agents").exists());
    // spawner.yaml should have enabled: true
    let spawner = fs::read_to_string(agent_dir.join("sub_agents").join("spawner.yaml")).unwrap();
    assert!(spawner.contains("enabled: true"));
}

#[test]
fn test_missing_skills_dir_falls_back_to_generic() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "# Process data\n\nTransform and analyze.");
    let out = tmp.path().join("out");

    // Pass a non-existent skills dir
    let nonexistent = tmp.path().join("no_skills_here");
    run_new(
        &prompt_path,
        Some(&out),
        Some("fallback-test"),
        Some(&nonexistent),
        false,
        false,
        "copilot",
        false,
        false,
    )
    .unwrap();

    let agents_dir = out.join("fallback-test").join(".claude").join("agents");
    let skill_files: Vec<_> = fs::read_dir(&agents_dir).unwrap().collect();
    assert!(
        !skill_files.is_empty(),
        "expected at least one generic skill file"
    );
}

#[test]
fn test_custom_name_is_sanitized() {
    let tmp = TempDir::new().unwrap();
    let prompt_path = write_prompt(tmp.path(), "Deploy to staging.");
    let out = tmp.path().join("out");

    run_new(
        &prompt_path,
        Some(&out),
        Some("My Custom Name!!!"),
        None,
        false,
        false,
        "copilot",
        false,
        false,
    )
    .unwrap();

    // Sanitized name should exist (special chars stripped)
    let entries: Vec<_> = fs::read_dir(&out).unwrap().collect();
    assert_eq!(entries.len(), 1);
    let dir_name = entries[0].as_ref().unwrap().file_name();
    let dir_str = dir_name.to_string_lossy();
    assert!(!dir_str.contains('!'), "name should be sanitized");
}

// ===========================================================================
// packager tests
// ===========================================================================

fn make_sample_dir(tmp: &TempDir) -> PathBuf {
    let dir = tmp.path().join("sample-bundle");
    fs::create_dir_all(dir.join("agents")).expect("mkdir");
    fs::write(dir.join("README.md"), "# Test Bundle").expect("write");
    fs::write(dir.join("agents/scanner.md"), "scanner skill").expect("write");
    fs::write(dir.join("main.py"), "print('hello')").expect("write");
    dir
}

#[test]
fn test_packager_tar_gz() {
    let tmp = TempDir::new().expect("tmpdir");
    let src = make_sample_dir(&tmp);
    let pkg = packager::FileSystemPackager::new(None);
    let opts = packager::PackageOptions::default();
    let res = packager::Packager::package(&pkg, &src, models::PackageFormat::TarGz, &opts)
        .expect("package");
    assert_eq!(res.format, models::PackageFormat::TarGz);
    assert!(res.path.exists());
    assert!(res.size_bytes > 0);
    assert!(!res.checksum.is_empty());
}

#[test]
fn test_packager_zip() {
    let tmp = TempDir::new().expect("tmpdir");
    let src = make_sample_dir(&tmp);
    let pkg = packager::FileSystemPackager::new(None);
    let res = packager::Packager::package(
        &pkg,
        &src,
        models::PackageFormat::Zip,
        &packager::PackageOptions::default(),
    )
    .expect("package");
    assert_eq!(res.format, models::PackageFormat::Zip);
    assert!(res.path.exists());
    assert!(res.size_bytes > 0);
}

#[test]
fn test_packager_directory() {
    let tmp = TempDir::new().expect("tmpdir");
    let src = make_sample_dir(&tmp);
    let dest = tmp.path().join("output");
    let pkg = packager::FileSystemPackager::new(None);
    let opts = packager::PackageOptions {
        output_dir: Some(dest),
        ..Default::default()
    };
    let res = packager::Packager::package(&pkg, &src, models::PackageFormat::Directory, &opts)
        .expect("package");
    assert!(res.path.join("README.md").exists());
    assert!(res.path.join("agents/scanner.md").exists());
}

#[test]
fn test_packager_uvx_uses_tarball() {
    let tmp = TempDir::new().expect("tmpdir");
    let src = make_sample_dir(&tmp);
    let pkg = packager::FileSystemPackager::new(None);
    let res = packager::Packager::package(
        &pkg,
        &src,
        models::PackageFormat::Uvx,
        &packager::PackageOptions::default(),
    )
    .expect("package");
    assert!(res.path.to_string_lossy().ends_with(".tar.gz"));
}

#[test]
fn test_packager_nonexistent_source_fails() {
    let pkg = packager::FileSystemPackager::new(None);
    let err = packager::Packager::package(
        &pkg,
        Path::new("/nonexistent/dir"),
        models::PackageFormat::TarGz,
        &packager::PackageOptions::default(),
    )
    .unwrap_err();
    assert_eq!(err.kind, error::BundleErrorKind::Packaging);
}

#[test]
fn test_sha256_file_deterministic() {
    let tmp = TempDir::new().expect("tmpdir");
    let file = tmp.path().join("data.txt");
    fs::write(&file, "hello world").expect("write");
    let a = packager::sha256_file(&file).expect("hash");
    let b = packager::sha256_file(&file).expect("hash");
    assert_eq!(a, b);
    assert!(!a.is_empty());
}

#[test]
fn test_sha256_bytes() {
    let h = packager::sha256_bytes(b"test");
    assert_eq!(h.len(), 64);
}

#[test]
fn test_packager_list_tar_gz() {
    let tmp = TempDir::new().expect("tmpdir");
    let src = make_sample_dir(&tmp);
    let pkg = packager::FileSystemPackager::new(None);
    let res = packager::Packager::package(
        &pkg,
        &src,
        models::PackageFormat::TarGz,
        &packager::PackageOptions::default(),
    )
    .expect("package");
    let contents = packager::Packager::list_contents(&pkg, &res.path).expect("list");
    assert!(!contents.is_empty());
}

#[test]
fn test_packager_list_dir() {
    let tmp = TempDir::new().expect("tmpdir");
    let src = make_sample_dir(&tmp);
    let pkg = packager::FileSystemPackager::new(None);
    let contents = packager::Packager::list_contents(&pkg, &src).expect("list");
    let has_readme = contents.iter().any(|c: &String| c.contains("README.md"));
    assert!(has_readme);
}

#[test]
fn test_into_packaged_bundle() {
    let pr = packager::PackageResult {
        format: models::PackageFormat::TarGz,
        path: PathBuf::from("out.tar.gz"),
        size_bytes: 4096,
        checksum: "abc".to_string(),
    };
    let meta = HashMap::from([("v".to_string(), "1".to_string())]);
    let pb = packager::into_packaged_bundle(pr, meta);
    assert_eq!(pb.size_bytes, 4096);
    assert_eq!(pb.metadata["v"], "1");
}

#[test]
fn test_hidden_files_excluded_by_default() {
    let tmp = TempDir::new().expect("tmpdir");
    let src = make_sample_dir(&tmp);
    fs::write(src.join(".hidden"), "secret").expect("write");
    let pkg = packager::FileSystemPackager::new(None);
    let res = packager::Packager::package(
        &pkg,
        &src,
        models::PackageFormat::TarGz,
        &packager::PackageOptions::default(),
    )
    .expect("package");
    let contents = packager::Packager::list_contents(&pkg, &res.path).expect("list");
    assert!(
        !contents.iter().any(|c: &String| c.contains(".hidden")),
        "hidden file should be excluded"
    );
}

#[test]
fn test_hidden_files_included_when_requested() {
    let tmp = TempDir::new().expect("tmpdir");
    let src = make_sample_dir(&tmp);
    fs::write(src.join(".hidden"), "secret").expect("write");
    let pkg = packager::FileSystemPackager::new(None);
    let opts = packager::PackageOptions {
        include_hidden: true,
        ..Default::default()
    };
    let res =
        packager::Packager::package(&pkg, &src, models::PackageFormat::TarGz, &opts).expect("ok");
    let contents = packager::Packager::list_contents(&pkg, &res.path).expect("list");
    assert!(
        contents.iter().any(|c: &String| c.contains(".hidden")),
        "hidden file should be included"
    );
}

// ===========================================================================
// distributor tests
// ===========================================================================

fn sample_package(tmp: &TempDir) -> models::PackagedBundle {
    let file = tmp.path().join("bundle.tar.gz");
    fs::write(&file, b"fake archive data").expect("write");
    models::PackagedBundle {
        format: models::PackageFormat::TarGz,
        path: file,
        size_bytes: 17,
        checksum: "abc123".to_string(),
        metadata: HashMap::new(),
    }
}

#[test]
fn test_distributor_new_defaults() {
    let d = distributor::Distributor::new(None, None);
    assert_eq!(d.default_branch, "main");
    assert!(d.organization.is_none());
}

#[test]
fn test_distributor_new_custom() {
    let d = distributor::Distributor::new(Some("myorg".into()), Some("develop".into()));
    assert_eq!(d.organization.as_deref(), Some("myorg"));
    assert_eq!(d.default_branch, "develop");
}

#[test]
fn test_local_distribution() {
    let tmp = TempDir::new().expect("tmpdir");
    let pkg = sample_package(&tmp);
    let target = tmp.path().join("local-dist");
    let d = distributor::Distributor::new(None, None);
    let result = d.distribute(
        &pkg,
        models::DistributionPlatform::Local,
        target.to_str().expect("path"),
        &distributor::DistributionOptions::default(),
    );
    assert!(result.success, "errors: {:?}", result.errors);
    assert!(result.url.is_some());
    assert!(target.join("bundle.tar.gz").exists());
    assert!(target.join("distribution_manifest.json").exists());
}

#[test]
fn test_local_distribution_manifest_content() {
    let tmp = TempDir::new().expect("tmpdir");
    let pkg = sample_package(&tmp);
    let target = tmp.path().join("manifested");
    let d = distributor::Distributor::new(None, None);
    d.distribute(
        &pkg,
        models::DistributionPlatform::Local,
        target.to_str().expect("path"),
        &distributor::DistributionOptions::default(),
    );
    let raw = fs::read_to_string(target.join("distribution_manifest.json")).expect("read manifest");
    let manifest: serde_json::Value = serde_json::from_str(&raw).expect("parse");
    assert_eq!(manifest["format"], "tar.gz");
    assert_eq!(manifest["checksum"], "abc123");
}

#[test]
fn test_pypi_not_supported() {
    let tmp = TempDir::new().expect("tmpdir");
    let pkg = sample_package(&tmp);
    let d = distributor::Distributor::new(None, None);
    let result = d.distribute(
        &pkg,
        models::DistributionPlatform::Pypi,
        "repo",
        &Default::default(),
    );
    assert!(!result.success);
    assert!(result.errors[0].contains("not yet supported"));
}

#[test]
fn test_github_without_org_fails() {
    let tmp = TempDir::new().expect("tmpdir");
    let pkg = sample_package(&tmp);
    let d = distributor::Distributor::new(None, None);
    let result = d.distribute(
        &pkg,
        models::DistributionPlatform::Github,
        "my-repo",
        &distributor::DistributionOptions {
            create_release: false,
            ..Default::default()
        },
    );
    assert!(!result.success);
}

#[test]
fn test_verify_checksum_match() {
    let tmp = TempDir::new().expect("tmpdir");
    let file = tmp.path().join("data.bin");
    fs::write(&file, b"hello").expect("write");
    let checksum = packager::sha256_file(&file).expect("hash");
    assert!(distributor::verify_checksum(&file, &checksum).expect("verify"));
}

#[test]
fn test_verify_checksum_mismatch() {
    let tmp = TempDir::new().expect("tmpdir");
    let file = tmp.path().join("data.bin");
    fs::write(&file, b"hello").expect("write");
    assert!(!distributor::verify_checksum(&file, "wrong").expect("verify"));
}

#[test]
fn test_verify_checksum_empty() {
    let tmp = TempDir::new().expect("tmpdir");
    let file = tmp.path().join("data.bin");
    fs::write(&file, b"hello").expect("write");
    assert!(distributor::verify_checksum(&file, "").expect("verify"));
}

#[test]
fn test_distribution_result_timestamps() {
    let tmp = TempDir::new().expect("tmpdir");
    let pkg = sample_package(&tmp);
    let target = tmp.path().join("timed");
    let d = distributor::Distributor::new(None, None);
    let result = d.distribute(
        &pkg,
        models::DistributionPlatform::Local,
        target.to_str().expect("path"),
        &Default::default(),
    );
    assert!(!result.timestamp.is_empty());
    assert!(result.distribution_time_seconds >= 0.0);
}

#[test]
fn test_local_distribution_dir_package() {
    let tmp = TempDir::new().expect("tmpdir");
    let src_dir = tmp.path().join("my-bundle");
    fs::create_dir_all(&src_dir).expect("mkdir");
    fs::write(src_dir.join("README.md"), "# Hello").expect("write");
    let pkg = models::PackagedBundle {
        format: models::PackageFormat::Directory,
        path: src_dir,
        size_bytes: 0,
        checksum: String::new(),
        metadata: HashMap::new(),
    };
    let target = tmp.path().join("dir-dist");
    let d = distributor::Distributor::new(None, None);
    let result = d.distribute(
        &pkg,
        models::DistributionPlatform::Local,
        target.to_str().expect("path"),
        &Default::default(),
    );
    assert!(result.success, "errors: {:?}", result.errors);
    assert!(target.join("my-bundle").join("README.md").exists());
}

// ===========================================================================
// update_manager tests
// ===========================================================================

fn compute_checksum_str(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("sha256:{:x}", hasher.finalize())
}

fn make_bundle_dir(tmp: &TempDir) -> PathBuf {
    let dir = tmp.path().join("my-bundle");
    fs::create_dir_all(&dir).expect("mkdir");
    let manifest = serde_json::json!({
        "framework": { "version": "abc1234", "updated_at": "2025-01-01" },
        "file_checksums": {
            "README.md": compute_checksum_str(b"# Original"),
            "main.py": compute_checksum_str(b"print('hello')"),
        },
    });
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_string(&manifest).expect("json"),
    )
    .expect("write");
    fs::write(dir.join("README.md"), "# Original").expect("write");
    fs::write(dir.join("main.py"), "print('hello')").expect("write");
    dir
}

#[test]
fn test_update_manager_new() {
    let _um = update_manager::UpdateManager::new(None);
}

#[test]
fn test_detect_customizations_none() {
    let tmp = TempDir::new().expect("tmpdir");
    let dir = make_bundle_dir(&tmp);
    let um = update_manager::UpdateManager::new(None);
    let result = um.detect_customizations(&dir).expect("detect");
    assert!(!result["README.md"]);
    assert!(!result["main.py"]);
}

#[test]
fn test_detect_customizations_modified() {
    let tmp = TempDir::new().expect("tmpdir");
    let dir = make_bundle_dir(&tmp);
    fs::write(dir.join("README.md"), "# Modified").expect("write");
    let um = update_manager::UpdateManager::new(None);
    let result = um.detect_customizations(&dir).expect("detect");
    assert!(result["README.md"]);
    assert!(!result["main.py"]);
}

#[test]
fn test_compute_checksum_deterministic() {
    let tmp = TempDir::new().expect("tmpdir");
    let file = tmp.path().join("test.txt");
    fs::write(&file, "hello world").expect("write");
    let a = update_manager::compute_checksum(&file).expect("cs");
    let b = update_manager::compute_checksum(&file).expect("cs");
    assert_eq!(a, b);
    assert!(a.starts_with("sha256:"));
}

#[test]
fn test_compute_checksum_prefix() {
    let tmp = TempDir::new().expect("tmpdir");
    let file = tmp.path().join("data.bin");
    fs::write(&file, b"test").expect("write");
    let cs = update_manager::compute_checksum(&file).expect("cs");
    assert!(cs.starts_with("sha256:"), "got: {cs}");
}

#[test]
fn test_create_backup() {
    let tmp = TempDir::new().expect("tmpdir");
    let dir = make_bundle_dir(&tmp);
    let um = update_manager::UpdateManager::new(None);
    let backup = um.create_backup(&dir).expect("backup");
    assert!(backup.exists());
    assert!(backup.join("manifest.json").exists());
    assert!(backup.join("README.md").exists());
    let name = backup.file_name().expect("name").to_string_lossy();
    assert!(name.contains("backup"), "dir: {name}");
}

#[test]
fn test_update_bundle_no_framework_repo() {
    let tmp = TempDir::new().expect("tmpdir");
    let dir = make_bundle_dir(&tmp);
    let um = update_manager::UpdateManager::new(None);
    let result = um.update_bundle(&dir, false, false);
    assert!(!result.success);
    assert!(result.error.as_deref().unwrap_or("").contains("framework"));
}

#[test]
fn test_update_bundle_with_templates() {
    let tmp = TempDir::new().expect("tmpdir");
    let dir = make_bundle_dir(&tmp);
    let framework = tmp.path().join("framework");
    let templates = framework.join("templates");
    fs::create_dir_all(&templates).expect("mkdir");
    fs::write(templates.join("README.md"), "# Updated").expect("write");
    fs::write(templates.join("new_file.txt"), "new").expect("write");
    let um = update_manager::UpdateManager::new(Some(framework));
    let result = um.update_bundle(&dir, false, false);
    assert!(result.success, "error: {:?}", result.error);
    assert!(result.updated_files.contains(&"README.md".to_string()));
    assert!(result.updated_files.contains(&"new_file.txt".to_string()));
    assert!(dir.join("new_file.txt").exists());
}

#[test]
fn test_update_bundle_preserves_edits() {
    let tmp = TempDir::new().expect("tmpdir");
    let dir = make_bundle_dir(&tmp);
    fs::write(dir.join("README.md"), "# Custom").expect("write");
    let framework = tmp.path().join("framework");
    let templates = framework.join("templates");
    fs::create_dir_all(&templates).expect("mkdir");
    fs::write(templates.join("README.md"), "# Framework").expect("write");
    fs::write(templates.join("main.py"), "print('updated')").expect("write");
    let um = update_manager::UpdateManager::new(Some(framework));
    let result = um.update_bundle(&dir, true, false);
    assert!(result.success, "error: {:?}", result.error);
    assert!(result.preserved_files.contains(&"README.md".to_string()));
    assert!(result.updated_files.contains(&"main.py".to_string()));
    let content = fs::read_to_string(dir.join("README.md")).expect("read");
    assert_eq!(content, "# Custom");
}

#[test]
fn test_update_info_serde_roundtrip() {
    let info = update_manager::UpdateInfo {
        available: true,
        current_version: "abc".into(),
        latest_version: "def".into(),
        changes: vec!["fix bug".into()],
    };
    let json = serde_json::to_string(&info).expect("ser");
    let r: update_manager::UpdateInfo = serde_json::from_str(&json).expect("de");
    assert!(r.available);
    assert_eq!(r.changes.len(), 1);
}

#[test]
fn test_update_result_serde_roundtrip() {
    let result = update_manager::UpdateResult {
        success: false,
        updated_files: Vec::new(),
        preserved_files: Vec::new(),
        conflicts: Vec::new(),
        error: Some("oops".into()),
    };
    let json = serde_json::to_string(&result).expect("ser");
    let r: update_manager::UpdateResult = serde_json::from_str(&json).expect("de");
    assert!(!r.success);
    assert_eq!(r.error.as_deref(), Some("oops"));
}
