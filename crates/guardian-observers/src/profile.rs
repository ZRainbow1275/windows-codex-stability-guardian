use guardian_core::{
    GuardianError,
    policy::FailureClass,
    types::{DomainReport, EvidenceItem, StatusLevel},
};
use guardian_windows::eventlog::{EventRecord, query_recent_events};

pub fn observe() -> Result<DomainReport, GuardianError> {
    let mut evidence = vec![
        EvidenceItem::new("collector_mode", "eventlog_read_only"),
        EvidenceItem::new("current_behavior", "read_only"),
    ];
    let mut notes = Vec::new();
    let mut failure_classes = Vec::new();

    let application_events = profile_events(
        "Application",
        Some("Provider[@Name='Microsoft-Windows-User Profiles Service']"),
    )?;
    let operational_events =
        profile_events("Microsoft-Windows-User Profile Service/Operational", None)?;

    evidence.push(EvidenceItem::new(
        "application_event_count",
        application_events.len().to_string(),
    ));
    evidence.push(EvidenceItem::new(
        "operational_event_count",
        operational_events.len().to_string(),
    ));

    let mut status = StatusLevel::Ok;
    let combined = merge_events(&application_events, &operational_events);
    if let Some(latest) = combined.first() {
        evidence.push(EvidenceItem::new(
            "latest_event_id",
            latest.event_id.to_string(),
        ));
        evidence.push(EvidenceItem::new("latest_event_date", latest.date.clone()));
        evidence.push(EvidenceItem::new(
            "latest_event_log",
            latest.log_name.clone(),
        ));
        evidence.push(EvidenceItem::new(
            "latest_event_description",
            truncate(&latest.description),
        ));
    }

    if let Some((process_name, pid)) = combined.iter().find_map(extract_locking_process) {
        evidence.push(EvidenceItem::new(
            "locking_process_name",
            process_name.clone(),
        ));
        evidence.push(EvidenceItem::new("locking_process_pid", pid));
        if process_name.to_ascii_lowercase().contains("avp.exe")
            || process_name.to_ascii_lowercase().contains("kaspersky")
        {
            failure_classes.push(FailureClass::P4);
            notes.push(
                "Profile evidence points to security software involvement; Guardian must stay in guided recovery mode."
                    .to_string(),
            );
        }
    }

    let has_1552 = combined.iter().any(|event| event.event_id == 1552);
    let has_1511 = combined.iter().any(|event| event.event_id == 1511);
    let has_1500 = combined.iter().any(|event| event.event_id == 1500);
    let has_1515 = combined.iter().any(|event| event.event_id == 1515);
    let has_1512 = combined.iter().any(|event| event.event_id == 1512);
    let has_1542 = combined.iter().any(|event| event.event_id == 1542);

    if has_1552 {
        status = StatusLevel::Warn;
        failure_classes.push(FailureClass::P1);
        notes.push(
            "Recent User Profiles Service events indicate a registry-lock condition (P1)."
                .to_string(),
        );
    }
    if has_1511 && (has_1500 || has_1515 || has_1542) {
        status = StatusLevel::Warn;
        failure_classes.push(FailureClass::P2);
        notes
            .push("Recent profile events match the temporary-profile risk chain (P2).".to_string());
    }
    if has_1512 || has_1542 {
        status = StatusLevel::Warn;
        failure_classes.push(FailureClass::P3);
        notes.push("Recent profile events include hive load or unload failures (P3).".to_string());
    }

    push_failure_classes(&mut evidence, &mut failure_classes);

    let summary = if failure_classes.is_empty() {
        "No recent critical User Profile Service events were found in Application or Operational logs."
            .to_string()
    } else {
        format!(
            "Detected recent User Profile Service evidence for {}.",
            failure_classes
                .iter()
                .map(|class| class.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };

    Ok(DomainReport::new(status, summary, evidence, notes))
}

const PROFILE_EVENT_IDS: [u32; 6] = [1500, 1511, 1512, 1515, 1542, 1552];

fn profile_events(
    log_name: &str,
    system_filter: Option<&str>,
) -> Result<Vec<EventRecord>, GuardianError> {
    let event_filter = PROFILE_EVENT_IDS
        .iter()
        .map(|event_id| format!("EventID={event_id}"))
        .collect::<Vec<_>>()
        .join(" or ");
    let xpath = if let Some(filter) = system_filter {
        format!("*[System[({filter}) and ({event_filter})]]")
    } else {
        format!("*[System[({event_filter})]]")
    };

    query_recent_events(log_name, &xpath, 8).map_err(|error| {
        GuardianError::invalid_state(format!(
            "profile event query failed for `{log_name}`: {error}"
        ))
    })
}

fn merge_events(application: &[EventRecord], operational: &[EventRecord]) -> Vec<EventRecord> {
    let mut combined = application.to_vec();
    combined.extend_from_slice(operational);
    combined.sort_by(|left, right| right.date.cmp(&left.date));
    combined
}

fn extract_locking_process(event: &EventRecord) -> Option<(String, String)> {
    let structured_process_name = event
        .data_fields
        .iter()
        .find(|field| field_name_matches(field.name.as_deref(), "InterferingImageName"))
        .map(|field| field.value.trim())
        .filter(|value| !value.is_empty());
    let structured_pid = event
        .data_fields
        .iter()
        .find(|field| field_name_matches(field.name.as_deref(), "InterferingPID"))
        .map(|field| field.value.trim())
        .filter(|value| !value.is_empty());

    if let (Some(process_name), Some(pid)) = (structured_process_name, structured_pid) {
        return Some((process_name.to_string(), pid.to_string()));
    }

    extract_locking_process_from_description(&event.description)
}

fn extract_locking_process_from_description(description: &str) -> Option<(String, String)> {
    let process_name = description
        .split("Process name:")
        .nth(1)?
        .split(", PID:")
        .next()?
        .trim()
        .to_string();
    let pid = description
        .split("PID:")
        .nth(1)?
        .split(',')
        .next()?
        .trim()
        .to_string();

    if process_name.is_empty() || pid.is_empty() {
        None
    } else {
        Some((process_name, pid))
    }
}

fn field_name_matches(field_name: Option<&str>, expected: &str) -> bool {
    let Some(field_name) = field_name.map(str::trim) else {
        return false;
    };

    field_name == expected
        || field_name
            .rsplit(['.', '/', ':'])
            .next()
            .is_some_and(|leaf| leaf == expected)
}

fn truncate(value: &str) -> String {
    const LIMIT: usize = 220;
    if value.chars().count() <= LIMIT {
        value.to_string()
    } else {
        format!("{}...", value.chars().take(LIMIT).collect::<String>())
    }
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

#[cfg(test)]
mod tests {
    use guardian_windows::eventlog::{EventField, EventRecord};

    use super::{extract_locking_process, extract_locking_process_from_description};

    #[test]
    fn extracts_process_name_and_pid_from_structured_event_data() {
        let event = EventRecord {
            log_name: "Application".to_string(),
            source: "Microsoft-Windows-User Profiles Service".to_string(),
            date: "2026-04-16T21:22:24.2850000Z".to_string(),
            event_id: 1552,
            level: "Error".to_string(),
            description: "Structured payload".to_string(),
            data_fields: vec![
                EventField {
                    name: Some("ProfileFailure.InterferingImageName".to_string()),
                    value: r"C:\Program Files\Foo\avp.exe".to_string(),
                },
                EventField {
                    name: Some("ProfileFailure.InterferingPID".to_string()),
                    value: "6640".to_string(),
                },
            ],
        };
        let (process_name, pid) =
            extract_locking_process(&event).expect("expected structured process extraction");
        assert!(process_name.ends_with("avp.exe"));
        assert_eq!(pid, "6640");
    }

    #[test]
    fn extracts_process_name_and_pid_from_legacy_description() {
        let description = "User hive is loaded by another process (Registry Lock) Process name: C:\\Program Files\\Foo\\avp.exe, PID: 6640, ProfSvc PID: 2868.";
        let (process_name, pid) = extract_locking_process_from_description(description)
            .expect("expected process extraction");
        assert!(process_name.ends_with("avp.exe"));
        assert_eq!(pid, "6640");
    }
}
