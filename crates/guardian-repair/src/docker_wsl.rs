use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use chrono::Local;
use guardian_core::{GuardianError, types::ActionPlan};
use guardian_observers::docker_wsl::analyze_wslconfig;
use guardian_windows::{
    paths::{guardian_backup_dir, wslconfig_path},
    process::run_command_with_cmd_fallback,
};

const DEFAULT_MEMORY_GB: u64 = 8;
const DEFAULT_PROCESSORS: u64 = 6;
const DEFAULT_SWAP_GB: u64 = 4;
const DEFAULT_AUTO_MEMORY_RECLAIM: &str = "gradual";
const RUNTIME_RECOVERY_TIMEOUT: Duration = Duration::from_secs(180);
const RUNTIME_RECOVERY_POLL_INTERVAL: Duration = Duration::from_secs(2);
const RUNTIME_RECOVERY_REQUIRED_HEALTHY_POLLS: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DockerRepairOutcome {
    Noop,
    ManagedConfigWritten,
    RuntimeRecovered,
    RestartBlocked,
    PartiallyApplied,
    Unresolved,
}

impl DockerRepairOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Noop => "noop",
            Self::ManagedConfigWritten => "managed_config_written",
            Self::RuntimeRecovered => "runtime_recovered",
            Self::RestartBlocked => "restart_blocked",
            Self::PartiallyApplied => "partially_applied",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DockerRepairExecution {
    pub wslconfig_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub created_file: bool,
    pub restart_required: bool,
    pub runtime_anomaly_detected: bool,
    pub runtime_anomaly_after: bool,
    pub runtime_repair_attempted: bool,
    pub runtime_repair_blocked: bool,
    pub runtime_action: Option<String>,
    pub runtime_block_reason: Option<String>,
    pub runtime_error: Option<String>,
    pub runtime_wait_poll_count: u32,
    pub runtime_wait_stable_success_polls: u32,
    pub runtime_wait_timed_out: bool,
    pub runtime_wait_elapsed_ms: u64,
    pub running_containers_before: Option<u64>,
    pub docker_desktop_status_before: Option<String>,
    pub docker_desktop_status_after: Option<String>,
    pub missing_keys_before: Vec<String>,
    pub missing_keys_after: Vec<String>,
    pub baseline: WslBaseline,
    pub outcome: DockerRepairOutcome,
}

impl DockerRepairExecution {
    pub fn is_successful(&self) -> bool {
        self.outcome != DockerRepairOutcome::Unresolved
    }

    pub fn outcome_summary(&self) -> String {
        match self.outcome {
            DockerRepairOutcome::Noop => {
                "Docker / WSL repair confirm found no missing `.wslconfig` baseline keys and no runtime recovery work was required."
                    .to_string()
            }
            DockerRepairOutcome::ManagedConfigWritten => {
                if self.runtime_repair_attempted && !self.runtime_anomaly_after {
                    "Docker / WSL repair confirm wrote the managed `.wslconfig` baseline and completed the safe runtime restart chain."
                        .to_string()
                } else {
                    "Docker / WSL repair confirm wrote the managed `.wslconfig` baseline and re-check reached zero missing keys."
                        .to_string()
                }
            }
            DockerRepairOutcome::RuntimeRecovered => {
                "Docker / WSL repair confirm completed the safe runtime restart chain and health re-check is clean."
                    .to_string()
            }
            DockerRepairOutcome::RestartBlocked => {
                "Docker / WSL repair confirm respected guardrails and skipped runtime restart because the current machine could not prove it was safe."
                    .to_string()
            }
            DockerRepairOutcome::PartiallyApplied => {
                "Docker / WSL repair confirm applied the safe subset of recovery actions, but runtime recovery classes still remain after re-check."
                    .to_string()
            }
            DockerRepairOutcome::Unresolved => {
                "Docker / WSL repair confirm could not safely resolve the requested runtime/config state."
                    .to_string()
            }
        }
    }

    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();

        if !self.missing_keys_before.is_empty() {
            notes.push(format!(
                "Managed WSL baseline: memory={}, processors={}, swap={}, autoMemoryReclaim={}",
                self.baseline.memory_value(),
                self.baseline.processors,
                self.baseline.swap_value(),
                self.baseline.auto_memory_reclaim
            ));
        }

        if let Some(backup_path) = &self.backup_path {
            notes.push(format!(
                "`.wslconfig` backup created at {}",
                backup_path.display()
            ));
        } else if self.created_file {
            notes.push(
                "`.wslconfig` did not exist before repair; Guardian created a new managed file."
                    .to_string(),
            );
        }

        if let Some(running_containers) = self.running_containers_before {
            notes.push(format!(
                "Running containers before runtime recovery decision: {running_containers}"
            ));
        }

        if let Some(status) = &self.docker_desktop_status_before {
            notes.push(format!(
                "Docker Desktop status before recovery decision: {status}"
            ));
        }

        if let Some(action) = &self.runtime_action {
            notes.push(format!(
                "Runtime recovery action executed: `wsl --shutdown` + `docker desktop {action}`."
            ));
        }

        if self.runtime_repair_blocked
            && let Some(reason) = &self.runtime_block_reason
        {
            notes.push(format!(
                "Runtime recovery was blocked by guardrails: {reason}"
            ));
        }

        if let Some(error) = &self.runtime_error {
            notes.push(format!("Runtime recovery command error: {error}"));
        }

        if self.runtime_repair_attempted {
            notes.push(format!(
                "Runtime recovery stability wait: {} poll(s), {} consecutive healthy poll(s), timed_out={}.",
                self.runtime_wait_poll_count,
                self.runtime_wait_stable_success_polls,
                self.runtime_wait_timed_out
            ));
        }

        if let Some(status) = &self.docker_desktop_status_after {
            notes.push(format!(
                "Docker Desktop status after recovery decision: {status}"
            ));
        }

        if self.restart_required {
            notes.push(
                "WSL baseline changes will apply after the next `wsl --shutdown` or full WSL restart."
                    .to_string(),
            );
        }

        if self.runtime_repair_attempted && !self.runtime_anomaly_after {
            notes.push(
                "Runtime recovery re-check completed cleanly after the guarded WSL and Docker Desktop restart chain."
                    .to_string(),
            );
        } else if self.runtime_anomaly_detected && self.runtime_anomaly_after {
            notes.push(
                "Runtime recovery classes are still present after the confirm flow completed."
                    .to_string(),
            );
        }

        notes
    }
}

#[derive(Debug, Clone)]
pub struct WslBaseline {
    pub memory_gb: u64,
    pub processors: u64,
    pub swap_gb: u64,
    pub auto_memory_reclaim: String,
}

impl WslBaseline {
    pub fn memory_value(&self) -> String {
        format!("{}GB", self.memory_gb)
    }

    pub fn swap_value(&self) -> String {
        format!("{}GB", self.swap_gb)
    }

    fn render_new_file(&self) -> String {
        normalize_wslconfig_lines(&[
            "# guardian-managed: WSL baseline generated for D3 recovery".to_string(),
            "[wsl2]".to_string(),
            format!("memory={}", self.memory_value()),
            format!("processors={}", self.processors),
            format!("swap={}", self.swap_value()),
            String::new(),
            "[experimental]".to_string(),
            format!("autoMemoryReclaim={}", self.auto_memory_reclaim),
        ])
    }
}

pub fn planned_actions() -> Vec<ActionPlan> {
    vec![
        ActionPlan::new(
            "guardian repair docker --dry-run".to_string(),
            "Preview the Docker / WSL recovery chain, including managed `.wslconfig` baseline repair and guarded runtime restart."
                .to_string(),
            false,
        ),
        ActionPlan::new(
            "guardian repair docker --confirm".to_string(),
            "Execute the low-risk Docker / WSL repair chain with backup, managed `.wslconfig` write, guarded runtime restart, and post-check."
                .to_string(),
            true,
        ),
    ]
}

pub fn execute_confirmed() -> Result<DockerRepairExecution, GuardianError> {
    let wslconfig_path = wslconfig_path().map_err(GuardianError::Io)?;
    let created_file = !wslconfig_path.exists();
    let original_contents = if created_file {
        String::new()
    } else {
        fs::read_to_string(&wslconfig_path)?
    };

    if !created_file {
        validate_wslconfig_text(&original_contents)?;
    }

    let baseline = recommended_baseline();
    let missing_keys_before = missing_keys(&original_contents);
    let runtime_before = collect_runtime_snapshot();
    let runtime_anomaly_detected = runtime_before.anomaly_detected;

    if missing_keys_before.is_empty() && !runtime_anomaly_detected {
        return Ok(DockerRepairExecution {
            wslconfig_path,
            backup_path: None,
            created_file: false,
            restart_required: false,
            runtime_anomaly_detected,
            runtime_anomaly_after: false,
            runtime_repair_attempted: false,
            runtime_repair_blocked: false,
            runtime_action: None,
            runtime_block_reason: None,
            runtime_error: None,
            runtime_wait_poll_count: 0,
            runtime_wait_stable_success_polls: 0,
            runtime_wait_timed_out: false,
            runtime_wait_elapsed_ms: 0,
            running_containers_before: runtime_before.running_containers,
            docker_desktop_status_before: runtime_before.docker_desktop_status.clone(),
            docker_desktop_status_after: runtime_before.docker_desktop_status,
            missing_keys_before,
            missing_keys_after: Vec::new(),
            baseline,
            outcome: DockerRepairOutcome::Noop,
        });
    }

    let (backup_path, missing_keys_after) = if missing_keys_before.is_empty() {
        (None, Vec::new())
    } else {
        let backup_path = if created_file {
            None
        } else {
            Some(backup_wslconfig(&wslconfig_path)?)
        };
        let merged_contents = merge_managed_wslconfig(&original_contents, &baseline)?;
        write_wslconfig_atomic(&wslconfig_path, &merged_contents)?;
        let refreshed_contents = fs::read_to_string(&wslconfig_path)?;
        let missing_keys_after = missing_keys(&refreshed_contents);
        (backup_path, missing_keys_after)
    };

    let runtime_attempt = if runtime_anomaly_detected {
        attempt_runtime_recovery(&runtime_before)
    } else {
        RuntimeRecoveryAttempt::not_needed(runtime_before.clone())
    };
    let runtime_after = runtime_attempt.wait.snapshot_after.clone();
    let runtime_anomaly_after = runtime_attempt.wait.timed_out || runtime_after.anomaly_detected;
    let config_recovered = missing_keys_after.is_empty();
    let config_changed = !missing_keys_before.is_empty();

    let outcome = if runtime_attempt.blocked {
        DockerRepairOutcome::RestartBlocked
    } else if runtime_attempt.error.is_some() {
        if config_recovered {
            DockerRepairOutcome::PartiallyApplied
        } else {
            DockerRepairOutcome::Unresolved
        }
    } else if !runtime_anomaly_detected {
        DockerRepairOutcome::ManagedConfigWritten
    } else if !runtime_anomaly_after && !config_changed {
        DockerRepairOutcome::RuntimeRecovered
    } else if !runtime_anomaly_after && config_recovered {
        DockerRepairOutcome::ManagedConfigWritten
    } else if config_recovered {
        DockerRepairOutcome::PartiallyApplied
    } else {
        DockerRepairOutcome::Unresolved
    };

    Ok(DockerRepairExecution {
        wslconfig_path,
        backup_path,
        created_file,
        restart_required: config_changed && runtime_anomaly_after,
        runtime_anomaly_detected,
        runtime_anomaly_after,
        runtime_repair_attempted: runtime_attempt.attempted,
        runtime_repair_blocked: runtime_attempt.blocked,
        runtime_action: runtime_attempt.action,
        runtime_block_reason: runtime_attempt.block_reason,
        runtime_error: runtime_attempt.error,
        runtime_wait_poll_count: runtime_attempt.wait.poll_count,
        runtime_wait_stable_success_polls: runtime_attempt.wait.stable_success_polls,
        runtime_wait_timed_out: runtime_attempt.wait.timed_out,
        runtime_wait_elapsed_ms: runtime_attempt.wait.elapsed_ms,
        running_containers_before: runtime_before.running_containers,
        docker_desktop_status_before: runtime_before.docker_desktop_status,
        docker_desktop_status_after: runtime_after.docker_desktop_status,
        missing_keys_before,
        missing_keys_after,
        baseline,
        outcome,
    })
}

fn recommended_baseline() -> WslBaseline {
    let host_total_memory_gb = query_total_physical_memory_gb().unwrap_or(16);
    let logical_processors = env::var("NUMBER_OF_PROCESSORS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(20);

    let memory_gb = (host_total_memory_gb / 2).clamp(4, DEFAULT_MEMORY_GB);
    let processors = (logical_processors / 3).clamp(4, DEFAULT_PROCESSORS);
    let swap_gb = (memory_gb / 2).clamp(2, DEFAULT_SWAP_GB);

    WslBaseline {
        memory_gb,
        processors,
        swap_gb,
        auto_memory_reclaim: DEFAULT_AUTO_MEMORY_RECLAIM.to_string(),
    }
}

fn query_total_physical_memory_gb() -> Option<u64> {
    let args = vec![
        OsString::from("-NoProfile"),
        OsString::from("-Command"),
        OsString::from("(Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory"),
    ];
    let output = run_command_with_cmd_fallback("powershell", &args).ok()?;
    if !output.success() {
        return None;
    }

    let bytes = output.stdout.trim().parse::<u64>().ok()?;
    let gib = 1024_u64 * 1024 * 1024;
    Some(((bytes as f64) / (gib as f64)).round() as u64)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeSnapshot {
    anomaly_detected: bool,
    docker_desktop_cli_available: bool,
    docker_desktop_status: Option<String>,
    running_containers: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeRecoveryPlan {
    Start,
    Restart,
    Blocked(String),
}

#[derive(Debug, Clone)]
struct RuntimeRecoveryAttempt {
    attempted: bool,
    blocked: bool,
    action: Option<String>,
    block_reason: Option<String>,
    error: Option<String>,
    wait: RuntimeRecoveryWait,
}

impl RuntimeRecoveryAttempt {
    fn not_needed(snapshot_after: RuntimeSnapshot) -> Self {
        Self {
            attempted: false,
            blocked: false,
            action: None,
            block_reason: None,
            error: None,
            wait: RuntimeRecoveryWait::immediate(snapshot_after),
        }
    }
}

#[derive(Debug, Clone)]
struct RuntimeRecoveryWait {
    snapshot_after: RuntimeSnapshot,
    poll_count: u32,
    stable_success_polls: u32,
    timed_out: bool,
    elapsed_ms: u64,
}

impl RuntimeRecoveryWait {
    fn immediate(snapshot_after: RuntimeSnapshot) -> Self {
        Self {
            snapshot_after,
            poll_count: 0,
            stable_success_polls: 0,
            timed_out: false,
            elapsed_ms: 0,
        }
    }
}

fn collect_runtime_snapshot() -> RuntimeSnapshot {
    let docker_version_ok = command_succeeds("docker", ["version", "--format", "{{json .}}"]);
    let docker_info = command_output("docker", ["info", "--format", "{{ .ContainersRunning }}"]);
    let docker_info_ok = docker_info.is_ok();
    let running_containers = docker_info
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok());
    let wsl_list = command_output("wsl", ["-l", "-v"]);
    let wsl_ok = wsl_list.is_ok();
    let wsl_has_docker_desktop = wsl_list
        .as_deref()
        .map(|output| output.contains("docker-desktop"))
        .unwrap_or(false);
    let docker_desktop_cli_available = command_succeeds("docker", ["desktop", "version"]);
    let docker_desktop_status = docker_desktop_status();

    RuntimeSnapshot {
        anomaly_detected: !(docker_version_ok
            && docker_info_ok
            && wsl_ok
            && wsl_has_docker_desktop),
        docker_desktop_cli_available,
        docker_desktop_status,
        running_containers,
    }
}

fn decide_runtime_recovery_plan(snapshot: &RuntimeSnapshot) -> RuntimeRecoveryPlan {
    if let Some(running_containers) = snapshot.running_containers
        && running_containers > 0
    {
        return RuntimeRecoveryPlan::Blocked(format!(
            "Docker reports {running_containers} running container(s); Guardian will not restart WSL or Docker Desktop while workload may still be active."
        ));
    }

    if !snapshot.docker_desktop_cli_available {
        return RuntimeRecoveryPlan::Blocked(
            "The `docker desktop` CLI plugin is unavailable, so Guardian cannot safely orchestrate the restart chain on this machine."
                .to_string(),
        );
    }

    if snapshot.running_containers.is_none()
        && snapshot.docker_desktop_status.as_deref() != Some("stopped")
    {
        return RuntimeRecoveryPlan::Blocked(
            "Guardian could not prove that the running-container count is zero, and Docker Desktop is not known to be stopped."
                .to_string(),
        );
    }

    match snapshot.docker_desktop_status.as_deref() {
        Some("stopped") => RuntimeRecoveryPlan::Start,
        _ => RuntimeRecoveryPlan::Restart,
    }
}

fn attempt_runtime_recovery(snapshot_before: &RuntimeSnapshot) -> RuntimeRecoveryAttempt {
    match decide_runtime_recovery_plan(snapshot_before) {
        RuntimeRecoveryPlan::Blocked(reason) => RuntimeRecoveryAttempt {
            attempted: false,
            blocked: true,
            action: None,
            block_reason: Some(reason),
            error: None,
            wait: RuntimeRecoveryWait::immediate(snapshot_before.clone()),
        },
        RuntimeRecoveryPlan::Start => execute_runtime_recovery("start"),
        RuntimeRecoveryPlan::Restart => execute_runtime_recovery("restart"),
    }
}

fn execute_runtime_recovery(docker_action: &str) -> RuntimeRecoveryAttempt {
    let mut command_runner =
        |program: &str, args: &[&str]| command_output(program, args.iter().copied());
    let mut collect_snapshot = collect_runtime_snapshot;
    let max_polls =
        runtime_recovery_max_polls(RUNTIME_RECOVERY_TIMEOUT, RUNTIME_RECOVERY_POLL_INTERVAL);

    execute_runtime_recovery_with(
        docker_action,
        &mut command_runner,
        &mut collect_snapshot,
        max_polls,
        RUNTIME_RECOVERY_POLL_INTERVAL,
    )
}

fn execute_runtime_recovery_with<C, S>(
    docker_action: &str,
    run_command: &mut C,
    collect_snapshot: &mut S,
    max_polls: u32,
    poll_interval: Duration,
) -> RuntimeRecoveryAttempt
where
    C: FnMut(&str, &[&str]) -> Result<String, GuardianError>,
    S: FnMut() -> RuntimeSnapshot,
{
    if let Err(error) = run_command("wsl", &["--shutdown"]) {
        return RuntimeRecoveryAttempt {
            attempted: true,
            blocked: false,
            action: Some(docker_action.to_string()),
            block_reason: None,
            error: Some(format!("`wsl --shutdown` failed: {error}")),
            wait: RuntimeRecoveryWait::immediate(collect_snapshot()),
        };
    }

    if let Err(error) = run_command("docker", &["desktop", docker_action, "--timeout", "180"]) {
        return RuntimeRecoveryAttempt {
            attempted: true,
            blocked: false,
            action: Some(docker_action.to_string()),
            block_reason: None,
            error: Some(format!("`docker desktop {docker_action}` failed: {error}")),
            wait: RuntimeRecoveryWait::immediate(collect_snapshot()),
        };
    }

    RuntimeRecoveryAttempt {
        attempted: true,
        blocked: false,
        action: Some(docker_action.to_string()),
        block_reason: None,
        error: None,
        wait: wait_for_runtime_health_with(collect_snapshot, max_polls, poll_interval),
    }
}

fn wait_for_runtime_health_with<F>(
    mut collect_snapshot: F,
    max_polls: u32,
    poll_interval: Duration,
) -> RuntimeRecoveryWait
where
    F: FnMut() -> RuntimeSnapshot,
{
    let started_at = Instant::now();
    let mut poll_count = 0_u32;
    let mut stable_success_polls = 0_u32;
    let mut latest = collect_snapshot();
    poll_count += 1;
    stable_success_polls = next_stable_success_polls(stable_success_polls, &latest);

    while poll_count < max_polls && stable_success_polls < RUNTIME_RECOVERY_REQUIRED_HEALTHY_POLLS {
        if !poll_interval.is_zero() {
            thread::sleep(poll_interval);
        }
        latest = collect_snapshot();
        poll_count += 1;
        stable_success_polls = next_stable_success_polls(stable_success_polls, &latest);
    }

    RuntimeRecoveryWait {
        snapshot_after: latest,
        poll_count,
        stable_success_polls,
        timed_out: stable_success_polls < RUNTIME_RECOVERY_REQUIRED_HEALTHY_POLLS,
        elapsed_ms: started_at.elapsed().as_millis().min(u64::MAX as u128) as u64,
    }
}

fn next_stable_success_polls(current: u32, snapshot: &RuntimeSnapshot) -> u32 {
    if snapshot.anomaly_detected {
        0
    } else {
        current.saturating_add(1)
    }
}

fn runtime_recovery_max_polls(timeout: Duration, poll_interval: Duration) -> u32 {
    if poll_interval.is_zero() {
        return RUNTIME_RECOVERY_REQUIRED_HEALTHY_POLLS.max(1);
    }

    let max_polls = timeout
        .as_millis()
        .checked_div(poll_interval.as_millis().max(1))
        .unwrap_or(0)
        .saturating_add(1)
        .min(u32::MAX as u128) as u32;

    max_polls.max(RUNTIME_RECOVERY_REQUIRED_HEALTHY_POLLS)
}

fn docker_desktop_status() -> Option<String> {
    let output = command_output("docker", ["desktop", "status"]).ok()?;
    output.lines().map(str::trim).find_map(|line| {
        if !line.starts_with("Status") {
            return None;
        }
        line.split_whitespace()
            .nth(1)
            .map(|value| value.to_ascii_lowercase())
    })
}

fn command_succeeds<I, S>(program: &str, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    command_output(program, args).is_ok()
}

fn command_output<I, S>(program: &str, args: I) -> Result<String, GuardianError>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let args_vec: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let output = run_command_with_cmd_fallback(program, &args_vec).map_err(GuardianError::Io)?;
    if output.success() {
        Ok(output.stdout)
    } else {
        Err(GuardianError::CommandFailed {
            command: format!(
                "{} {}",
                program,
                args_vec
                    .iter()
                    .map(|arg| arg.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            status: output.status,
            stderr: if output.stderr.is_empty() {
                output.stdout
            } else {
                output.stderr
            },
        })
    }
}

fn validate_wslconfig_text(contents: &str) -> Result<(), GuardianError> {
    for (index, raw_line) in contents.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if parse_section_name(line).is_some() {
            continue;
        }
        if !line.contains('=') {
            return Err(GuardianError::invalid_state(format!(
                "`.wslconfig` contains an unparsable line at {}: {}",
                index + 1,
                raw_line.trim()
            )));
        }
    }

    Ok(())
}

fn missing_keys(contents: &str) -> Vec<String> {
    let analysis = analyze_wslconfig(contents);
    let mut missing = Vec::new();

    if !analysis.wsl2_has_memory {
        missing.push("wsl2.memory".to_string());
    }
    if !analysis.wsl2_has_processors {
        missing.push("wsl2.processors".to_string());
    }
    if !analysis.wsl2_has_swap {
        missing.push("wsl2.swap".to_string());
    }
    if !analysis.experimental_has_auto_memory_reclaim {
        missing.push("experimental.autoMemoryReclaim".to_string());
    }

    missing
}

fn backup_wslconfig(path: &Path) -> Result<PathBuf, GuardianError> {
    let backup_dir = guardian_backup_dir()
        .map_err(GuardianError::Io)?
        .join("wslconfig");
    fs::create_dir_all(&backup_dir)?;

    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let backup_path = backup_dir.join(format!(".wslconfig.pre-guardian-managed-{timestamp}.bak"));
    fs::copy(path, &backup_path)?;
    Ok(backup_path)
}

fn write_wslconfig_atomic(path: &Path, contents: &str) -> Result<(), GuardianError> {
    let parent = path.parent().ok_or_else(|| {
        GuardianError::invalid_state("`.wslconfig` path does not have a parent directory")
    })?;
    fs::create_dir_all(parent)?;

    let temp_path = parent.join(format!(
        ".guardian-wslconfig-{}.tmp",
        Local::now().format("%Y%m%d-%H%M%S")
    ));
    fs::write(&temp_path, contents)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(&temp_path, path)?;
    Ok(())
}

fn merge_managed_wslconfig(
    contents: &str,
    baseline: &WslBaseline,
) -> Result<String, GuardianError> {
    if contents.trim().is_empty() {
        return Ok(baseline.render_new_file());
    }

    let lines: Vec<String> = contents
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    let mut current_section = String::new();
    let mut has_wsl2_section = false;
    let mut has_experimental_section = false;
    let mut missing_wsl2 = missing_keys_for_section(contents, "wsl2");
    let mut missing_experimental = missing_keys_for_section(contents, "experimental");
    let mut result = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if let Some(section_name) = parse_section_name(trimmed) {
            current_section = section_name;
            if current_section == "wsl2" {
                has_wsl2_section = true;
            }
            if current_section == "experimental" {
                has_experimental_section = true;
            }
        }

        result.push(line.clone());

        let next_is_section = lines
            .get(index + 1)
            .map(|next| parse_section_name(next.trim()).is_some())
            .unwrap_or(true);

        if next_is_section {
            match current_section.as_str() {
                "wsl2" if !missing_wsl2.is_empty() => {
                    append_managed_keys(&mut result, &mut missing_wsl2, baseline, "wsl2");
                }
                "experimental" if !missing_experimental.is_empty() => {
                    append_managed_keys(
                        &mut result,
                        &mut missing_experimental,
                        baseline,
                        "experimental",
                    );
                }
                _ => {}
            }
        }
    }

    if !has_wsl2_section && !missing_wsl2.is_empty() {
        append_new_section(&mut result, baseline, "wsl2", &missing_wsl2);
    }
    if !has_experimental_section && !missing_experimental.is_empty() {
        append_new_section(&mut result, baseline, "experimental", &missing_experimental);
    }

    Ok(normalize_wslconfig_lines(&result))
}

fn missing_keys_for_section(contents: &str, section: &str) -> Vec<String> {
    missing_keys(contents)
        .into_iter()
        .filter(|key| key.starts_with(section))
        .collect()
}

fn append_managed_keys(
    result: &mut Vec<String>,
    missing_keys: &mut Vec<String>,
    baseline: &WslBaseline,
    _section: &str,
) {
    if missing_keys.is_empty() {
        return;
    }

    result.push("# guardian-managed: D3 baseline keys".to_string());
    for key in missing_keys.iter() {
        result.push(render_key_line(key, baseline));
    }
    missing_keys.clear();
}

fn append_new_section(
    result: &mut Vec<String>,
    baseline: &WslBaseline,
    section: &str,
    missing_keys: &[String],
) {
    if result
        .last()
        .map(|line| !line.trim().is_empty())
        .unwrap_or(false)
    {
        result.push(String::new());
    }
    result.push(format!("[{section}]"));
    result.push("# guardian-managed: D3 baseline keys".to_string());
    for key in missing_keys {
        result.push(render_key_line(key, baseline));
    }
}

fn render_key_line(key: &str, baseline: &WslBaseline) -> String {
    match key {
        "wsl2.memory" => format!("memory={}", baseline.memory_value()),
        "wsl2.processors" => format!("processors={}", baseline.processors),
        "wsl2.swap" => format!("swap={}", baseline.swap_value()),
        "experimental.autoMemoryReclaim" => {
            format!("autoMemoryReclaim={}", baseline.auto_memory_reclaim)
        }
        _ => key.to_string(),
    }
}

fn normalize_wslconfig_lines(lines: &[String]) -> String {
    let mut normalized = lines.join("\r\n");
    if !normalized.ends_with("\r\n") {
        normalized.push_str("\r\n");
    }
    normalized
}

fn parse_section_name(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('[') || !trimmed.ends_with(']') {
        return None;
    }

    let section = trimmed.trim_matches(['[', ']']);
    if section.is_empty() {
        None
    } else {
        Some(section.trim().to_ascii_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use guardian_core::GuardianError;

    use super::{
        RuntimeRecoveryPlan, RuntimeSnapshot, WslBaseline, decide_runtime_recovery_plan,
        execute_runtime_recovery_with, merge_managed_wslconfig, missing_keys,
        validate_wslconfig_text, wait_for_runtime_health_with,
    };
    use std::{cell::RefCell, time::Duration};

    fn baseline() -> WslBaseline {
        WslBaseline {
            memory_gb: 8,
            processors: 6,
            swap_gb: 4,
            auto_memory_reclaim: "gradual".to_string(),
        }
    }

    #[test]
    fn creates_new_file_with_full_baseline() {
        let rendered = merge_managed_wslconfig("", &baseline()).expect("expected new file");
        assert!(rendered.contains("[wsl2]"));
        assert!(rendered.contains("memory=8GB"));
        assert!(rendered.contains("processors=6"));
        assert!(rendered.contains("swap=4GB"));
        assert!(rendered.contains("[experimental]"));
        assert!(rendered.contains("autoMemoryReclaim=gradual"));
    }

    #[test]
    fn appends_missing_keys_without_overwriting_existing_values() {
        let rendered = merge_managed_wslconfig(
            "[wsl2]\r\nmemory=12GB\r\n\r\n[experimental]\r\n",
            &baseline(),
        )
        .expect("expected merge");

        assert!(rendered.contains("memory=12GB"));
        assert!(rendered.contains("processors=6"));
        assert!(rendered.contains("swap=4GB"));
        assert!(rendered.contains("autoMemoryReclaim=gradual"));
    }

    #[test]
    fn reports_missing_keys_after_partial_config() {
        let missing = missing_keys("[wsl2]\nprocessors=6\n");
        assert_eq!(
            missing,
            vec![
                "wsl2.memory".to_string(),
                "wsl2.swap".to_string(),
                "experimental.autoMemoryReclaim".to_string()
            ]
        );
    }

    #[test]
    fn rejects_unparsable_lines() {
        let error = validate_wslconfig_text("[wsl2]\nthis is not parseable\n")
            .expect_err("expected parse failure");
        assert!(error.to_string().contains("unparsable line"));
    }

    #[test]
    fn blocks_runtime_restart_when_running_containers_exist() {
        let snapshot = RuntimeSnapshot {
            anomaly_detected: true,
            docker_desktop_cli_available: true,
            docker_desktop_status: Some("running".to_string()),
            running_containers: Some(3),
        };

        let plan = decide_runtime_recovery_plan(&snapshot);
        assert!(matches!(plan, RuntimeRecoveryPlan::Blocked(_)));
    }

    #[test]
    fn allows_runtime_restart_when_no_containers_are_running() {
        let snapshot = RuntimeSnapshot {
            anomaly_detected: true,
            docker_desktop_cli_available: true,
            docker_desktop_status: Some("running".to_string()),
            running_containers: Some(0),
        };

        let plan = decide_runtime_recovery_plan(&snapshot);
        assert_eq!(plan, RuntimeRecoveryPlan::Restart);
    }

    #[test]
    fn allows_runtime_start_when_desktop_is_stopped() {
        let snapshot = RuntimeSnapshot {
            anomaly_detected: true,
            docker_desktop_cli_available: true,
            docker_desktop_status: Some("stopped".to_string()),
            running_containers: None,
        };

        let plan = decide_runtime_recovery_plan(&snapshot);
        assert_eq!(plan, RuntimeRecoveryPlan::Start);
    }

    #[test]
    fn blocks_runtime_restart_when_docker_desktop_cli_is_unavailable() {
        let snapshot = RuntimeSnapshot {
            anomaly_detected: true,
            docker_desktop_cli_available: false,
            docker_desktop_status: Some("running".to_string()),
            running_containers: Some(0),
        };

        let plan = decide_runtime_recovery_plan(&snapshot);
        assert!(matches!(
            plan,
            RuntimeRecoveryPlan::Blocked(reason)
            if reason.contains("docker desktop")
        ));
    }

    #[test]
    fn blocks_runtime_restart_when_zero_container_proof_is_missing() {
        let snapshot = RuntimeSnapshot {
            anomaly_detected: true,
            docker_desktop_cli_available: true,
            docker_desktop_status: Some("running".to_string()),
            running_containers: None,
        };

        let plan = decide_runtime_recovery_plan(&snapshot);
        assert!(matches!(
            plan,
            RuntimeRecoveryPlan::Blocked(reason)
            if reason.contains("could not prove")
        ));
    }

    #[test]
    fn runtime_recovery_records_shutdown_failure() {
        let mut command_runner = |program: &str, _args: &[&str]| -> Result<String, GuardianError> {
            if program == "wsl" {
                Err(GuardianError::invalid_state("simulated shutdown failure"))
            } else {
                Ok(String::new())
            }
        };
        let mut collect_snapshot = || RuntimeSnapshot {
            anomaly_detected: true,
            docker_desktop_cli_available: true,
            docker_desktop_status: Some("running".to_string()),
            running_containers: Some(0),
        };

        let attempt = execute_runtime_recovery_with(
            "restart",
            &mut command_runner,
            &mut collect_snapshot,
            3,
            Duration::ZERO,
        );

        assert!(attempt.attempted);
        assert!(!attempt.blocked);
        assert_eq!(attempt.action.as_deref(), Some("restart"));
        assert!(
            attempt
                .error
                .as_deref()
                .expect("shutdown failure should be recorded")
                .contains("wsl --shutdown")
        );
        assert_eq!(attempt.wait.poll_count, 0);
        assert_eq!(attempt.wait.stable_success_polls, 0);
        assert!(!attempt.wait.timed_out);
    }

    #[test]
    fn runtime_recovery_records_docker_desktop_failure() {
        let mut command_runner = |program: &str, _args: &[&str]| -> Result<String, GuardianError> {
            match program {
                "wsl" => Ok(String::new()),
                "docker" => Err(GuardianError::invalid_state(
                    "simulated docker restart failure",
                )),
                _ => Ok(String::new()),
            }
        };
        let mut collect_snapshot = || RuntimeSnapshot {
            anomaly_detected: true,
            docker_desktop_cli_available: true,
            docker_desktop_status: Some("running".to_string()),
            running_containers: Some(0),
        };

        let attempt = execute_runtime_recovery_with(
            "restart",
            &mut command_runner,
            &mut collect_snapshot,
            3,
            Duration::ZERO,
        );

        assert!(attempt.attempted);
        assert!(!attempt.blocked);
        assert_eq!(attempt.action.as_deref(), Some("restart"));
        assert!(
            attempt
                .error
                .as_deref()
                .expect("docker desktop failure should be recorded")
                .contains("docker desktop restart")
        );
        assert_eq!(attempt.wait.poll_count, 0);
        assert_eq!(attempt.wait.stable_success_polls, 0);
        assert!(!attempt.wait.timed_out);
    }

    #[test]
    fn runtime_recovery_reports_timeout_after_successful_commands_if_health_never_stabilizes() {
        let mut command_runner =
            |_program: &str, _args: &[&str]| -> Result<String, GuardianError> { Ok(String::new()) };
        let snapshots = RefCell::new(vec![
            RuntimeSnapshot {
                anomaly_detected: true,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: false,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: true,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
        ]);
        let mut collect_snapshot = || snapshots.borrow_mut().remove(0);

        let attempt = execute_runtime_recovery_with(
            "restart",
            &mut command_runner,
            &mut collect_snapshot,
            3,
            Duration::ZERO,
        );

        assert!(attempt.attempted);
        assert!(!attempt.blocked);
        assert!(attempt.error.is_none());
        assert_eq!(attempt.wait.poll_count, 3);
        assert_eq!(attempt.wait.stable_success_polls, 0);
        assert!(attempt.wait.timed_out);
        assert!(attempt.wait.snapshot_after.anomaly_detected);
    }

    #[test]
    fn runtime_wait_requires_consecutive_healthy_polls() {
        let snapshots = RefCell::new(vec![
            RuntimeSnapshot {
                anomaly_detected: true,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: false,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: false,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
        ]);

        let wait =
            wait_for_runtime_health_with(|| snapshots.borrow_mut().remove(0), 5, Duration::ZERO);

        assert_eq!(wait.poll_count, 3);
        assert_eq!(wait.stable_success_polls, 2);
        assert!(!wait.timed_out);
        assert!(!wait.snapshot_after.anomaly_detected);
    }

    #[test]
    fn runtime_wait_resets_stable_polls_on_regression() {
        let snapshots = RefCell::new(vec![
            RuntimeSnapshot {
                anomaly_detected: true,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: false,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: true,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: false,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: false,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
        ]);

        let wait =
            wait_for_runtime_health_with(|| snapshots.borrow_mut().remove(0), 5, Duration::ZERO);

        assert_eq!(wait.poll_count, 5);
        assert_eq!(wait.stable_success_polls, 2);
        assert!(!wait.timed_out);
    }

    #[test]
    fn runtime_wait_times_out_without_stable_recovery() {
        let snapshots = RefCell::new(vec![
            RuntimeSnapshot {
                anomaly_detected: true,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: false,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
            RuntimeSnapshot {
                anomaly_detected: true,
                docker_desktop_cli_available: true,
                docker_desktop_status: Some("running".to_string()),
                running_containers: Some(0),
            },
        ]);

        let wait =
            wait_for_runtime_health_with(|| snapshots.borrow_mut().remove(0), 3, Duration::ZERO);

        assert_eq!(wait.poll_count, 3);
        assert_eq!(wait.stable_success_polls, 0);
        assert!(wait.timed_out);
        assert!(wait.snapshot_after.anomaly_detected);
    }
}
