//! Remote VM health checking.
//!
//! Matches Python `amplihack/fleet/fleet_health.py`:
//! - SSH reachability probe
//! - System resource monitoring (memory, disk, load)
//! - Agent process detection
//! - Health status aggregation

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Instant;
use tracing::{debug, warn};

/// Health check result for a single VM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub vm_name: String,
    pub ssh_reachable: bool,
    pub latency_ms: Option<u64>,
    pub memory: Option<ResourceInfo>,
    pub disk: Option<ResourceInfo>,
    pub load_average: Option<f64>,
    pub agent_processes: usize,
    pub overall: HealthStatus,
}

/// Overall health status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unreachable,
}

/// Resource usage info (memory or disk).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceInfo {
    pub total_mb: u64,
    pub used_mb: u64,
    pub percent_used: f64,
}

impl ResourceInfo {
    /// Whether usage is above the warning threshold.
    pub fn is_warning(&self) -> bool {
        self.percent_used > 80.0
    }

    /// Whether usage is critical.
    pub fn is_critical(&self) -> bool {
        self.percent_used > 95.0
    }
}

/// Check SSH reachability of a host.
pub fn check_ssh_reachable(ssh_target: &str) -> (bool, Option<u64>) {
    let start = Instant::now();
    let result = Command::new("ssh")
        .args([
            "-o", "ConnectTimeout=5",
            "-o", "StrictHostKeyChecking=no",
            "-o", "BatchMode=yes",
            ssh_target,
            "echo", "ok",
        ])
        .output();

    match result {
        Ok(output) if output.status.success() => {
            let ms = start.elapsed().as_millis() as u64;
            debug!(target = ssh_target, latency_ms = ms, "SSH reachable");
            (true, Some(ms))
        }
        _ => {
            warn!(target = ssh_target, "SSH unreachable");
            (false, None)
        }
    }
}

/// Run a remote health check via SSH.
pub fn check_vm_health(vm_name: &str, ssh_target: &str) -> HealthReport {
    let (reachable, latency) = check_ssh_reachable(ssh_target);

    if !reachable {
        return HealthReport {
            vm_name: vm_name.into(),
            ssh_reachable: false,
            latency_ms: None,
            memory: None,
            disk: None,
            load_average: None,
            agent_processes: 0,
            overall: HealthStatus::Unreachable,
        };
    }

    let memory = check_remote_memory(ssh_target);
    let disk = check_remote_disk(ssh_target);
    let load = check_remote_load(ssh_target);
    let agents = count_agent_processes(ssh_target);

    let overall = compute_overall_status(&memory, &disk, load);

    HealthReport {
        vm_name: vm_name.into(),
        ssh_reachable: true,
        latency_ms: latency,
        memory,
        disk,
        load_average: load,
        agent_processes: agents,
        overall,
    }
}

/// Parse memory info from `free -m` output.
pub fn parse_free_output(output: &str) -> Option<ResourceInfo> {
    for line in output.lines() {
        if line.starts_with("Mem:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                let total: u64 = parts[1].parse().ok()?;
                let used: u64 = parts[2].parse().ok()?;
                let percent = if total > 0 {
                    (used as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                return Some(ResourceInfo {
                    total_mb: total,
                    used_mb: used,
                    percent_used: percent,
                });
            }
        }
    }
    None
}

/// Parse disk info from `df -m /` output.
pub fn parse_df_output(output: &str) -> Option<ResourceInfo> {
    for line in output.lines().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 4 {
            let total: u64 = parts[1].parse().ok()?;
            let used: u64 = parts[2].parse().ok()?;
            let percent = if total > 0 {
                (used as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            return Some(ResourceInfo {
                total_mb: total,
                used_mb: used,
                percent_used: percent,
            });
        }
    }
    None
}

fn check_remote_memory(ssh_target: &str) -> Option<ResourceInfo> {
    run_ssh_command(ssh_target, "free -m")
        .ok()
        .and_then(|out| parse_free_output(&out))
}

fn check_remote_disk(ssh_target: &str) -> Option<ResourceInfo> {
    run_ssh_command(ssh_target, "df -m /")
        .ok()
        .and_then(|out| parse_df_output(&out))
}

fn check_remote_load(ssh_target: &str) -> Option<f64> {
    run_ssh_command(ssh_target, "cat /proc/loadavg")
        .ok()
        .and_then(|out| out.split_whitespace().next()?.parse().ok())
}

fn count_agent_processes(ssh_target: &str) -> usize {
    run_ssh_command(ssh_target, "pgrep -c -f 'claude|copilot|codex|amplifier'")
        .ok()
        .and_then(|out| out.trim().parse().ok())
        .unwrap_or(0)
}

fn run_ssh_command(ssh_target: &str, cmd: &str) -> Result<String> {
    let output = Command::new("ssh")
        .args([
            "-o", "ConnectTimeout=5",
            "-o", "StrictHostKeyChecking=no",
            "-o", "BatchMode=yes",
            ssh_target,
            cmd,
        ])
        .output()
        .context("failed to execute SSH command")?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn compute_overall_status(
    memory: &Option<ResourceInfo>,
    disk: &Option<ResourceInfo>,
    load: Option<f64>,
) -> HealthStatus {
    if memory.as_ref().is_some_and(|m| m.is_critical())
        || disk.as_ref().is_some_and(|d| d.is_critical())
    {
        return HealthStatus::Unhealthy;
    }
    if memory.as_ref().is_some_and(|m| m.is_warning())
        || disk.as_ref().is_some_and(|d| d.is_warning())
        || load.is_some_and(|l| l > 4.0)
    {
        return HealthStatus::Degraded;
    }
    HealthStatus::Healthy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_free_output_valid() {
        let output = "\
              total        used        free      shared  buff/cache   available\n\
Mem:          16384        8192        4096         128        4096       12288\n\
Swap:          2048           0        2048";
        let info = parse_free_output(output).unwrap();
        assert_eq!(info.total_mb, 16384);
        assert_eq!(info.used_mb, 8192);
        assert!((info.percent_used - 50.0).abs() < 0.1);
    }

    #[test]
    fn parse_df_output_valid() {
        let output = "\
Filesystem     1M-blocks  Used Available Use% Mounted on\n\
/dev/sda1         102400 51200     51200  50% /";
        let info = parse_df_output(output).unwrap();
        assert_eq!(info.total_mb, 102400);
        assert_eq!(info.used_mb, 51200);
    }

    #[test]
    fn resource_info_thresholds() {
        let ok = ResourceInfo {
            total_mb: 1000,
            used_mb: 500,
            percent_used: 50.0,
        };
        assert!(!ok.is_warning());
        assert!(!ok.is_critical());

        let warn = ResourceInfo {
            total_mb: 1000,
            used_mb: 850,
            percent_used: 85.0,
        };
        assert!(warn.is_warning());
        assert!(!warn.is_critical());

        let crit = ResourceInfo {
            total_mb: 1000,
            used_mb: 960,
            percent_used: 96.0,
        };
        assert!(crit.is_critical());
    }

    #[test]
    fn overall_status_computation() {
        assert_eq!(
            compute_overall_status(&None, &None, None),
            HealthStatus::Healthy
        );
        let high_mem = Some(ResourceInfo {
            total_mb: 1000,
            used_mb: 850,
            percent_used: 85.0,
        });
        assert_eq!(
            compute_overall_status(&high_mem, &None, None),
            HealthStatus::Degraded
        );
        let crit_disk = Some(ResourceInfo {
            total_mb: 1000,
            used_mb: 960,
            percent_used: 96.0,
        });
        assert_eq!(
            compute_overall_status(&None, &crit_disk, None),
            HealthStatus::Unhealthy
        );
        assert_eq!(
            compute_overall_status(&None, &None, Some(5.0)),
            HealthStatus::Degraded
        );
    }

    #[test]
    fn health_report_serializes() {
        let report = HealthReport {
            vm_name: "vm-1".into(),
            ssh_reachable: true,
            latency_ms: Some(42),
            memory: None,
            disk: None,
            load_average: Some(1.5),
            agent_processes: 3,
            overall: HealthStatus::Healthy,
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["vm_name"], "vm-1");
        assert_eq!(json["overall"], "healthy");
    }
}
