use std::{ffi::OsString, fs};

use guardian_core::{
    GuardianError,
    policy::FailureClass,
    types::{DomainReport, EvidenceItem, StatusLevel},
};
use guardian_windows::{paths::wslconfig_path, process::run_command_with_cmd_fallback};

pub fn observe() -> Result<DomainReport, GuardianError> {
    let mut evidence = Vec::new();
    let mut notes = Vec::new();
    let mut status = StatusLevel::Ok;
    let mut failure_classes = Vec::new();

    match command_output("docker", ["version", "--format", "{{json .}}"]) {
        Ok(output) => evidence.push(EvidenceItem::new("docker_version", truncate(&output))),
        Err(error) => {
            status = StatusLevel::Fail;
            failure_classes.push(FailureClass::D1);
            notes.push(format!("Unable to collect `docker version`: {error}"));
        }
    }

    match command_output("docker", ["info", "--format", "{{json .}}"]) {
        Ok(output) => evidence.push(EvidenceItem::new("docker_info", truncate(&output))),
        Err(error) => {
            status = StatusLevel::Fail;
            failure_classes.push(FailureClass::D4);
            notes.push(format!("Unable to collect `docker info`: {error}"));
        }
    }

    match command_output("wsl", ["-l", "-v"]) {
        Ok(output) => {
            evidence.push(EvidenceItem::new("wsl_list", flatten_lines(&output)));
            if !output.contains("docker-desktop") {
                status = status.max(StatusLevel::Warn);
                failure_classes.push(FailureClass::D2);
                notes.push(
                    "`wsl -l -v` did not list the `docker-desktop` distro, which matches the utility VM anomaly classifier."
                        .to_string(),
                );
            }
        }
        Err(error) => {
            status = StatusLevel::Fail;
            failure_classes.push(FailureClass::D2);
            notes.push(format!("Unable to collect `wsl -l -v`: {error}"));
        }
    }

    let wslconfig = wslconfig_path().map_err(GuardianError::Io)?;
    evidence.push(EvidenceItem::new(
        "wslconfig_path",
        wslconfig.display().to_string(),
    ));
    evidence.push(EvidenceItem::new(
        "wslconfig_exists",
        wslconfig.exists().to_string(),
    ));

    if wslconfig.exists() {
        let contents = fs::read_to_string(&wslconfig)?;
        let analysis = analyze_wslconfig(&contents);

        evidence.push(EvidenceItem::new(
            "wslconfig_wsl2_has_memory",
            analysis.wsl2_has_memory.to_string(),
        ));
        evidence.push(EvidenceItem::new(
            "wslconfig_wsl2_has_processors",
            analysis.wsl2_has_processors.to_string(),
        ));
        evidence.push(EvidenceItem::new(
            "wslconfig_wsl2_has_swap",
            analysis.wsl2_has_swap.to_string(),
        ));
        evidence.push(EvidenceItem::new(
            "wslconfig_experimental_has_auto_memory_reclaim",
            analysis.experimental_has_auto_memory_reclaim.to_string(),
        ));

        if !analysis.has_recommended_baseline() {
            status = StatusLevel::Warn;
            failure_classes.push(FailureClass::D3);
            notes.push(
                "The current `.wslconfig` is missing at least one documented baseline key."
                    .to_string(),
            );
        }
    } else {
        status = StatusLevel::Warn;
        failure_classes.push(FailureClass::D3);
        notes.push(
            "`.wslconfig` does not exist yet, so no WSL resource baseline is configured."
                .to_string(),
        );
    }

    push_failure_classes(&mut evidence, &mut failure_classes);

    Ok(DomainReport::new(
        status,
        format!(
            "Collected live Docker, WSL, and `.wslconfig` evidence with {} failure classifier(s).",
            failure_classes.len()
        ),
        evidence,
        notes,
    ))
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

fn flatten_lines(contents: &str) -> String {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" | ")
}

fn truncate(value: &str) -> String {
    const LIMIT: usize = 240;
    if value.chars().count() <= LIMIT {
        value.to_string()
    } else {
        format!("{}...", value.chars().take(LIMIT).collect::<String>())
    }
}

pub fn analyze_wslconfig(contents: &str) -> WslConfigAnalysis {
    let mut analysis = WslConfigAnalysis::default();
    let mut section = String::new();

    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line.trim_matches(['[', ']']).to_ascii_lowercase();
            continue;
        }

        let Some((key, _value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        match section.as_str() {
            "wsl2" => match key.as_str() {
                "memory" => analysis.wsl2_has_memory = true,
                "processors" => analysis.wsl2_has_processors = true,
                "swap" => analysis.wsl2_has_swap = true,
                _ => {}
            },
            "experimental" => {
                if key == "automemoryreclaim" {
                    analysis.experimental_has_auto_memory_reclaim = true;
                }
            }
            _ => {}
        }
    }

    analysis
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

#[derive(Debug, Default)]
pub struct WslConfigAnalysis {
    pub wsl2_has_memory: bool,
    pub wsl2_has_processors: bool,
    pub wsl2_has_swap: bool,
    pub experimental_has_auto_memory_reclaim: bool,
}

impl WslConfigAnalysis {
    fn has_recommended_baseline(&self) -> bool {
        self.wsl2_has_memory
            && self.wsl2_has_processors
            && self.wsl2_has_swap
            && self.experimental_has_auto_memory_reclaim
    }
}

#[cfg(test)]
mod tests {
    use super::analyze_wslconfig;

    #[test]
    fn parses_section_scoped_wslconfig_keys() {
        let analysis = analyze_wslconfig(
            r#"
[wsl2]
memory=8GB
processors=6
swap=4GB

[experimental]
autoMemoryReclaim=gradual
"#,
        );

        assert!(analysis.has_recommended_baseline());
    }
}
