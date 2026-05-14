use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use chrono::Local;
use guardian_core::{GuardianError, types::ActionPlan};
use guardian_observers::codex as codex_observer;
use guardian_windows::{
    codex_config::{append_trusted_project_entries, codex_config_path, missing_project_trust_keys},
    paths::{codex_home_dir, guardian_backup_dir, latest_codex_state_db},
    process::run_command_with_cmd_fallback,
};
use rusqlite::{Connection, params};

const SCRIPT_OUTPUT_LIMIT: usize = 8;
const REPAIR_PREFIX: &str = "[codex-resume-repair]";
const BACKUP_PREFIX: &str = "SQLite backup:";
const ACTIVE_VERSION_PREFIX: &str = "Repair complete. Active version:";
const SESSION_ARCHIVE_GRACE_DAYS: i64 = 30;

/// Trusted PowerShell repair script bundled into `guardian.exe`. The original lives at
/// `apps/guardian/assets/tools/repair-codex-resume.ps1` and is materialized to
/// `<codex_home>/tools/repair-codex-resume.ps1` on first repair run so end users never see
/// the historical "trusted repair script is missing" hard-fail.
const EMBEDDED_REPAIR_SCRIPT: &str =
    include_str!("../../../apps/guardian/assets/tools/repair-codex-resume.ps1");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexRepairOutcome {
    Noop,
    Repaired,
    Unresolved,
}

impl CodexRepairOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Noop => "noop",
            Self::Repaired => "repaired",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CodexRepairExecution {
    pub script_path: Option<PathBuf>,
    pub state_db_path: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub stale_rows_before: Option<i64>,
    pub stale_rows_after: Option<i64>,
    pub old_sessions_before: Option<i64>,
    pub old_sessions_after: Option<i64>,
    pub old_session_archive_days: Option<i64>,
    pub active_version: Option<String>,
    pub stdout_excerpt: Vec<String>,
    pub stderr_excerpt: Vec<String>,
    pub outcome: CodexRepairOutcome,
    pub trust_repair: Option<CodexTrustRepair>,
    pub slow_path_repair: Option<CodexSlowPathRepair>,
    pub slow_path_error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CodexTrustRepair {
    pub target_project_path: PathBuf,
    pub target_source: String,
    pub config_path: PathBuf,
    pub config_backup_path: Option<PathBuf>,
    pub missing_keys_before: Vec<String>,
    pub added_keys: Vec<String>,
    pub created_config: bool,
}

#[derive(Debug, Clone)]
pub struct CodexSlowPathRepair {
    pub launcher_path: PathBuf,
    pub launcher_backup_path: Option<PathBuf>,
    pub hotfix_binary_path: PathBuf,
    pub hotfix_source_path: PathBuf,
    pub launcher_updated: bool,
    pub hotfix_binary_updated: bool,
}

impl CodexRepairExecution {
    pub fn is_successful(&self) -> bool {
        self.outcome != CodexRepairOutcome::Unresolved
    }

    pub fn outcome_summary(&self) -> String {
        let trust_added = self
            .trust_repair
            .as_ref()
            .is_some_and(|repair| !repair.added_keys.is_empty());
        let slow_path_repaired = self
            .slow_path_repair
            .as_ref()
            .is_some_and(|repair| repair.launcher_updated || repair.hotfix_binary_updated);
        let stale_repaired = matches!(
            (self.stale_rows_before, self.stale_rows_after),
            (Some(before), Some(after)) if before > 0 && after == 0
        );
        let old_sessions_archived = matches!(
            (self.old_sessions_before, self.old_sessions_after),
            (Some(before), Some(after)) if before > after
        );
        let old_session_archive_incomplete = matches!(
            (self.old_sessions_before, self.old_sessions_after),
            (Some(before), Some(after)) if before > 0 && after > 0
        );
        let mut repaired_steps = Vec::new();
        if stale_repaired {
            repaired_steps.push("cleared stale rows");
        }
        if old_sessions_archived {
            repaired_steps.push("archived Codex sessions older than 30 days");
        }
        if trust_added {
            repaired_steps.push("appended missing trusted project entries");
        }
        if slow_path_repaired {
            repaired_steps.push("staged the Codex slow-path launcher hotfix");
        }

        match self.outcome {
            CodexRepairOutcome::Noop => {
                "Codex repair confirm completed without changing stale rows, 30-day session archive state, trust entries, or slow-path launcher state."
                    .to_string()
            }
            CodexRepairOutcome::Repaired => {
                if repaired_steps.is_empty() {
                    "Codex repair confirm completed without changing stale rows, 30-day session archive state, trust entries, or slow-path launcher state."
                        .to_string()
                } else {
                    format!("Codex repair confirm {}.", repaired_steps.join(", "))
                }
            }
            CodexRepairOutcome::Unresolved => {
                let tail = if old_session_archive_incomplete {
                    "but some sessions older than 30 days remained unarchived after verification."
                } else if self.slow_path_error.is_some() {
                    "but the Codex slow-path launcher hotfix step was skipped due to an error (see notes)."
                } else {
                    "but stale rows still remain after verification."
                };
                if repaired_steps.is_empty() {
                    format!("Codex repair confirm executed, {tail}")
                } else {
                    format!("Codex repair confirm {}, {tail}", repaired_steps.join(", "))
                }
            }
        }
    }

    pub fn notes(&self) -> Vec<String> {
        let mut notes = Vec::new();

        if let Some(script_path) = &self.script_path {
            notes.push(format!(
                "Trusted script executed: {}",
                script_path.display()
            ));
        }

        if let Some(active_version) = &self.active_version {
            notes.push(format!(
                "Active Codex version after repair: {active_version}"
            ));
        }
        if let Some(backup_path) = &self.backup_path {
            notes.push(format!(
                "SQLite backup created at {}",
                backup_path.display()
            ));
        }
        if let (Some(before), Some(after), Some(days)) = (
            self.old_sessions_before,
            self.old_sessions_after,
            self.old_session_archive_days,
        ) && before > 0
        {
            notes.push(format!(
                "Codex session archive applied with a {days}-day retention window: old_unarchived_before={before}, old_unarchived_after={after}"
            ));
            notes.push(
                "Existing `codex resume` processes were not stopped; restart the stuck picker to observe the refreshed session list."
                    .to_string(),
            );
        }
        if let Some(trust_repair) = &self.trust_repair {
            notes.push(format!(
                "Trusted project target: {}",
                trust_repair.target_project_path.display()
            ));
            notes.push(format!(
                "Trusted project source: {}",
                trust_repair.target_source
            ));
            if let Some(config_backup_path) = &trust_repair.config_backup_path {
                notes.push(format!(
                    "Codex config backup created at {}",
                    config_backup_path.display()
                ));
            }
            if !trust_repair.added_keys.is_empty() {
                notes.push(format!(
                    "Trusted project keys appended: {}",
                    trust_repair.added_keys.join(" | ")
                ));
            }
        }
        if let Some(slow_path_repair) = &self.slow_path_repair {
            notes.push(format!(
                "Codex slow-path launcher target: {}",
                slow_path_repair.launcher_path.display()
            ));
            notes.push(format!(
                "Codex slow-path hotfix source: {}",
                slow_path_repair.hotfix_source_path.display()
            ));
            notes.push(format!(
                "Codex slow-path hotfix binary path: {}",
                slow_path_repair.hotfix_binary_path.display()
            ));
            if let Some(launcher_backup_path) = &slow_path_repair.launcher_backup_path {
                notes.push(format!(
                    "Codex launcher backup created at {}",
                    launcher_backup_path.display()
                ));
            }
            notes.push(format!(
                "Codex slow-path launcher updated: {}",
                slow_path_repair.launcher_updated
            ));
            notes.push(format!(
                "Codex slow-path hotfix binary updated: {}",
                slow_path_repair.hotfix_binary_updated
            ));
        }
        if let Some(slow_path_error) = &self.slow_path_error {
            notes.push(format!(
                "Codex slow-path launcher hotfix was skipped: {slow_path_error}"
            ));
        }
        if !self.stdout_excerpt.is_empty() {
            notes.push(format!(
                "Script stdout excerpt: {}",
                self.stdout_excerpt.join(" | ")
            ));
        }
        if !self.stderr_excerpt.is_empty() {
            notes.push(format!(
                "Script stderr excerpt: {}",
                self.stderr_excerpt.join(" | ")
            ));
        }

        notes
    }
}

pub fn planned_actions(project_path: Option<&Path>) -> Vec<ActionPlan> {
    let dry_run = command_with_project_path("guardian repair codex --dry-run", project_path);
    let confirm = command_with_project_path("guardian repair codex --confirm", project_path);
    vec![
        ActionPlan::new(
            dry_run,
            "Preview the Codex repair chain, including trust recovery, 30-day session archiving, and slow-path launcher staging when those drifts are identified."
                .to_string(),
            false,
        ),
        ActionPlan::new(
            confirm,
            "Execute the managed Codex repair chain with backup, verification, audit, 30-day session archiving, and controlled slow-path launcher hotfix staging."
                .to_string(),
            true,
        ),
    ]
}

pub fn execute_confirmed(
    project_path: Option<&Path>,
) -> Result<CodexRepairExecution, GuardianError> {
    let observer_report = codex_observer::observe_with_target(project_path)?;
    let repair_stale_rows = domain_has_failure_class(&observer_report, "C2");
    let repair_old_sessions = domain_has_failure_class(&observer_report, "C4")
        && domain_evidence_i64(&observer_report, "old_unarchived_threads")
            .is_some_and(|count| count > 0);
    let repair_slow_path = slow_path_repair_required();
    let trust_target = trust_target_from_report(&observer_report);

    let mut execution = CodexRepairExecution {
        script_path: None,
        state_db_path: None,
        backup_path: None,
        stale_rows_before: None,
        stale_rows_after: None,
        old_sessions_before: None,
        old_sessions_after: None,
        old_session_archive_days: None,
        active_version: None,
        stdout_excerpt: Vec::new(),
        stderr_excerpt: Vec::new(),
        outcome: CodexRepairOutcome::Noop,
        trust_repair: None,
        slow_path_repair: None,
        slow_path_error: None,
    };

    if repair_stale_rows || repair_old_sessions {
        let codex_home = codex_home_dir().map_err(GuardianError::Io)?;
        let state_db_path = latest_codex_state_db(&codex_home)
            .map_err(GuardianError::Io)?
            .ok_or_else(|| {
                GuardianError::invalid_state(format!(
                    "expected a `state_*.sqlite` database under `{}` but none was found",
                    codex_home.display()
                ))
            })?;
        execution.state_db_path = Some(state_db_path.clone());

        if repair_stale_rows {
            let script_path =
                ensure_repair_script_installed(&codex_home).map_err(GuardianError::Io)?;
            if !script_path.exists() {
                return Err(GuardianError::invalid_state(format!(
                    "trusted repair script is missing: {}",
                    script_path.display()
                )));
            }

            let stale_rows_before = inspect_stale_rows(&state_db_path)?;
            let target_version = current_codex_version();
            let process_output = run_repair_script(
                &script_path,
                &codex_home,
                &state_db_path,
                target_version.as_deref(),
            )?;
            let stale_rows_after = inspect_stale_rows(&state_db_path)?;

            execution.script_path = Some(script_path);
            execution.backup_path = backup_path_from_output(&process_output.stdout);
            execution.stale_rows_before = Some(stale_rows_before);
            execution.stale_rows_after = Some(stale_rows_after);
            execution.active_version = active_version_from_output(&process_output.stdout);
            execution.stdout_excerpt = excerpt_lines(&process_output.stdout);
            execution.stderr_excerpt = excerpt_lines(&process_output.stderr);

            if stale_rows_after > 0 {
                execution.outcome = CodexRepairOutcome::Unresolved;
            } else if stale_rows_before > 0 {
                execution.outcome = CodexRepairOutcome::Repaired;
            }
        }

        if repair_old_sessions {
            let old_sessions_before =
                inspect_old_unarchived_sessions(&state_db_path, SESSION_ARCHIVE_GRACE_DAYS)?;
            if old_sessions_before > 0 && execution.backup_path.is_none() {
                execution.backup_path = Some(backup_state_db(
                    &state_db_path,
                    &codex_home.join("backups"),
                    "pre-old-session-archive",
                )?);
            }
            archive_old_sessions(&state_db_path, SESSION_ARCHIVE_GRACE_DAYS)?;
            let old_sessions_after =
                inspect_old_unarchived_sessions(&state_db_path, SESSION_ARCHIVE_GRACE_DAYS)?;

            execution.old_sessions_before = Some(old_sessions_before);
            execution.old_sessions_after = Some(old_sessions_after);
            execution.old_session_archive_days = Some(SESSION_ARCHIVE_GRACE_DAYS);

            if old_sessions_after > 0 && old_sessions_before > 0 {
                execution.outcome = CodexRepairOutcome::Unresolved;
            } else if old_sessions_before > old_sessions_after {
                execution.outcome = CodexRepairOutcome::Repaired;
            }
        }
    }

    if let Some(target) = trust_target {
        let trust_repair = apply_project_trust_repair(&target)?;
        if !trust_repair.added_keys.is_empty() {
            execution.outcome = CodexRepairOutcome::Repaired;
        }
        execution.trust_repair = Some(trust_repair);
    }

    if repair_slow_path {
        match apply_slow_path_repair() {
            Ok(slow_path_repair) => {
                if slow_path_repair.launcher_updated || slow_path_repair.hotfix_binary_updated {
                    execution.outcome = CodexRepairOutcome::Repaired;
                }
                execution.slow_path_repair = Some(slow_path_repair);
            }
            Err(error) => {
                // Slow-path failures must not discard successful stale-row or trust
                // repair work. Capture the failure so the caller can persist an audit
                // record and surface actionable evidence to CLI/GUI/tray outputs.
                execution.slow_path_error = Some(error.to_string());
                if execution.outcome == CodexRepairOutcome::Noop {
                    execution.outcome = CodexRepairOutcome::Unresolved;
                }
            }
        }
    }

    Ok(execution)
}

fn repair_script_path(codex_home: &Path) -> PathBuf {
    codex_home.join("tools").join("repair-codex-resume.ps1")
}

/// Materialize the bundled repair script into `<codex_home>/tools/` if it is missing.
///
/// Returns the canonical script path. The function is idempotent: when the destination
/// already exists its bytes are left untouched so user-side edits or upgrade-time backups
/// are preserved. Failures during write surface as `io::Error` so callers can propagate
/// them through `GuardianError::Io`.
pub fn ensure_repair_script_installed(codex_home: &Path) -> std::io::Result<PathBuf> {
    let script_path = repair_script_path(codex_home);
    if script_path.exists() {
        return Ok(script_path);
    }
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&script_path, EMBEDDED_REPAIR_SCRIPT.as_bytes())?;
    Ok(script_path)
}

/// Best-effort startup hook that lays down every Guardian-owned helper under `~/.codex/tools/`.
///
/// Today only `repair-codex-resume.ps1` is bundled; future additions should plug in here so
/// the app entry point keeps a single deploy call. Errors are returned to the caller, which
/// is expected to log-and-continue rather than abort startup.
pub fn ensure_codex_tools_deployed() -> std::io::Result<PathBuf> {
    let codex_home = codex_home_dir()?;
    fs::create_dir_all(&codex_home)?;
    ensure_repair_script_installed(&codex_home)
}

fn inspect_stale_rows(path: &Path) -> Result<i64, GuardianError> {
    let connection = Connection::open(path)
        .map_err(|error| GuardianError::invalid_state(format!("sqlite open failed: {error}")))?;
    connection
        .query_row(
            "select count(*) from threads where has_user_event = 0 and trim(coalesce(first_user_message, '')) <> ''",
            [],
            |row| row.get(0),
        )
        .map_err(|error| GuardianError::invalid_state(format!("stale row query failed: {error}")))
}

fn inspect_old_unarchived_sessions(path: &Path, days: i64) -> Result<i64, GuardianError> {
    let connection = Connection::open(path)
        .map_err(|error| GuardianError::invalid_state(format!("sqlite open failed: {error}")))?;
    let archive_cutoff = archive_cutoff_epoch(days);
    connection
        .query_row(
            "select count(*) from threads where archived = 0 and created_at < ?1",
            [archive_cutoff],
            |row| row.get(0),
        )
        .map_err(|error| {
            GuardianError::invalid_state(format!("old session archive query failed: {error}"))
        })
}

fn archive_old_sessions(path: &Path, days: i64) -> Result<usize, GuardianError> {
    let mut connection = Connection::open(path)
        .map_err(|error| GuardianError::invalid_state(format!("sqlite open failed: {error}")))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| {
            GuardianError::invalid_state(format!("sqlite busy timeout failed: {error}"))
        })?;

    let archive_cutoff = archive_cutoff_epoch(days);
    let archived_at = current_epoch_seconds();
    let transaction = connection.transaction().map_err(|error| {
        GuardianError::invalid_state(format!("sqlite transaction failed: {error}"))
    })?;
    let changed = transaction
        .execute(
            "update threads set archived = 1, archived_at = ?1 where archived = 0 and created_at < ?2",
            params![archived_at, archive_cutoff],
        )
        .map_err(|error| {
            GuardianError::invalid_state(format!("old session archive update failed: {error}"))
        })?;
    transaction
        .commit()
        .map_err(|error| GuardianError::invalid_state(format!("sqlite commit failed: {error}")))?;
    Ok(changed)
}

fn backup_state_db(
    state_db_path: &Path,
    backup_root: &Path,
    reason: &str,
) -> Result<PathBuf, GuardianError> {
    fs::create_dir_all(backup_root)?;
    let state_db_file_name = state_db_path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("state.sqlite");
    let backup_path = backup_root.join(format!(
        "{}.{}-{}.bak",
        state_db_file_name,
        reason,
        Local::now().format("%Y%m%d-%H%M%S")
    ));
    let connection = Connection::open(state_db_path)
        .map_err(|error| GuardianError::invalid_state(format!("sqlite open failed: {error}")))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| {
            GuardianError::invalid_state(format!("sqlite busy timeout failed: {error}"))
        })?;
    let backup_target = backup_path.display().to_string();
    connection
        .execute(
            &format!("VACUUM main INTO {}", quote_sqlite_literal(&backup_target)),
            [],
        )
        .map_err(|error| {
            GuardianError::invalid_state(format!(
                "sqlite backup failed for `{}`: {error}",
                state_db_path.display()
            ))
        })?;
    Ok(backup_path)
}

fn archive_cutoff_epoch(days: i64) -> i64 {
    current_epoch_seconds().saturating_sub(days.saturating_mul(86_400))
}

fn current_epoch_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn quote_sqlite_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn run_repair_script(
    script_path: &Path,
    codex_home: &Path,
    state_db_path: &Path,
    target_version: Option<&str>,
) -> Result<ProcessOutput, GuardianError> {
    let bootstrap = render_repair_script_bootstrap(
        script_path,
        codex_home,
        state_db_path,
        target_version,
        resolve_codex_shim_path().as_deref(),
    );
    let args = vec![
        OsString::from("-NoProfile"),
        OsString::from("-ExecutionPolicy"),
        OsString::from("Bypass"),
        OsString::from("-Command"),
        OsString::from(bootstrap),
    ];
    run_command_with_extra_path("powershell", &args, &repair_script_path_entries()?)
}

fn current_codex_version() -> Option<String> {
    let args = [OsString::from("--version")];
    let output = run_command_with_cmd_fallback("codex", &args).ok()?;
    if !output.success() {
        return None;
    }
    parse_semver_fragment(&output.stdout)
}

fn command_with_project_path(base: &str, project_path: Option<&Path>) -> String {
    project_path.map_or_else(
        || base.to_string(),
        |path| {
            let rendered = path.display().to_string().replace('"', "\\\"");
            format!("{base} --project-path \"{rendered}\"")
        },
    )
}

fn domain_has_failure_class(report: &guardian_core::types::DomainReport, class: &str) -> bool {
    domain_evidence_value(report, "failure_classes")
        .map(|value| value.split(',').any(|item| item.trim() == class))
        .unwrap_or(false)
}

fn domain_evidence_value<'a>(
    report: &'a guardian_core::types::DomainReport,
    key: &str,
) -> Option<&'a str> {
    report
        .evidence
        .iter()
        .find(|item| item.key == key)
        .map(|item| item.value.as_str())
}

fn domain_evidence_i64(report: &guardian_core::types::DomainReport, key: &str) -> Option<i64> {
    domain_evidence_value(report, key)?.trim().parse().ok()
}

#[derive(Debug, Clone)]
struct TrustRepairTarget {
    path: PathBuf,
    source: String,
}

fn trust_target_from_report(
    report: &guardian_core::types::DomainReport,
) -> Option<TrustRepairTarget> {
    let path = domain_evidence_value(report, "trust_target_path")?;
    let source = domain_evidence_value(report, "trust_target_source")
        .unwrap_or("unknown")
        .to_string();
    let missing_keys = domain_evidence_value(report, "trust_missing_lookup_keys").unwrap_or("");
    if missing_keys.trim().is_empty() {
        return None;
    }

    Some(TrustRepairTarget {
        path: PathBuf::from(path),
        source,
    })
}

fn apply_project_trust_repair(
    target: &TrustRepairTarget,
) -> Result<CodexTrustRepair, GuardianError> {
    let config_path = codex_config_path().map_err(GuardianError::Io)?;
    let created_config = !config_path.exists();
    let existing_text = if created_config {
        String::new()
    } else {
        fs::read_to_string(&config_path)?
    };
    let missing_keys_before = missing_project_trust_keys(&existing_text, &target.path)
        .map_err(|error| GuardianError::invalid_state(format!("invalid codex config: {error}")))?;

    if missing_keys_before.is_empty() {
        return Ok(CodexTrustRepair {
            target_project_path: target.path.clone(),
            target_source: target.source.clone(),
            config_path,
            config_backup_path: None,
            missing_keys_before,
            added_keys: Vec::new(),
            created_config,
        });
    }

    let config_backup_path = backup_codex_config(&config_path, &existing_text)?;
    let rendered = append_trusted_project_entries(&existing_text, &missing_keys_before);
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&config_path, rendered)?;

    let verified_text = fs::read_to_string(&config_path)?;
    let missing_after = missing_project_trust_keys(&verified_text, &target.path)
        .map_err(|error| GuardianError::invalid_state(format!("invalid codex config: {error}")))?;
    if !missing_after.is_empty() {
        return Err(GuardianError::invalid_state(format!(
            "Codex trust verification failed for `{}`; {} expected lookup key(s) still missing after write",
            target.path.display(),
            missing_after.len()
        )));
    }

    Ok(CodexTrustRepair {
        target_project_path: target.path.clone(),
        target_source: target.source.clone(),
        config_path,
        config_backup_path,
        missing_keys_before: missing_keys_before.clone(),
        added_keys: missing_keys_before,
        created_config,
    })
}

fn backup_codex_config(
    config_path: &Path,
    existing_text: &str,
) -> Result<Option<PathBuf>, GuardianError> {
    if !config_path.exists() {
        return Ok(None);
    }

    let backup_dir = guardian_backup_dir().map_err(GuardianError::Io)?;
    fs::create_dir_all(&backup_dir)?;
    let backup_path = backup_dir.join(format!(
        "codex-config-{}.toml.bak",
        Local::now().format("%Y%m%d-%H%M%S")
    ));
    fs::write(&backup_path, existing_text)?;
    Ok(Some(backup_path))
}

fn apply_slow_path_repair() -> Result<CodexSlowPathRepair, GuardianError> {
    let package_root = resolve_codex_package_root()?;
    let target_triple = current_target_triple()?;
    let launcher_path = package_root.join("bin").join("codex.js");
    if !launcher_path.exists() {
        return Err(GuardianError::invalid_state(format!(
            "expected Codex launcher at `{}` but it is missing",
            launcher_path.display()
        )));
    }

    let hotfix_binary_path = package_root
        .join("vendor-hotfix")
        .join(target_triple)
        .join("codex")
        .join(codex_binary_name());
    let hotfix_source_candidates = hotfix_source_candidates(
        &package_root,
        &env::temp_dir(),
        bundled_hotfix_root().as_deref(),
        target_triple,
        codex_binary_name(),
    );
    let hotfix_source_path = hotfix_source_candidates
        .iter()
        .find(|candidate| candidate.exists())
        .cloned()
        .ok_or_else(|| {
            let checked_paths = hotfix_source_candidates
                .iter()
                .map(|candidate| format!("`{}`", candidate.display()))
                .collect::<Vec<_>>()
                .join(", ");
            GuardianError::invalid_state(format!(
                "unable to locate a verified Codex hotfix binary; checked {checked_paths}"
            ))
        })?;

    let hotfix_binary_updated = stage_hotfix_binary(&hotfix_source_path, &hotfix_binary_path)?;
    let launcher_text = fs::read_to_string(&launcher_path)?;
    let (patched_launcher_text, launcher_updated) = ensure_hotfix_launcher_patch(&launcher_text)?;
    let launcher_backup_path = if launcher_updated {
        let backup_path = backup_codex_launcher(&launcher_path, &launcher_text)?;
        fs::write(&launcher_path, patched_launcher_text)?;
        Some(backup_path)
    } else {
        None
    };

    let verified_launcher_text = fs::read_to_string(&launcher_path)?;
    if !verified_launcher_text.contains("vendor-hotfix")
        || !verified_launcher_text.contains("existsSync(hotfixBinaryPath)")
    {
        return Err(GuardianError::invalid_state(format!(
            "Codex launcher verification failed after patching `{}`",
            launcher_path.display()
        )));
    }
    if !hotfix_binary_path.exists() {
        return Err(GuardianError::invalid_state(format!(
            "expected staged hotfix binary at `{}` after repair",
            hotfix_binary_path.display()
        )));
    }

    Ok(CodexSlowPathRepair {
        launcher_path,
        launcher_backup_path,
        hotfix_binary_path,
        hotfix_source_path,
        launcher_updated,
        hotfix_binary_updated,
    })
}

fn slow_path_repair_required() -> bool {
    let Ok(package_root) = resolve_codex_package_root() else {
        return false;
    };
    let Ok(target_triple) = current_target_triple() else {
        return false;
    };
    let launcher_path = package_root.join("bin").join("codex.js");
    let Ok(launcher_text) = fs::read_to_string(&launcher_path) else {
        return false;
    };
    let hotfix_binary_path = package_root
        .join("vendor-hotfix")
        .join(target_triple)
        .join("codex")
        .join(codex_binary_name());
    let Ok(hotfix_source) = find_hotfix_source_binary(&package_root, target_triple) else {
        return false;
    };
    let Some(hotfix_source) = hotfix_source else {
        return false;
    };

    let launcher_missing_hotfix = !launcher_text.contains("vendor-hotfix")
        || !launcher_text.contains("existsSync(hotfixBinaryPath)");
    launcher_missing_hotfix || (hotfix_source != hotfix_binary_path && !hotfix_binary_path.exists())
}

fn resolve_codex_package_root() -> Result<PathBuf, GuardianError> {
    for candidate in codex_package_root_candidates()? {
        if candidate.join("bin").join("codex.js").exists() {
            return Ok(candidate);
        }
    }

    Err(GuardianError::invalid_state(
        "unable to locate the global `@openai/codex` package root on this machine",
    ))
}

fn codex_package_root_candidates() -> Result<Vec<PathBuf>, GuardianError> {
    let mut candidates = Vec::new();

    if let Some(app_data) = env::var_os("APPDATA") {
        candidates.push(
            PathBuf::from(app_data)
                .join("npm")
                .join("node_modules")
                .join("@openai")
                .join("codex"),
        );
    }

    let args = [OsString::from("root"), OsString::from("-g")];
    if let Ok(output) = run_command_with_cmd_fallback("npm", &args)
        && output.success()
    {
        let npm_root = output.stdout.trim();
        if !npm_root.is_empty() {
            candidates.push(PathBuf::from(npm_root).join("@openai").join("codex"));
        }
    }

    dedupe_paths(candidates)
}

fn find_hotfix_source_binary(
    package_root: &Path,
    target_triple: &str,
) -> Result<Option<PathBuf>, GuardianError> {
    let hotfix_binary_name = codex_binary_name();
    for candidate in hotfix_source_candidates(
        package_root,
        &env::temp_dir(),
        bundled_hotfix_root().as_deref(),
        target_triple,
        hotfix_binary_name,
    ) {
        if candidate.exists() {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn hotfix_source_candidates(
    package_root: &Path,
    temp_root: &Path,
    bundled_root: Option<&Path>,
    target_triple: &str,
    binary_name: &str,
) -> Vec<PathBuf> {
    let mut candidates = vec![
        package_root
            .join("vendor-hotfix")
            .join(target_triple)
            .join("codex")
            .join(binary_name),
    ];
    if let Some(bundled_root) = bundled_root {
        candidates.push(
            bundled_root
                .join(target_triple)
                .join("codex")
                .join(binary_name),
        );
    }
    candidates.push(
        temp_root
            .join("codex-src")
            .join("codex-rs")
            .join("target")
            .join("release")
            .join(binary_name),
    );
    dedupe_paths(candidates).expect("path dedupe should be infallible")
}

fn bundled_hotfix_root() -> Option<PathBuf> {
    let current_exe = env::current_exe().ok()?;
    let exe_dir = current_exe.parent()?;
    Some(exe_dir.join("vendor-hotfix"))
}

fn current_target_triple() -> Result<&'static str, GuardianError> {
    match (env::consts::OS, env::consts::ARCH) {
        ("windows", "x86_64") => Ok("x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => Ok("aarch64-pc-windows-msvc"),
        ("macos", "x86_64") => Ok("x86_64-apple-darwin"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-musl"),
        (os, arch) => Err(GuardianError::invalid_state(format!(
            "unsupported platform for Codex launcher staging: {os} ({arch})"
        ))),
    }
}

fn codex_binary_name() -> &'static str {
    if cfg!(windows) { "codex.exe" } else { "codex" }
}

fn stage_hotfix_binary(source_path: &Path, destination_path: &Path) -> Result<bool, GuardianError> {
    if source_path == destination_path {
        return Ok(false);
    }

    if destination_path.exists() && files_identical(source_path, destination_path)? {
        return Ok(false);
    }

    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source_path, destination_path)?;
    Ok(true)
}

fn files_identical(left: &Path, right: &Path) -> Result<bool, GuardianError> {
    let left_metadata = fs::metadata(left)?;
    let right_metadata = fs::metadata(right)?;
    if left_metadata.len() != right_metadata.len() {
        return Ok(false);
    }

    Ok(fs::read(left)? == fs::read(right)?)
}

fn ensure_hotfix_launcher_patch(contents: &str) -> Result<(String, bool), GuardianError> {
    if contents
        .contains("const hotfixVendorRoot = path.join(__dirname, \"..\", \"vendor-hotfix\");")
        && contents.contains("const hotfixBinaryPath = path.join(")
        && contents.contains("existsSync(hotfixBinaryPath)")
    {
        return Ok((contents.to_string(), false));
    }

    let original_vendor_block = r#"const codexBinaryName = process.platform === "win32" ? "codex.exe" : "codex";
const localVendorRoot = path.join(__dirname, "..", "vendor");
const localBinaryPath = path.join(
  localVendorRoot,
  targetTriple,
  "codex",
  codexBinaryName,
);"#;
    let patched_vendor_block = r#"const codexBinaryName = process.platform === "win32" ? "codex.exe" : "codex";
const localVendorRoot = path.join(__dirname, "..", "vendor");
const hotfixVendorRoot = path.join(__dirname, "..", "vendor-hotfix");
const localBinaryPath = path.join(
  localVendorRoot,
  targetTriple,
  "codex",
  codexBinaryName,
);
const hotfixBinaryPath = path.join(
  hotfixVendorRoot,
  targetTriple,
  "codex",
  codexBinaryName,
);"#;
    let patched_contents = contents.replacen(original_vendor_block, patched_vendor_block, 1);
    if patched_contents == contents {
        return Err(GuardianError::invalid_state(
            "unable to locate the expected Codex launcher vendor block",
        ));
    }

    let original_binary_line =
        r#"const binaryPath = path.join(archRoot, "codex", codexBinaryName);"#;
    let patched_binary_line = r#"const binaryPath = existsSync(hotfixBinaryPath)
  ? hotfixBinaryPath
  : path.join(archRoot, "codex", codexBinaryName);"#;
    let patched_contents = patched_contents.replacen(original_binary_line, patched_binary_line, 1);
    if !patched_contents.contains("existsSync(hotfixBinaryPath)") {
        return Err(GuardianError::invalid_state(
            "unable to inject the Codex launcher hotfix binary override",
        ));
    }

    Ok((patched_contents, true))
}

fn backup_codex_launcher(
    launcher_path: &Path,
    existing_text: &str,
) -> Result<PathBuf, GuardianError> {
    let backup_path = launcher_path.with_file_name(format!(
        "codex.js.pre-resume-hotfix-{}.bak",
        Local::now().format("%Y%m%d-%H%M%S")
    ));
    fs::write(&backup_path, existing_text)?;
    Ok(backup_path)
}

fn dedupe_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, GuardianError> {
    let mut deduped = Vec::new();
    for path in paths {
        if deduped.iter().any(|existing| existing == &path) {
            continue;
        }
        deduped.push(path);
    }
    Ok(deduped)
}

fn parse_semver_fragment(text: &str) -> Option<String> {
    text.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '.'))
        .find(|token| looks_like_semver(token))
        .map(ToOwned::to_owned)
}

fn looks_like_semver(token: &str) -> bool {
    let parts = token.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|ch| ch.is_ascii_digit()))
}

fn run_command_with_extra_path(
    program: &str,
    args: &[OsString],
    extra_paths: &[PathBuf],
) -> Result<ProcessOutput, GuardianError> {
    let resolved_path = if extra_paths.is_empty() {
        None
    } else {
        Some(build_path_with_prepend(extra_paths)?)
    };

    let output = match spawn_command(program, args, resolved_path.as_ref()) {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound && cfg!(target_os = "windows") => {
            spawn_cmd_fallback(program, args, resolved_path.as_ref())?
        }
        Err(error) => return Err(GuardianError::Io(error)),
    };

    let stdout = decode_output(&output.stdout).trim().to_string();
    let stderr = decode_output(&output.stderr).trim().to_string();
    if output.status.success() {
        Ok(ProcessOutput { stdout, stderr })
    } else {
        Err(GuardianError::CommandFailed {
            command: format!(
                "{} {}",
                program,
                args.iter()
                    .map(|arg| arg.to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            status: output.status.code().unwrap_or(-1),
            stderr: if stderr.is_empty() {
                stdout.clone()
            } else {
                stderr
            },
        })
    }
}

fn spawn_command(
    program: &str,
    args: &[OsString],
    path_override: Option<&OsString>,
) -> std::io::Result<std::process::Output> {
    let mut command = Command::new(program);
    command.args(args);
    if let Some(path_override) = path_override {
        command.env("PATH", path_override);
    }
    command.output()
}

fn spawn_cmd_fallback(
    program: &str,
    args: &[OsString],
    path_override: Option<&OsString>,
) -> std::io::Result<std::process::Output> {
    let mut command = Command::new("cmd");
    command.arg("/C").arg(program).args(args);
    if let Some(path_override) = path_override {
        command.env("PATH", path_override);
    }
    command.output()
}

fn repair_script_path_entries() -> Result<Vec<PathBuf>, GuardianError> {
    let mut entries = Vec::new();
    if let Some(app_data) = env::var_os("APPDATA") {
        entries.push(PathBuf::from(app_data).join("npm"));
    }
    if let Ok(package_root) = resolve_codex_package_root()
        && let Some(npm_root) = package_root
            .parent()
            .and_then(Path::parent)
            .and_then(Path::parent)
    {
        entries.push(npm_root.to_path_buf());
    }
    dedupe_paths(entries)
}

fn build_path_with_prepend(extra_paths: &[PathBuf]) -> Result<OsString, GuardianError> {
    let mut merged = extra_paths.to_vec();
    if let Some(existing) = env::var_os("PATH") {
        merged.extend(env::split_paths(&existing));
    }
    env::join_paths(merged).map_err(|error| {
        GuardianError::invalid_state(format!("failed to build PATH for Codex repair: {error}"))
    })
}

fn resolve_codex_shim_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(app_data) = env::var_os("APPDATA") {
        let npm_root = PathBuf::from(app_data).join("npm");
        candidates.push(npm_root.join("codex.cmd"));
        candidates.push(npm_root.join("codex"));
    }
    dedupe_paths(candidates)
        .ok()?
        .into_iter()
        .find(|candidate| candidate.exists())
}

fn render_repair_script_bootstrap(
    script_path: &Path,
    codex_home: &Path,
    state_db_path: &Path,
    target_version: Option<&str>,
    codex_shim_path: Option<&Path>,
) -> String {
    let mut commands = Vec::new();
    if let Some(codex_shim_path) = codex_shim_path {
        let version_probe = format!("\"{}\" --version 2>nul", codex_shim_path.display());
        commands.push(format!(
            "function codex {{ param([Parameter(ValueFromRemainingArguments=$true)][string[]]$cliArgs); if ($cliArgs.Count -eq 1 -and $cliArgs[0] -eq '--version') {{ & cmd /d /c {} }} else {{ & {} @cliArgs }} }}",
            quote_powershell_literal(version_probe),
            quote_powershell_literal(codex_shim_path),
        ));
    }

    let mut script_invocation = format!(
        "& {} -CodexHome {} -StateDbPath {} -SkipInstall",
        quote_powershell_literal(script_path),
        quote_powershell_literal(codex_home),
        quote_powershell_literal(state_db_path),
    );
    if let Some(target_version) = target_version {
        script_invocation.push_str(" -TargetVersion ");
        script_invocation.push_str(&quote_powershell_literal(target_version));
    }
    commands.push(script_invocation);

    format!("& {{ {} }}", commands.join("; "))
}

fn quote_powershell_literal(value: impl AsRef<OsStr>) -> String {
    let rendered = value.as_ref().to_string_lossy().replace('\'', "''");
    format!("'{rendered}'")
}

fn decode_output(bytes: &[u8]) -> String {
    let has_utf16_shape =
        bytes.len() >= 2 && bytes.iter().skip(1).step_by(2).any(|byte| *byte == 0);
    if has_utf16_shape {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn excerpt_lines(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(normalize_repair_prefix)
        .take(SCRIPT_OUTPUT_LIMIT)
        .map(ToOwned::to_owned)
        .collect()
}

fn normalize_repair_prefix(line: &str) -> &str {
    line.trim_start_matches(REPAIR_PREFIX).trim()
}

fn backup_path_from_output(stdout: &str) -> Option<PathBuf> {
    stdout.lines().find_map(|line| {
        let normalized = normalize_repair_prefix(line);
        normalized
            .strip_prefix(BACKUP_PREFIX)
            .map(|path| PathBuf::from(path.trim()))
    })
}

fn active_version_from_output(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let normalized = normalize_repair_prefix(line);
        normalized
            .strip_prefix(ACTIVE_VERSION_PREFIX)
            .map(|value| value.trim().to_string())
    })
}

struct ProcessOutput {
    stdout: String,
    stderr: String,
}

#[cfg(test)]
mod tests {
    use super::{
        ACTIVE_VERSION_PREFIX, BACKUP_PREFIX, CodexRepairExecution, CodexRepairOutcome,
        CodexTrustRepair, EMBEDDED_REPAIR_SCRIPT, REPAIR_PREFIX, active_version_from_output,
        backup_path_from_output, command_with_project_path, ensure_hotfix_launcher_patch,
        ensure_repair_script_installed, hotfix_source_candidates, normalize_repair_prefix,
        parse_semver_fragment, render_repair_script_bootstrap,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn parses_backup_path_from_script_output() {
        let stdout = format!(
            "{REPAIR_PREFIX} Step one\n{REPAIR_PREFIX} {BACKUP_PREFIX} C:\\Users\\example\\.codex\\backups\\state_9.sqlite.pre-has-user-event-heal-20260415-210000.bak"
        );
        let backup = backup_path_from_output(&stdout).expect("expected backup path");
        assert!(backup.ends_with("state_9.sqlite.pre-has-user-event-heal-20260415-210000.bak"));
    }

    #[test]
    fn parses_active_version_from_script_output() {
        let stdout = format!("{REPAIR_PREFIX} {ACTIVE_VERSION_PREFIX} codex-cli 0.121.0");
        let version = active_version_from_output(&stdout).expect("expected version");
        assert_eq!(version, "codex-cli 0.121.0");
    }

    #[test]
    fn strips_script_prefix_for_human_excerpt() {
        assert_eq!(
            normalize_repair_prefix("[codex-resume-repair] Hello world"),
            "Hello world"
        );
    }

    #[test]
    fn parses_semver_fragment_from_codex_version_output() {
        let version = parse_semver_fragment("codex-cli 0.121.0").expect("expected semver");
        assert_eq!(version, "0.121.0");
    }

    #[test]
    fn renders_command_with_project_path_override() {
        assert_eq!(
            command_with_project_path(
                "guardian repair codex --confirm",
                Some(Path::new(r"D:\Workspaces\Guardian Project")),
            ),
            r#"guardian repair codex --confirm --project-path "D:\Workspaces\Guardian Project""#
        );
    }

    #[test]
    fn launcher_patch_injects_vendor_hotfix_override() {
        let original = r#"const codexBinaryName = process.platform === "win32" ? "codex.exe" : "codex";
const localVendorRoot = path.join(__dirname, "..", "vendor");
const localBinaryPath = path.join(
  localVendorRoot,
  targetTriple,
  "codex",
  codexBinaryName,
);

let vendorRoot;
const archRoot = path.join(vendorRoot, targetTriple);
const binaryPath = path.join(archRoot, "codex", codexBinaryName);"#;

        let (patched, updated) =
            ensure_hotfix_launcher_patch(original).expect("launcher patch should apply");
        assert!(updated);
        assert!(
            patched.contains(
                "const hotfixVendorRoot = path.join(__dirname, \"..\", \"vendor-hotfix\");"
            )
        );
        assert!(patched.contains("const hotfixBinaryPath = path.join("));
        assert!(patched.contains("const binaryPath = existsSync(hotfixBinaryPath)"));
    }

    #[test]
    fn launcher_patch_is_idempotent() {
        let patched = r#"const hotfixVendorRoot = path.join(__dirname, "..", "vendor-hotfix");
const hotfixBinaryPath = path.join(
  hotfixVendorRoot,
  targetTriple,
  "codex",
  codexBinaryName,
);
const binaryPath = existsSync(hotfixBinaryPath)
  ? hotfixBinaryPath
  : path.join(archRoot, "codex", codexBinaryName);"#;

        let (second_pass, updated) =
            ensure_hotfix_launcher_patch(patched).expect("launcher patch should stay valid");
        assert!(!updated);
        assert_eq!(second_pass, patched);
    }

    fn sample_trust_repair() -> CodexTrustRepair {
        CodexTrustRepair {
            target_project_path: PathBuf::from(r"D:\Workspaces\Guardian Project"),
            target_source: "cli_argument".to_string(),
            config_path: PathBuf::from(r"C:\Users\Example\.codex\config.toml"),
            config_backup_path: Some(PathBuf::from(
                r"C:\Users\Example\AppData\Local\guardian\backups\codex-config-20260423-120000.toml.bak",
            )),
            missing_keys_before: vec![r"D:\Workspaces\Guardian Project".to_string()],
            added_keys: vec![r"D:\Workspaces\Guardian Project".to_string()],
            created_config: false,
        }
    }

    #[test]
    fn slow_path_error_preserves_trust_repair_and_surfaces_note() {
        // Simulates the soft-failure branch of `execute_confirmed` where
        // `apply_slow_path_repair` failed (e.g. no hotfix binary available on
        // the workstation) but stale-row and trust repair already landed. The
        // execution must still be returned so the audit can be persisted and
        // the CLI/GUI/tray surfaces show the actionable error.
        let mut execution = CodexRepairExecution {
            script_path: None,
            state_db_path: None,
            backup_path: None,
            stale_rows_before: None,
            stale_rows_after: None,
            old_sessions_before: None,
            old_sessions_after: None,
            old_session_archive_days: None,
            active_version: None,
            stdout_excerpt: Vec::new(),
            stderr_excerpt: Vec::new(),
            outcome: CodexRepairOutcome::Repaired,
            trust_repair: Some(sample_trust_repair()),
            slow_path_repair: None,
            slow_path_error: Some("unable to locate a verified Codex hotfix binary".to_string()),
        };
        // Trust repair succeeded so the overall outcome stays `Repaired`; the
        // skip is reported as an evidence/note and does not downgrade prior work.
        assert!(execution.is_successful());
        let summary = execution.outcome_summary();
        assert!(
            summary.contains("appended missing trusted project entries"),
            "summary should still list trust repair work: {summary}"
        );
        let notes = execution.notes();
        assert!(
            notes.iter().any(
                |note| note.contains("slow-path launcher hotfix was skipped")
                    && note.contains("unable to locate a verified Codex hotfix binary")
            ),
            "notes must surface the slow-path skip reason: {notes:?}"
        );

        // With no prior successful work, the same soft-failure flips the
        // outcome to Unresolved so the CLI exit code reflects the problem.
        execution.outcome = CodexRepairOutcome::Noop;
        execution.trust_repair = None;
        if execution.outcome == CodexRepairOutcome::Noop {
            execution.outcome = CodexRepairOutcome::Unresolved;
        }
        assert!(!execution.is_successful());
        assert!(
            execution
                .outcome_summary()
                .contains("slow-path launcher hotfix step was skipped")
        );
    }

    #[test]
    fn source_candidates_prioritize_vendor_hotfix_before_temp_build() {
        let candidates = hotfix_source_candidates(
            Path::new(r"C:\Users\Example\AppData\Roaming\npm\node_modules\@openai\codex"),
            Path::new(r"C:\Users\Example\AppData\Local\Temp"),
            Some(Path::new(
                r"D:\Workspaces\Guardian Project\dist\v0.1.0\vendor-hotfix",
            )),
            "x86_64-pc-windows-msvc",
            "codex.exe",
        );

        assert_eq!(
            candidates,
            vec![
                PathBuf::from(
                    r"C:\Users\Example\AppData\Roaming\npm\node_modules\@openai\codex\vendor-hotfix\x86_64-pc-windows-msvc\codex\codex.exe"
                ),
                PathBuf::from(
                    r"D:\Workspaces\Guardian Project\dist\v0.1.0\vendor-hotfix\x86_64-pc-windows-msvc\codex\codex.exe"
                ),
                PathBuf::from(
                    r"C:\Users\Example\AppData\Local\Temp\codex-src\codex-rs\target\release\codex.exe"
                ),
            ]
        );
    }

    #[test]
    fn repair_script_bootstrap_injects_cmd_based_version_probe() {
        let bootstrap = render_repair_script_bootstrap(
            Path::new(r"C:\Users\Example\.codex\tools\repair-codex-resume.ps1"),
            Path::new(r"C:\Users\Example\.codex"),
            Path::new(r"C:\Users\Example\.codex\state_5.sqlite"),
            Some("0.124.0"),
            Some(Path::new(r"C:\Users\Example\AppData\Roaming\npm\codex.cmd")),
        );

        assert!(bootstrap.contains("function codex"));
        assert!(bootstrap.contains("& cmd /d /c"));
        assert!(bootstrap.contains("codex.cmd\" --version 2>nul"));
        assert!(bootstrap.contains("-TargetVersion '0.124.0'"));
    }

    #[test]
    fn embedded_repair_script_matches_runtime_contract() {
        assert!(
            EMBEDDED_REPAIR_SCRIPT.starts_with("[CmdletBinding()]"),
            "embedded script must begin with the PowerShell binding header"
        );
        assert!(
            EMBEDDED_REPAIR_SCRIPT.contains("[codex-resume-repair]"),
            "embedded script must emit the audited prefix Guardian parses"
        );
        assert!(
            EMBEDDED_REPAIR_SCRIPT.contains("$StateDbPath"),
            "embedded script must accept the StateDbPath parameter"
        );
    }

    #[test]
    fn ensure_repair_script_installed_writes_when_missing() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let codex_home = temp.path();

        let path = ensure_repair_script_installed(codex_home).expect("first install");

        assert_eq!(
            path,
            codex_home.join("tools").join("repair-codex-resume.ps1")
        );
        let written = std::fs::read(&path).expect("read written script");
        assert_eq!(written, EMBEDDED_REPAIR_SCRIPT.as_bytes());
    }

    #[test]
    fn ensure_repair_script_installed_preserves_existing_content() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let codex_home = temp.path();
        let custom_payload = b"# operator-customized repair script\n";
        let tools_dir = codex_home.join("tools");
        std::fs::create_dir_all(&tools_dir).expect("seed tools dir");
        std::fs::write(tools_dir.join("repair-codex-resume.ps1"), custom_payload)
            .expect("seed custom script");

        let path = ensure_repair_script_installed(codex_home).expect("idempotent install");

        let preserved = std::fs::read(&path).expect("read preserved script");
        assert_eq!(
            preserved, custom_payload,
            "existing operator-owned script must not be overwritten"
        );
    }
}
