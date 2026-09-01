//! DockerManager — image building and container execution.

use anyhow::{Context, Result};
use std::env;
use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::{
    DEFAULT_IMAGE_NAME, DockerDetector, LITELLM_ROUTING_LABEL, LITELLM_ROUTING_REVISION,
    VERSION_LABEL, docker_setup_command,
    helpers::{forwarded_env_vars, is_secret_env_key},
};
use crate::util::{run_with_timeout, run_with_timeout_described};

const DOCKER_BUILD_TIMEOUT: Duration = Duration::from_secs(600);
const DOCKER_RUN_TIMEOUT: Duration = Duration::from_secs(3600);

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
        validate_docker_gateway_env()?;
        if !self.detector.is_running() {
            eprintln!("Docker is not running.");
            return Ok(1);
        }

        if !self.build_image()? {
            eprintln!("Failed to build Docker image.");
            return Ok(1);
        }
        if amplihack_utils::litellm_proxy::proxy_requested()
            && !self.detector.image_supports_litellm(self.image_name)
        {
            anyhow::bail!(
                "the Docker image does not declare LiteLLM routing support for amplihack {}; provide an image built from this amplihack version with the required routing labels, or launch the agent outside Docker",
                crate::VERSION
            );
        }

        let run_args = self.build_run_args(cwd, amplihack_args, env::vars_os());
        let mut command = Command::new("docker");
        command.args(&run_args);
        amplihack_utils::litellm_proxy::scrub_proxy_environment(&mut command);
        command.envs(
            forwarded_env_vars(env::vars_os())
                .into_iter()
                .filter(|(key, _)| is_secret_env_key(key)),
        );
        let status = run_with_timeout_described(command, DOCKER_RUN_TIMEOUT, "docker run")
            .context("failed to execute docker run")?;
        Ok(status.code().unwrap_or(1))
    }

    fn build_image(&self) -> Result<bool> {
        let image_exists = self.detector.check_image_exists(self.image_name);
        let routing_requested = amplihack_utils::litellm_proxy::proxy_requested();
        let routing_compatible = image_exists
            && routing_requested
            && self.detector.image_supports_litellm(self.image_name);
        if existing_image_is_usable(image_exists, routing_requested, routing_compatible) {
            return Ok(true);
        }
        if image_exists {
            println!(
                "Rebuilding incompatible Docker image for amplihack {}: {}",
                crate::VERSION,
                self.image_name
            );
        }

        let dockerfile = self.project_root.join("Dockerfile");
        let upgrade_context;
        let build_args = if dockerfile.is_file() {
            self.build_image_args(&dockerfile)
        } else if image_exists {
            upgrade_context = self.prepare_upgrade_context()?;
            self.build_image_args(&upgrade_context.path().join("Dockerfile"))
        } else {
            eprintln!("Dockerfile not found at {}", dockerfile.display());
            return Ok(false);
        };

        println!("Building Docker image: {}", self.image_name);
        let mut command = docker_setup_command();
        command.args(build_args);
        let status = run_with_timeout(command, DOCKER_BUILD_TIMEOUT)
            .context("failed to execute docker build")?;
        if !status.success() {
            eprintln!("Docker build failed.");
            return Ok(false);
        }

        println!("Successfully built Docker image: {}", self.image_name);
        Ok(true)
    }

    fn prepare_upgrade_context(&self) -> Result<tempfile::TempDir> {
        let context = tempfile::tempdir().context("failed to create Docker upgrade context")?;
        let executable =
            env::current_exe().context("failed to locate the running amplihack executable")?;
        fs::copy(&executable, context.path().join("amplihack")).with_context(|| {
            format!(
                "failed to stage {} for Docker image upgrade",
                executable.display()
            )
        })?;
        fs::write(
            context.path().join("Dockerfile"),
            format!(
                "FROM {}\nCOPY amplihack /usr/local/bin/amplihack\n",
                self.image_name
            ),
        )
        .context("failed to write Docker upgrade definition")?;
        Ok(context)
    }

    pub(crate) fn build_image_args(&self, dockerfile: &Path) -> Vec<String> {
        let build_context = dockerfile.parent().unwrap_or(&self.project_root);
        vec![
            "build".to_string(),
            "-t".to_string(),
            self.image_name.to_string(),
            "--label".to_string(),
            format!("{LITELLM_ROUTING_LABEL}={LITELLM_ROUTING_REVISION}"),
            "--label".to_string(),
            format!("{VERSION_LABEL}={}", crate::VERSION),
            "-f".to_string(),
            dockerfile.display().to_string(),
            build_context.display().to_string(),
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
            let assignment = if is_secret_env_key(&key) {
                key
            } else {
                format!("{key}={value}")
            };
            args.extend(["-e".to_string(), assignment]);
        }

        args.push(self.image_name.to_string());
        args.extend(amplihack_args.iter().cloned());
        args
    }
}

pub(super) fn existing_image_is_usable(
    image_exists: bool,
    routing_requested: bool,
    routing_compatible: bool,
) -> bool {
    image_exists && (!routing_requested || routing_compatible)
}

pub(super) fn validate_docker_gateway_env() -> Result<()> {
    if env::var(amplihack_utils::litellm_proxy::ENDPOINT_ENV)
        .is_ok_and(|endpoint| amplihack_utils::litellm_proxy::endpoint_is_loopback(&endpoint))
    {
        anyhow::bail!(
            "the Docker launcher cannot safely reach a host-loopback LiteLLM gateway without exposing all host services; use a reachable HTTPS gateway or launch the agent outside Docker"
        );
    }
    Ok(())
}
