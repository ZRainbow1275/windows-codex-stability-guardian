use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLevel {
    Ok,
    Warn,
    Fail,
}

impl StatusLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub key: String,
    pub value: String,
}

impl EvidenceItem {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainReport {
    pub status: StatusLevel,
    pub summary: String,
    pub evidence: Vec<EvidenceItem>,
    pub notes: Vec<String>,
}

impl DomainReport {
    pub fn new(
        status: StatusLevel,
        summary: impl Into<String>,
        evidence: Vec<EvidenceItem>,
        notes: Vec<String>,
    ) -> Self {
        Self {
            status,
            summary: summary.into(),
            evidence,
            notes,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainReports {
    pub codex: DomainReport,
    pub docker_wsl: DomainReport,
    pub profile: DomainReport,
}

impl DomainReports {
    pub fn placeholder(status: StatusLevel, summary: String) -> Self {
        let report = DomainReport::new(status, summary, Vec::new(), Vec::new());
        Self {
            codex: report.clone(),
            docker_wsl: report.clone(),
            profile: report,
        }
    }

    pub fn single_codex(codex: DomainReport) -> Self {
        Self {
            codex,
            docker_wsl: skipped_domain("docker_wsl"),
            profile: skipped_domain("profile"),
        }
    }

    pub fn single_docker_wsl(docker_wsl: DomainReport) -> Self {
        Self {
            codex: skipped_domain("codex"),
            docker_wsl,
            profile: skipped_domain("profile"),
        }
    }

    pub fn single_profile(profile: DomainReport) -> Self {
        Self {
            codex: skipped_domain("codex"),
            docker_wsl: skipped_domain("docker_wsl"),
            profile,
        }
    }
}

fn skipped_domain(name: &str) -> DomainReport {
    DomainReport::new(
        StatusLevel::Ok,
        format!("Skipped `{name}` collection for this focused command."),
        Vec::new(),
        vec!["Cross-domain collection was skipped for this focused command.".to_string()],
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionPlan {
    pub command: String,
    pub description: String,
    pub requires_confirmation: bool,
}

impl ActionPlan {
    pub fn new(command: String, description: String, requires_confirmation: bool) -> Self {
        Self {
            command,
            description,
            requires_confirmation,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub status: StatusLevel,
    pub timestamp: String,
    pub domains: DomainReports,
    pub actions: Vec<ActionPlan>,
    pub notes: Vec<String>,
}

impl HealthReport {
    pub fn new(
        timestamp: String,
        domains: DomainReports,
        actions: Vec<ActionPlan>,
        notes: Vec<String>,
    ) -> Self {
        let status = [
            domains.codex.status,
            domains.docker_wsl.status,
            domains.profile.status,
        ]
        .into_iter()
        .max()
        .unwrap_or(StatusLevel::Ok);

        Self {
            status,
            timestamp,
            domains,
            actions,
            notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DomainReport, DomainReports, HealthReport, StatusLevel};

    #[test]
    fn overall_status_tracks_the_worst_domain() {
        let domains = DomainReports {
            codex: DomainReport::new(StatusLevel::Ok, "ok", Vec::new(), Vec::new()),
            docker_wsl: DomainReport::new(StatusLevel::Warn, "warn", Vec::new(), Vec::new()),
            profile: DomainReport::new(StatusLevel::Fail, "fail", Vec::new(), Vec::new()),
        };

        let report = HealthReport::new("now".to_string(), domains, Vec::new(), Vec::new());
        assert_eq!(report.status, StatusLevel::Fail);
    }
}
