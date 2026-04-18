use std::{
    ffi::OsString,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
};

use guardian_core::{GuardianError, types::ActionPlan};
use guardian_windows::{
    paths::{codex_home_dir, latest_codex_state_db},
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
    pub script_path: PathBuf,
    pub state_db_path: PathBuf,
    pub backup_path: Option<PathBuf>,
    pub stale_rows_before: i64,
    pub stale_rows_after: i64,
    pub active_version: Option<String>,
    pub stdout_excerpt: Vec<String>,
    pub stderr_excerpt: Vec<String>,
    pub outcome: CodexRepairOutcome,
}

impl CodexRepairExecution {
    pub fn is_successful(&self) -> bool {
        self.outcome != CodexRepairOutcome::Unresolved
    }

    pub fn outcome_summary(&self) -> String {
        match self.outcome {
            CodexRepairOutcome::Noop => {
                "Codex repair confirm completed without changing stale rows.".to_string()
            }
            CodexRepairOutcome::Repaired => {
                "Codex stale-row repair completed and re-check reached zero stale rows.".to_string()
            }
            CodexRepairOutcome::Unresolved => {
                "Codex repair confirm executed, but stale rows still remain after verification."
                    .to_string()
            }
        }
    }

    pub fn notes(&self) -> Vec<String> {
        let mut notes = vec![format!(
            "Trusted script executed: {}",
            self.script_path.display()
        )];

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

pub fn planned_actions() -> Vec<ActionPlan> {
    vec![
        ActionPlan::new(
            "guardian repair codex --dry-run".to_string(),
            "Preview the Codex repair chain without mutating the environment.".to_string(),
            false,
        ),
        ActionPlan::new(
            "guardian repair codex --confirm".to_string(),
            "Execute the trusted Codex stale-row repair chain with backup, verification, and audit."
                .to_string(),
            true,
        ),
    ]
}

pub fn execute_confirmed() -> Result<CodexRepairExecution, GuardianError> {
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
    let active_version = active_version_from_output(&process_output.stdout);
    let backup_path = backup_path_from_output(&process_output.stdout);

    let outcome = if stale_rows_before == 0 && stale_rows_after == 0 {
        CodexRepairOutcome::Noop
    } else if stale_rows_before > 0 && stale_rows_after == 0 {
        CodexRepairOutcome::Repaired
    } else {
        CodexRepairOutcome::Unresolved
    };

    Ok(CodexRepairExecution {
        script_path,
        state_db_path,
        backup_path,
        stale_rows_before,
        stale_rows_after,
        active_version,
        stdout_excerpt: excerpt_lines(&process_output.stdout),
        stderr_excerpt: excerpt_lines(&process_output.stderr),
        outcome,
    })
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
        backup_path_from_output, normalize_repair_prefix, parse_semver_fragment,
    };

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
}
