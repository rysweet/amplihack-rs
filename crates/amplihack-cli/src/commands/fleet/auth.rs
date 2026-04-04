use super::*;

pub(super) struct AuthPropagator {
    pub(super) azlin_path: PathBuf,
}

impl AuthPropagator {
    pub(super) fn new(azlin_path: PathBuf) -> Self {
        Self { azlin_path }
    }

    pub(super) fn propagate_all(&self, vm_name: &str, services: &[String]) -> Vec<AuthResult> {
        let target_services = if services.is_empty() {
            vec![
                "github".to_string(),
                "azure".to_string(),
                "claude".to_string(),
            ]
        } else {
            services.to_vec()
        };

        target_services
            .into_iter()
            .map(|service| {
                if auth_files_for_service(&service).is_none() {
                    return AuthResult {
                        service: service.clone(),
                        vm_name: vm_name.to_string(),
                        success: false,
                        files_copied: Vec::new(),
                        error: Some(format!("Unknown service: {}", service)),
                        duration_seconds: 0.0,
                    };
                }

                self.propagate_service(vm_name, &service)
            })
            .collect()
    }

    pub(super) fn verify_auth(&self, vm_name: &str) -> Vec<(String, bool)> {
        let checks = [
            ("github", "gh auth status"),
            ("azure", "az account show --query name -o tsv"),
        ];

        checks
            .into_iter()
            .map(|(service, command)| {
                let works = self
                    .remote_exec(vm_name, command)
                    .map(|output| {
                        if output.status.success() {
                            true
                        } else {
                            let detail = String::from_utf8_lossy(&output.stderr);
                            let stdout = String::from_utf8_lossy(&output.stdout);
                            let raw = if detail.trim().is_empty() {
                                stdout.trim().to_string()
                            } else {
                                detail.trim().to_string()
                            };
                            if !raw.is_empty() {
                                let sanitized = sanitize_external_error_detail(&raw, 200);
                                tracing::warn!(
                                    "Auth verify failed for {service} on {vm_name}: {sanitized}"
                                );
                            }
                            false
                        }
                    })
                    .unwrap_or(false);
                (service.to_string(), works)
            })
            .collect()
    }

    pub(super) fn propagate_service(&self, vm_name: &str, service: &str) -> AuthResult {
        let start = std::time::Instant::now();
        let mut files_copied = Vec::new();
        let mut errors = Vec::new();
        let Some(files) = auth_files_for_service(service) else {
            return AuthResult {
                service: service.to_string(),
                vm_name: vm_name.to_string(),
                success: false,
                files_copied,
                error: Some(format!("Unknown service: {}", service)),
                duration_seconds: 0.0,
            };
        };

        let mut dest_dirs = Vec::<String>::new();
        for (_, dest, _) in files {
            let parent = remote_parent_dir(dest);
            if !dest_dirs.iter().any(|existing| existing == &parent) {
                dest_dirs.push(parent);
            }
        }
        for dest_dir in dest_dirs {
            let command = format!("mkdir -p {}", shell_single_quote(&dest_dir));
            let _ = self.remote_exec(vm_name, &command);
        }

        for (src, dest, mode) in files {
            let src_path = expand_tilde(src);
            if !src_path.exists() {
                continue;
            }

            let mut cmd = Command::new(&self.azlin_path);
            cmd.args([
                "cp",
                &src_path.to_string_lossy(),
                &format!("{vm_name}:{dest}"),
            ]);
            match run_output_with_timeout(cmd, Duration::from_secs(60)) {
                Ok(output) if output.status.success() => {
                    if validate_chmod_mode(mode).is_ok() {
                        let chmod = format!("chmod {mode} {}", shell_single_quote(dest));
                        let _ = self.remote_exec(vm_name, &chmod);
                    }
                    if let Some(name) = src_path.file_name().and_then(|name| name.to_str()) {
                        files_copied.push(name.to_string());
                    }
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let file_name = src_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("file");
                    let detail = sanitize_external_error_detail(stderr.trim(), 200);
                    errors.push(format!("Failed to copy {file_name}: {detail}"));
                }
                Err(error) => {
                    let file_name = src_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("file");
                    let message = error.to_string();
                    if message.contains("timed out after") {
                        errors.push(format!("Timeout copying {file_name}"));
                    } else {
                        let detail = sanitize_external_error_detail(&message, 200);
                        errors.push(format!("Error copying {file_name}: {detail}"));
                    }
                }
            }
        }

        AuthResult {
            service: service.to_string(),
            vm_name: vm_name.to_string(),
            success: errors.is_empty(),
            files_copied,
            error: (!errors.is_empty()).then(|| errors.join("; ")),
            duration_seconds: start.elapsed().as_secs_f64(),
        }
    }

    pub(super) fn remote_exec(&self, vm_name: &str, command: &str) -> Result<Output> {
        validate_vm_name(vm_name)?;
        let mut cmd = Command::new(&self.azlin_path);
        cmd.args(["connect", vm_name, "--no-tmux", "--", command]);
        run_output_with_timeout(cmd, Duration::from_secs(30))
    }
}
