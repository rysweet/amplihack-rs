//! DockerManager — image building and container execution.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::env;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{DEFAULT_IMAGE_NAME, DockerDetector, helpers::forwarded_env_vars};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(unix)]
fn nix_uid() -> u32 {
    // SAFETY: libc getter has no preconditions.
    unsafe { libc::geteuid() }
}

#[cfg(unix)]
fn nix_gid() -> u32 {
    // SAFETY: libc getter has no preconditions.
    unsafe { libc::getegid() }
}

#[derive(Debug, Clone)]
pub(crate) struct DockerManager {
    project_root: PathBuf,
    image_name: &'static str,
    detector: DockerDetector,
}

impl Default for DockerManager {
    fn default() -> Self {
        Self {
            project_root: workspace_root(),
            image_name: DEFAULT_IMAGE_NAME,
            detector: DockerDetector,
        }
    }
}

impl DockerManager {
    #[cfg(test)]
    pub(crate) fn new_for_tests(project_root: PathBuf) -> Self {
        Self {
            project_root,
            image_name: DEFAULT_IMAGE_NAME,
            detector: DockerDetector,
        }
    }

    pub(crate) fn run_command(&self, amplihack_args: &[String], cwd: &Path) -> Result<i32> {
        if !self.detector.is_running() {
            eprintln!("Docker is not running.");
            return Ok(1);
        }

        if !self.build_image()? {
            eprintln!("Failed to build Docker image.");
            return Ok(1);
        }

        let run_args = self.build_run_args(cwd, amplihack_args, env::vars_os());
        let status = Command::new("docker")
            .args(&run_args)
            .status()
            .context("failed to execute docker run")?;
        Ok(status.code().unwrap_or(1))
    }

    fn build_image(&self) -> Result<bool> {
        if self.detector.check_image_exists(self.image_name) {
            return Ok(true);
        }

        let dockerfile = self.project_root.join("Dockerfile");
        if !dockerfile.is_file() {
            eprintln!("Dockerfile not found at {}", dockerfile.display());
            return Ok(false);
        }

        println!("Building Docker image: {}", self.image_name);
        let status = Command::new("docker")
            .args(self.build_image_args(&dockerfile))
            .status()
            .context("failed to execute docker build")?;
        if !status.success() {
            eprintln!("Docker build failed.");
            return Ok(false);
        }

        println!("Successfully built Docker image: {}", self.image_name);
        Ok(true)
    }

    pub(crate) fn build_image_args(&self, dockerfile: &Path) -> Vec<String> {
        vec![
            "build".to_string(),
            "-t".to_string(),
            self.image_name.to_string(),
            "-f".to_string(),
            dockerfile.display().to_string(),
            self.project_root.display().to_string(),
        ]
    }

    pub(crate) fn build_run_args<I, K, V>(
        &self,
        cwd: &Path,
        amplihack_args: &[String],
        env_vars: I,
    ) -> Vec<String>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<std::ffi::OsString>,
        V: Into<std::ffi::OsString>,
    {
        let workspace_dir = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--interactive".to_string(),
        ];
        if std::io::stdin().is_terminal() {
            args.push("--tty".to_string());
        }
        args.extend([
            "--security-opt".to_string(),
            "no-new-privileges".to_string(),
            "--memory".to_string(),
            "4g".to_string(),
            "--cpus".to_string(),
            "2".to_string(),
        ]);
        #[cfg(unix)]
        {
            args.extend(["--user".to_string(), format!("{}:{}", nix_uid(), nix_gid())]);
        }
        args.extend([
            "-v".to_string(),
            format!("{}:/workspace", workspace_dir.display()),
            "-w".to_string(),
            "/workspace".to_string(),
        ]);

        for (key, value) in forwarded_env_vars(env_vars) {
            args.extend(["-e".to_string(), format!("{key}={value}")]);
        }

        args.push(self.image_name.to_string());
        args.extend(amplihack_args.iter().cloned());
        args
    }
}
