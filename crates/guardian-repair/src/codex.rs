use std::{
    env,
    ffi::OsString,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};

use chrono::Local;
use guardian_core::{GuardianError, types::ActionPlan};
use guardian_observers::codex as codex_observer;
use guardian_windows::{
    codex_config::{append_trusted_project_entries, codex_config_path, missing_project_trust_keys},
    paths::{codex_home_dir, guardian_backup_dir, latest_codex_state_db},
    process::run_command_with_cmd_fallback,
};
use rusqlite::Connection;

const SCRIPT_OUTPUT_LIMIT: usize = 8;
const REPAIR_PREFIX: &str = "[codex-resume-repair]";
const BACKUP_PREFIX: &str = "SQLite backup:";
const ACTIVE_VERSION_PREFIX: &str = "Repair complete. Active version:";

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
    pub active_version: Option<String>,
    pub stdout_excerpt: Vec<String>,
    pub stderr_excerpt: Vec<String>,
    pub outcome: CodexRepairOutcome,
    pub trust_repair: Option<CodexTrustRepair>,
    pub slow_path_repair: Option<CodexSlowPathRepair>,
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
        let mut repaired_steps = Vec::new();
        if stale_repaired {
            repaired_steps.push("cleared stale rows");
        }
        if trust_added {
            repaired_steps.push("appended missing trusted project entries");
        }
        if slow_path_repaired {
            repaired_steps.push("staged the Codex slow-path launcher hotfix");
        }

        match self.outcome {
            CodexRepairOutcome::Noop => {
                "Codex repair confirm completed without changing stale rows, trust entries, or slow-path launcher state."
                    .to_string()
            }
            CodexRepairOutcome::Repaired => {
                if repaired_steps.is_empty() {
                    "Codex repair confirm completed without changing stale rows, trust entries, or slow-path launcher state."
                        .to_string()
                } else {
                    format!("Codex repair confirm {}.", repaired_steps.join(", "))
                }
            }
            CodexRepairOutcome::Unresolved => {
                if repaired_steps.is_empty() {
                    "Codex repair confirm executed, but stale rows still remain after verification."
                        .to_string()
                } else {
                    format!(
                        "Codex repair confirm {}, but stale rows still remain after verification.",
                        repaired_steps.join(", ")
                    )
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
            "Preview the Codex repair chain, including trust recovery and slow-path launcher staging when those drifts are identified."
                .to_string(),
            false,
        ),
        ActionPlan::new(
            confirm,
            "Execute the managed Codex repair chain with backup, verification, audit, and controlled slow-path launcher hotfix staging."
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
    let repair_slow_path =
        domain_has_failure_class(&observer_report, "C4") || slow_path_repair_required();
    let trust_target = trust_target_from_report(&observer_report);

    let mut execution = CodexRepairExecution {
        script_path: None,
        state_db_path: None,
        backup_path: None,
        stale_rows_before: None,
        stale_rows_after: None,
        active_version: None,
        stdout_excerpt: Vec::new(),
        stderr_excerpt: Vec::new(),
        outcome: CodexRepairOutcome::Noop,
        trust_repair: None,
        slow_path_repair: None,
    };

    if repair_stale_rows {
        let codex_home = codex_home_dir().map_err(GuardianError::Io)?;
        let script_path = repair_script_path(&codex_home);
        if !script_path.exists() {
            return Err(GuardianError::invalid_state(format!(
                "trusted repair script is missing: {}",
                script_path.display()
            )));
        }

        let state_db_path = latest_codex_state_db(&codex_home)
            .map_err(GuardianError::Io)?
            .ok_or_else(|| {
                GuardianError::invalid_state(format!(
                    "expected a `state_*.sqlite` database under `{}` but none was found",
                    codex_home.display()
                ))
            })?;

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
        execution.state_db_path = Some(state_db_path);
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

    if let Some(target) = trust_target {
        let trust_repair = apply_project_trust_repair(&target)?;
        if !trust_repair.added_keys.is_empty() {
            execution.outcome = CodexRepairOutcome::Repaired;
        }
        execution.trust_repair = Some(trust_repair);
    }

    if repair_slow_path {
        let slow_path_repair = apply_slow_path_repair()?;
        if slow_path_repair.launcher_updated || slow_path_repair.hotfix_binary_updated {
            execution.outcome = CodexRepairOutcome::Repaired;
        }
        execution.slow_path_repair = Some(slow_path_repair);
    }

    Ok(execution)
}

fn repair_script_path(codex_home: &Path) -> PathBuf {
    codex_home.join("tools").join("repair-codex-resume.ps1")
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

fn run_repair_script(
    script_path: &Path,
    codex_home: &Path,
    state_db_path: &Path,
    target_version: Option<&str>,
) -> Result<ProcessOutput, GuardianError> {
    let mut args = vec![
        OsString::from("-NoProfile"),
        OsString::from("-ExecutionPolicy"),
        OsString::from("Bypass"),
        OsString::from("-File"),
        script_path.as_os_str().to_os_string(),
        OsString::from("-CodexHome"),
        codex_home.as_os_str().to_os_string(),
        OsString::from("-StateDbPath"),
        state_db_path.as_os_str().to_os_string(),
        OsString::from("-SkipInstall"),
    ];

    if let Some(target_version) = target_version {
        args.push(OsString::from("-TargetVersion"));
        args.push(OsString::from(target_version));
    }

    run_command("powershell", &args)
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
    let hotfix_source_path =
        find_hotfix_source_binary(&package_root, target_triple)?.ok_or_else(|| {
            GuardianError::invalid_state(format!(
                "unable to locate a verified Codex hotfix binary; checked `{}` and `{}`",
                hotfix_binary_path.display(),
                temp_hotfix_build_path().display()
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
    target_triple: &str,
    binary_name: &str,
) -> Vec<PathBuf> {
    vec![
        package_root
            .join("vendor-hotfix")
            .join(target_triple)
            .join("codex")
            .join(binary_name),
        temp_root
            .join("codex-src")
            .join("codex-rs")
            .join("target")
            .join("release")
            .join(binary_name),
    ]
}

fn temp_hotfix_build_path() -> PathBuf {
    env::temp_dir()
        .join("codex-src")
        .join("codex-rs")
        .join("target")
        .join("release")
        .join(codex_binary_name())
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

fn run_command(program: &str, args: &[OsString]) -> Result<ProcessOutput, GuardianError> {
    let output = match Command::new(program).args(args).output() {
        Ok(output) => output,
        Err(error) if error.kind() == ErrorKind::NotFound && cfg!(target_os = "windows") => {
            Command::new("cmd")
                .arg("/C")
                .arg(program)
                .args(args)
                .output()?
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
        ACTIVE_VERSION_PREFIX, BACKUP_PREFIX, REPAIR_PREFIX, active_version_from_output,
        backup_path_from_output, command_with_project_path, ensure_hotfix_launcher_patch,
        hotfix_source_candidates, normalize_repair_prefix, parse_semver_fragment,
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
                Some(Path::new(r"D:\Desktop\CREATOR SIX")),
            ),
            r#"guardian repair codex --confirm --project-path "D:\Desktop\CREATOR SIX""#
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

    #[test]
    fn source_candidates_prioritize_vendor_hotfix_before_temp_build() {
        let candidates = hotfix_source_candidates(
            Path::new(r"C:\Users\HP\AppData\Roaming\npm\node_modules\@openai\codex"),
            Path::new(r"C:\Users\HP\AppData\Local\Temp"),
            "x86_64-pc-windows-msvc",
            "codex.exe",
        );

        assert_eq!(
            candidates,
            vec![
                PathBuf::from(
                    r"C:\Users\HP\AppData\Roaming\npm\node_modules\@openai\codex\vendor-hotfix\x86_64-pc-windows-msvc\codex\codex.exe"
                ),
                PathBuf::from(
                    r"C:\Users\HP\AppData\Local\Temp\codex-src\codex-rs\target\release\codex.exe"
                ),
            ]
        );
    }
}
