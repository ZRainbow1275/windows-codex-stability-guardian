use std::{
    fs::{self, File},
    io::SeekFrom,
    io::{BufRead, BufReader, Read, Seek},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use guardian_core::{
    GuardianError,
    policy::FailureClass,
    types::{DomainReport, EvidenceItem, StatusLevel},
};
use guardian_windows::{
    codex_config::{codex_config_path, missing_project_trust_keys},
    paths::{
        codex_home_dir, codex_state_db_candidates, codex_tui_log_candidates, latest_codex_state_db,
    },
    process::{CommandOutput, run_command_with_cmd_fallback},
};
use rusqlite::Connection;

const SESSION_ARCHIVE_GRACE_DAYS: i64 = 30;
const RESUME_STUCK_THRESHOLD: Duration = Duration::from_secs(60);

pub fn observe() -> Result<DomainReport, GuardianError> {
    observe_with_target(None)
}

pub fn observe_with_target(project_path: Option<&Path>) -> Result<DomainReport, GuardianError> {
    let mut evidence = Vec::new();
    let mut notes = Vec::new();
    let mut status = StatusLevel::Ok;
    let mut failure_classes = Vec::new();

    let codex_home = codex_home_dir().map_err(GuardianError::Io)?;
    evidence.push(EvidenceItem::new(
        "codex_home",
        codex_home.display().to_string(),
    ));

    if !codex_home.exists() {
        return Ok(DomainReport::new(
            StatusLevel::Fail,
            "Codex home directory is missing, so no local Codex evidence could be collected.",
            evidence,
            vec!["Expected `%USERPROFILE%/.codex` to exist on this Windows machine.".to_string()],
        ));
    }

    match command_output("codex", ["--version"]) {
        Ok(output) => {
            if is_known_risky_version(&output) {
                status = StatusLevel::Warn;
                failure_classes.push(FailureClass::C3);
                notes.push(format!(
                    "Codex version `{output}` matches a known picker-risk window."
                ));
            }
            evidence.push(EvidenceItem::new("codex_version", output));
        }
        Err(error) => {
            status = StatusLevel::Warn;
            notes.push(format!("Unable to execute `codex --version`: {error}"));
        }
    }

    let history_path = codex_home.join("history.jsonl");
    if history_path.exists() {
        let line_count = count_lines(&history_path)?;
        evidence.push(EvidenceItem::new("history_lines", line_count.to_string()));
    } else {
        status = StatusLevel::Warn;
        notes.push("`history.jsonl` is missing.".to_string());
    }

    let sessions_root = codex_home.join("sessions");
    let session_count = count_session_files(&sessions_root)?;
    evidence.push(EvidenceItem::new(
        "session_files",
        session_count.to_string(),
    ));
    if session_count == 0 {
        status = StatusLevel::Warn;
        notes.push("No session files were found under `.codex/sessions`.".to_string());
    }

    let repair_script = codex_home.join("tools").join("repair-codex-resume.ps1");
    evidence.push(EvidenceItem::new(
        "repair_script_present",
        repair_script.exists().to_string(),
    ));

    let state_files = codex_state_db_candidates(&codex_home).map_err(GuardianError::Io)?;
    evidence.push(EvidenceItem::new(
        "state_files",
        state_files.len().to_string(),
    ));

    if let Some(latest_state) = latest_codex_state_db(&codex_home).map_err(GuardianError::Io)? {
        evidence.push(EvidenceItem::new(
            "latest_state_file",
            latest_state.display().to_string(),
        ));
        match inspect_state_db(&latest_state) {
            Ok(stats) => {
                evidence.push(EvidenceItem::new(
                    "threads_total",
                    stats.thread_count.to_string(),
                ));
                evidence.push(EvidenceItem::new(
                    "stale_rows",
                    stats.stale_rows.to_string(),
                ));
                evidence.push(EvidenceItem::new(
                    "session_archive_grace_days",
                    SESSION_ARCHIVE_GRACE_DAYS.to_string(),
                ));
                evidence.push(EvidenceItem::new(
                    "old_unarchived_threads",
                    stats.old_unarchived_threads.to_string(),
                ));
                if stats.stale_rows > 0 {
                    status = StatusLevel::Warn;
                    failure_classes.push(FailureClass::C2);
                    notes.push(
                        "Detected stale rows in the latest Codex state database.".to_string(),
                    );
                }
                if stats.old_unarchived_threads > 0 {
                    status = StatusLevel::Warn;
                    failure_classes.push(FailureClass::C4);
                    notes.push(format!(
                        "Detected {} Codex thread(s) older than {} days that are still unarchived; they can be archived safely and may contribute to `/resume` slow-path.",
                        stats.old_unarchived_threads,
                        SESSION_ARCHIVE_GRACE_DAYS
                    ));
                }
            }
            Err(error) => {
                status = StatusLevel::Warn;
                notes.push(format!(
                    "Unable to inspect the latest state database `{}`: {error}",
                    latest_state.display()
                ));
            }
        }
    } else {
        status = StatusLevel::Warn;
        notes.push("No `state_*.sqlite` file was found under `.codex`.".to_string());
    }

    match collect_codex_resume_process_signals() {
        Ok(process_signals) => {
            evidence.push(EvidenceItem::new(
                "resume_process_count",
                process_signals.observations.len().to_string(),
            ));
            evidence.push(EvidenceItem::new(
                "resume_stuck_threshold_seconds",
                RESUME_STUCK_THRESHOLD.as_secs().to_string(),
            ));
            if !process_signals.observations.is_empty() {
                evidence.push(EvidenceItem::new(
                    "resume_process_oldest_age_seconds",
                    process_signals.oldest_age_seconds().to_string(),
                ));
                evidence.push(EvidenceItem::new(
                    "resume_process_pids",
                    process_signals.pid_summary(),
                ));
            }
            let stuck_count = process_signals.stuck_count();
            if stuck_count > 0 {
                status = StatusLevel::Warn;
                failure_classes.push(FailureClass::C4);
                notes.push(format!(
                    "Detected {stuck_count} long-running `codex resume` process(es); oldest age is {} seconds, which matches the `/resume` slow-path classifier.",
                    process_signals.oldest_age_seconds()
                ));
            }
        }
        Err(error) => {
            notes.push(format!(
                "Unable to inspect running `codex resume` processes: {error}"
            ));
        }
    }

    let log_signals = collect_codex_log_signals()?;
    match &log_signals {
        Some(log_signals) => {
            evidence.push(EvidenceItem::new(
                "codex_tui_log_path",
                log_signals.path.display().to_string(),
            ));
            evidence.push(EvidenceItem::new(
                "codex_tui_signal_count",
                log_signals.matches.len().to_string(),
            ));
            if !log_signals.matches.is_empty() {
                evidence.push(EvidenceItem::new(
                    "codex_tui_matches",
                    log_signals.matches.join(" | "),
                ));
            }
            if log_signals.has_loading_sessions {
                status = StatusLevel::Warn;
                failure_classes.push(FailureClass::C4);
                notes.push(
                    "Recent Codex TUI log lines include `Loading sessions`, which matches the slow-path classifier."
                        .to_string(),
                );
            }
            if log_signals.has_config_error {
                status = StatusLevel::Warn;
                failure_classes.push(FailureClass::C5);
                notes.push(
                    "Recent Codex TUI log lines include configuration/access errors.".to_string(),
                );
            }
        }
        None => {
            evidence.push(EvidenceItem::new("codex_tui_log_present", "false"));
            notes.push(
                "No `codex-tui.log` was found under the expected `.codex/log` or `.codex` locations."
                    .to_string(),
            );
        }
    }

    let config_path = codex_config_path().map_err(GuardianError::Io)?;
    evidence.push(EvidenceItem::new(
        "codex_config_path",
        config_path.display().to_string(),
    ));
    evidence.push(EvidenceItem::new(
        "codex_config_present",
        config_path.exists().to_string(),
    ));

    if let Some(trust_target) = resolve_trust_target(project_path, log_signals.as_ref()) {
        evidence.push(EvidenceItem::new(
            "trust_target_path",
            trust_target.path.display().to_string(),
        ));
        evidence.push(EvidenceItem::new(
            "trust_target_source",
            trust_target.source,
        ));

        let config_text = if config_path.exists() {
            Some(std::fs::read_to_string(&config_path)?)
        } else {
            None
        };
        let config_text = config_text.as_deref().unwrap_or("");
        match missing_project_trust_keys(config_text, &trust_target.path) {
            Ok(missing_keys) => {
                if !missing_keys.is_empty() {
                    status = StatusLevel::Warn;
                    failure_classes.push(FailureClass::C6);
                    evidence.push(EvidenceItem::new(
                        "trust_missing_lookup_keys",
                        missing_keys.join(" | "),
                    ));
                    notes.push(format!(
                        "Codex project trust is missing for `{}`; Guardian found {} expected lookup key(s) absent from `config.toml`.",
                        trust_target.path.display(),
                        missing_keys.len()
                    ));
                }
            }
            Err(error) => {
                status = StatusLevel::Warn;
                failure_classes.push(FailureClass::C5);
                notes.push(format!(
                    "Unable to parse `%USERPROFILE%/.codex/config.toml` while checking project trust: {error}"
                ));
            }
        }
    }

    push_failure_classes(&mut evidence, &mut failure_classes);

    let summary = format!(
        "Collected live Codex evidence from `{}` with {} session file(s) and {} failure classifier(s).",
        codex_home.display(),
        session_count,
        failure_classes.len()
    );

    Ok(DomainReport::new(status, summary, evidence, notes))
}

fn command_output<I, S>(program: &str, args: I) -> Result<String, GuardianError>
where
    I: IntoIterator<Item = S>,
    S: Into<std::ffi::OsString>,
{
    let args_vec: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let output = run_command_with_cmd_fallback(program, &args_vec).map_err(GuardianError::Io)?;
    command_output_to_string(program, &args_vec, output)
}

fn count_lines(path: &Path) -> Result<usize, GuardianError> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut count = 0usize;
    for line in reader.lines() {
        line?;
        count += 1;
    }
    Ok(count)
}

fn count_session_files(root: &Path) -> Result<usize, GuardianError> {
    if !root.exists() {
        return Ok(0);
    }

    let mut count = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }

            if path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") {
                count += 1;
            }
        }
    }

    Ok(count)
}

fn inspect_state_db(path: &Path) -> Result<CodexStateDbStats, GuardianError> {
    let connection = Connection::open(path)
        .map_err(|error| GuardianError::invalid_state(format!("sqlite open failed: {error}")))?;
    let thread_count: i64 = connection
        .query_row("select count(*) from threads", [], |row| row.get(0))
        .map_err(|error| GuardianError::invalid_state(format!("threads count failed: {error}")))?;
    let stale_rows: i64 = connection
        .query_row(
            "select count(*) from threads where has_user_event = 0 and trim(coalesce(first_user_message, '')) <> ''",
            [],
            |row| row.get(0),
        )
        .map_err(|error| GuardianError::invalid_state(format!("stale row query failed: {error}")))?;
    let archive_cutoff = archive_cutoff_epoch(SESSION_ARCHIVE_GRACE_DAYS);
    let old_unarchived_threads: i64 = connection
        .query_row(
            "select count(*) from threads where archived = 0 and created_at < ?1",
            [archive_cutoff],
            |row| row.get(0),
        )
        .map_err(|error| {
            GuardianError::invalid_state(format!("old unarchived thread query failed: {error}"))
        })?;
    Ok(CodexStateDbStats {
        thread_count,
        stale_rows,
        old_unarchived_threads,
    })
}

fn archive_cutoff_epoch(days: i64) -> i64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    now.saturating_sub(days.saturating_mul(86_400))
}

fn collect_codex_resume_process_signals() -> Result<CodexResumeProcessSignals, GuardianError> {
    if !cfg!(target_os = "windows") {
        return Ok(CodexResumeProcessSignals::default());
    }

    let output = command_output(
        "powershell",
        ["-NoProfile", "-Command", RESUME_PROCESS_PROBE],
    )?;
    Ok(parse_resume_process_signals(&output))
}

const RESUME_PROCESS_PROBE: &str = r#"
$now = Get-Date
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
    Where-Object {
        $_.CommandLine -and
        ($_.Name -eq "codex.exe" -or $_.Name -eq "node.exe") -and
        ($_.CommandLine -match "(?i)codex(\.exe|\.js)?|@openai[\\/]+codex") -and
        ($_.CommandLine -match "(?i)(^|\s)resume(\s|$)")
    } |
    Sort-Object CreationDate |
    ForEach-Object {
        $ageSeconds = [int][Math]::Floor(($now - $_.CreationDate).TotalSeconds)
        $commandLine = ($_.CommandLine -replace "[\r\n\t]+", " ").Trim()
        "{0}`t{1}`t{2}" -f $_.ProcessId, $ageSeconds, $commandLine
    }
"#;

fn parse_resume_process_signals(output: &str) -> CodexResumeProcessSignals {
    let observations = output
        .lines()
        .filter_map(parse_resume_process_observation)
        .collect();
    CodexResumeProcessSignals { observations }
}

fn parse_resume_process_observation(line: &str) -> Option<CodexResumeProcessObservation> {
    let mut parts = line.splitn(3, '\t');
    let pid = parts.next()?.trim().parse().ok()?;
    let age_seconds = parts.next()?.trim().parse().ok()?;
    let command_line = parts.next()?.trim().to_string();
    if command_line.is_empty() {
        return None;
    }

    Some(CodexResumeProcessObservation {
        pid,
        age_seconds,
        command_line,
    })
}

fn command_output_to_string(
    program: &str,
    args: &[std::ffi::OsString],
    output: CommandOutput,
) -> Result<String, GuardianError> {
    if output.success() {
        Ok(output.stdout)
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
            status: output.status,
            stderr: if output.stderr.is_empty() {
                output.stdout
            } else {
                output.stderr
            },
        })
    }
}

fn is_known_risky_version(version: &str) -> bool {
    version.contains("0.120.0") || version.contains("0.104.0")
}

fn collect_codex_log_signals() -> Result<Option<CodexLogSignals>, GuardianError> {
    let Some(path) = codex_tui_log_candidates()
        .map_err(GuardianError::Io)?
        .into_iter()
        .find(|candidate| candidate.exists())
    else {
        return Ok(None);
    };

    let tail = read_tail(&path, 256 * 1024)?;
    let mut matches = Vec::new();
    let mut has_loading_sessions = false;
    let mut has_config_error = false;
    let mut trust_project_path = None;
    for line in tail.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if !is_codex_log_signal(trimmed) {
            continue;
        }

        has_loading_sessions |= is_loading_sessions_signal(trimmed);
        has_config_error |= is_config_error_signal(trimmed);
        if let Some(path) = extract_trust_warning_path(trimmed) {
            trust_project_path = Some(path);
        }

        matches.push(trim_log_line(trimmed));
    }

    let matches = last_unique(matches, 6);

    Ok(Some(CodexLogSignals {
        path,
        matches,
        has_loading_sessions,
        has_config_error,
        trust_project_path,
    }))
}

fn read_tail(path: &Path, max_bytes: u64) -> Result<String, GuardianError> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;

    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let mut text = String::from_utf8_lossy(&buffer).replace('\0', "");
    if start > 0
        && let Some(index) = text.find('\n')
    {
        text = text[index + 1..].to_string();
    }
    Ok(text)
}

fn is_codex_log_signal(line: &str) -> bool {
    if looks_like_tooling_echo(line) {
        return false;
    }

    is_loading_sessions_signal(line)
        || is_config_error_signal(line)
        || is_trust_warning_signal(line)
}

fn looks_like_tooling_echo(line: &str) -> bool {
    line.starts_with('+')
        || line.starts_with('-')
        || line.contains("MCP server stderr")
        || line.contains("MCP server stdout")
        || line.contains("ToolCall:")
        || line.contains("exec_command")
        || line.contains("apply_patch")
        || line.contains("Select-String")
        || line.contains("line.contains(")
        || line.contains("trimmed.contains(")
}

fn is_loading_sessions_signal(line: &str) -> bool {
    line.contains("Loading sessions...") || line.contains("Loading sessions…")
}

fn is_config_error_signal(line: &str) -> bool {
    line.contains("Error loading configuration:")
        || line.contains("os error 5")
        || line.contains("拒绝访问。 (os error 5)")
}

fn is_trust_warning_signal(line: &str) -> bool {
    line.contains("as a trusted project")
        && line.contains("config.toml")
        && line.contains("project-local config")
}

fn extract_trust_warning_path(line: &str) -> Option<PathBuf> {
    let start = line.find(" add ")? + " add ".len();
    let end = line.find(" as a trusted project")?;
    let candidate = line[start..end].trim();
    if candidate.is_empty() {
        return None;
    }
    Some(PathBuf::from(candidate))
}

fn trim_log_line(line: &str) -> String {
    const LIMIT: usize = 200;
    if line.chars().count() <= LIMIT {
        line.to_string()
    } else {
        format!("{}...", line.chars().take(LIMIT).collect::<String>())
    }
}

fn last_unique(lines: Vec<String>, limit: usize) -> Vec<String> {
    let mut reversed_unique = Vec::new();
    for line in lines.into_iter().rev() {
        if !reversed_unique.contains(&line) {
            reversed_unique.push(line);
        }
        if reversed_unique.len() == limit {
            break;
        }
    }
    reversed_unique.into_iter().rev().collect()
}

fn push_failure_classes(evidence: &mut Vec<EvidenceItem>, failure_classes: &mut Vec<FailureClass>) {
    failure_classes.sort_by_key(|class| class.as_str());
    failure_classes.dedup_by_key(|class| class.as_str());
    evidence.push(EvidenceItem::new(
        "failure_classes",
        if failure_classes.is_empty() {
            "none".to_string()
        } else {
            failure_classes
                .iter()
                .map(|class| class.as_str())
                .collect::<Vec<_>>()
                .join(",")
        },
    ));
}

struct CodexLogSignals {
    path: PathBuf,
    matches: Vec<String>,
    has_loading_sessions: bool,
    has_config_error: bool,
    trust_project_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct CodexStateDbStats {
    thread_count: i64,
    stale_rows: i64,
    old_unarchived_threads: i64,
}

#[derive(Debug, Clone, Default)]
struct CodexResumeProcessSignals {
    observations: Vec<CodexResumeProcessObservation>,
}

impl CodexResumeProcessSignals {
    fn stuck_count(&self) -> usize {
        self.observations
            .iter()
            .filter(|process| process.age_seconds >= RESUME_STUCK_THRESHOLD.as_secs())
            .count()
    }

    fn oldest_age_seconds(&self) -> u64 {
        self.observations
            .iter()
            .map(|process| process.age_seconds)
            .max()
            .unwrap_or_default()
    }

    fn pid_summary(&self) -> String {
        self.observations
            .iter()
            .take(6)
            .map(|process| {
                format!(
                    "{}:{}s:{}",
                    process.pid,
                    process.age_seconds,
                    trim_log_line(&process.command_line)
                )
            })
            .collect::<Vec<_>>()
            .join(" | ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodexResumeProcessObservation {
    pid: u32,
    age_seconds: u64,
    command_line: String,
}

#[derive(Debug, Clone)]
struct TrustTarget {
    path: PathBuf,
    source: &'static str,
}

fn resolve_trust_target(
    project_path: Option<&Path>,
    log_signals: Option<&CodexLogSignals>,
) -> Option<TrustTarget> {
    if let Some(project_path) = project_path {
        return Some(TrustTarget {
            path: project_path.to_path_buf(),
            source: "requested_project_path",
        });
    }

    log_signals
        .and_then(|signals| signals.trust_project_path.as_ref())
        .map(|path| TrustTarget {
            path: path.clone(),
            source: "codex_tui_warning",
        })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        CodexResumeProcessObservation, extract_trust_warning_path, is_codex_log_signal,
        is_trust_warning_signal, last_unique, parse_resume_process_signals,
    };

    #[test]
    fn trust_warning_signal_extracts_project_path() {
        let line = "To load project-local config, hooks, and exec policies, add d:\\workspaces\\inkforge as a trusted project in C:\\Users\\Example\\.codex\\config.toml.";
        assert!(is_trust_warning_signal(line));
        assert_eq!(
            extract_trust_warning_path(line),
            Some(PathBuf::from(r"d:\workspaces\inkforge"))
        );
    }

    #[test]
    fn trust_warning_parser_ignores_unrelated_lines() {
        assert!(!is_trust_warning_signal("Loading sessions..."));
        assert_eq!(extract_trust_warning_path("plain text"), None);
    }

    #[test]
    fn last_unique_keeps_latest_occurrence_order() {
        let lines = vec![
            "a".to_string(),
            "b".to_string(),
            "a".to_string(),
            "c".to_string(),
        ];
        assert_eq!(
            last_unique(lines, 3),
            vec!["b".to_string(), "a".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn loading_sessions_from_mcp_tool_output_is_not_a_log_signal() {
        let line = "2026-05-14T13:56:46.830415Z  INFO codex_rmcp_client::stdio_server_launcher: MCP server stderr (cmd): │ true TUI 会长时间停在 Loading sessions... 超过 1 分钟 │";
        assert!(!is_codex_log_signal(line));
    }

    #[test]
    fn parses_resume_process_observations() {
        let output = "16092\t4210\tC:\\Users\\Example\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\node_modules\\@openai\\codex-win32-x64\\vendor\\x86_64-pc-windows-msvc\\codex\\codex.exe resume\n";
        let signals = parse_resume_process_signals(output);
        assert_eq!(signals.observations.len(), 1);
        assert_eq!(
            signals.observations[0],
            CodexResumeProcessObservation {
                pid: 16092,
                age_seconds: 4210,
                command_line: "C:\\Users\\Example\\AppData\\Roaming\\npm\\node_modules\\@openai\\codex\\node_modules\\@openai\\codex-win32-x64\\vendor\\x86_64-pc-windows-msvc\\codex\\codex.exe resume".to_string()
            }
        );
        assert_eq!(signals.stuck_count(), 1);
    }
}
