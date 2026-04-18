use std::{ffi::OsString, io};

use crate::process::run_command;
use quick_xml::{
    Reader,
    events::{BytesStart, BytesText, Event},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventField {
    pub name: Option<String>,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRecord {
    pub log_name: String,
    pub source: String,
    pub date: String,
    pub event_id: u32,
    pub level: String,
    pub description: String,
    pub data_fields: Vec<EventField>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextTarget {
    EventId,
    Level,
    Channel,
    Message,
    Data,
}

pub fn is_available() -> bool {
    cfg!(target_os = "windows")
}

pub fn query_recent_events(
    log_name: &str,
    xpath: &str,
    count: usize,
) -> io::Result<Vec<EventRecord>> {
    let args = vec![
        OsString::from("qe"),
        OsString::from(log_name),
        OsString::from("/rd:true"),
        OsString::from(format!("/c:{count}")),
        OsString::from("/f:xml"),
        OsString::from(format!("/q:{xpath}")),
    ];

    let output = run_command("wevtutil", &args)?;
    if !output.success() {
        let detail = if output.stderr.is_empty() {
            output.stdout
        } else {
            output.stderr
        };
        return Err(io::Error::other(format!(
            "wevtutil query failed for `{log_name}` with status {}: {detail}",
            output.status
        )));
    }

    parse_wevtutil_xml(&output.stdout, log_name)
}

fn parse_wevtutil_xml(contents: &str, queried_log_name: &str) -> io::Result<Vec<EventRecord>> {
    if contents.trim().is_empty() {
        return Ok(Vec::new());
    }

    let sanitized = contents.trim_matches('\u{feff}').replace('\0', "");
    let wrapped = format!("<Events>{sanitized}</Events>");
    let mut reader = Reader::from_str(&wrapped);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut events = Vec::new();
    let mut current = None;
    let mut text_target = None;
    let mut in_user_data = false;
    let mut user_data_nodes: Vec<UserDataNode> = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(event)) => {
                if event.name().as_ref() == b"UserData" {
                    in_user_data = true;
                    user_data_nodes.clear();
                    text_target = None;
                } else if in_user_data {
                    handle_user_data_start(current.as_mut(), &mut user_data_nodes, &event, false);
                } else {
                    handle_start(&mut current, &mut text_target, &event, false);
                }
            }
            Ok(Event::Empty(event)) => {
                if event.name().as_ref() == b"UserData" {
                    in_user_data = false;
                    user_data_nodes.clear();
                    text_target = None;
                } else if in_user_data {
                    handle_user_data_start(current.as_mut(), &mut user_data_nodes, &event, true);
                } else {
                    handle_start(&mut current, &mut text_target, &event, true);
                }
            }
            Ok(Event::Text(text)) => {
                let decoded = decode_text(&text);
                if decoded.is_empty() {
                    buf.clear();
                    continue;
                }

                if in_user_data {
                    push_user_data_text(&mut user_data_nodes, &decoded);
                    buf.clear();
                    continue;
                }

                if let Some(builder) = current.as_mut() {
                    builder.push_text(text_target, &decoded);
                }
            }
            Ok(Event::CData(text)) => {
                let decoded = String::from_utf8_lossy(text.as_ref()).trim().to_string();
                if decoded.is_empty() {
                    buf.clear();
                    continue;
                }

                if in_user_data {
                    push_user_data_text(&mut user_data_nodes, &decoded);
                    buf.clear();
                    continue;
                }

                if let Some(builder) = current.as_mut() {
                    builder.push_text(text_target, &decoded);
                }
            }
            Ok(Event::End(event)) => {
                if in_user_data {
                    match event.name().as_ref() {
                        b"UserData" => {
                            in_user_data = false;
                            user_data_nodes.clear();
                        }
                        _ => {
                            if let Some(builder) = current.as_mut() {
                                finish_user_data_node(builder, &mut user_data_nodes);
                            } else {
                                user_data_nodes.pop();
                            }
                        }
                    }
                    buf.clear();
                    continue;
                }

                match event.name().as_ref() {
                    b"Event" => {
                        if let Some(builder) = current.take()
                            && let Some(event) = builder.build(queried_log_name)
                        {
                            events.push(event);
                        }
                        text_target = None;
                    }
                    b"Data" | b"Binary" => {
                        if let Some(builder) = current.as_mut() {
                            builder.finish_data();
                        }
                        if matches!(text_target, Some(TextTarget::Data)) {
                            text_target = None;
                        }
                    }
                    b"EventID" | b"Level" | b"Channel" | b"Message" => {
                        text_target = None;
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => {
                return Err(io::Error::other(format!(
                    "failed to parse wevtutil XML at byte {}: {error}",
                    reader.error_position()
                )));
            }
            _ => {}
        }

        buf.clear();
    }

    Ok(events)
}

#[derive(Debug, Default)]
struct UserDataNode {
    name: String,
    text: String,
    child_count: usize,
}

fn handle_start(
    current: &mut Option<EventBuilder>,
    text_target: &mut Option<TextTarget>,
    event: &BytesStart<'_>,
    is_empty: bool,
) {
    match event.name().as_ref() {
        b"Event" if !is_empty => {
            *current = Some(EventBuilder::default());
            *text_target = None;
        }
        b"Provider" => {
            if let Some(builder) = current.as_mut()
                && let Some(name) = attribute_value(event, b"Name")
            {
                builder.source = name;
            }
        }
        b"TimeCreated" => {
            if let Some(builder) = current.as_mut()
                && let Some(system_time) = attribute_value(event, b"SystemTime")
            {
                builder.date = system_time;
            }
        }
        b"EventID" if !is_empty => *text_target = Some(TextTarget::EventId),
        b"Level" if !is_empty => *text_target = Some(TextTarget::Level),
        b"Channel" if !is_empty => *text_target = Some(TextTarget::Channel),
        b"Message" if !is_empty => *text_target = Some(TextTarget::Message),
        b"Data" | b"Binary" => {
            if let Some(builder) = current.as_mut() {
                let name = attribute_value(event, b"Name");
                if is_empty {
                    builder.data_fields.push(EventField {
                        name,
                        value: String::new(),
                    });
                    *text_target = None;
                } else {
                    builder.begin_data(name);
                    *text_target = Some(TextTarget::Data);
                }
            }
        }
        _ => {}
    }
}

fn handle_user_data_start(
    builder: Option<&mut EventBuilder>,
    nodes: &mut Vec<UserDataNode>,
    event: &BytesStart<'_>,
    is_empty: bool,
) {
    if let Some(parent) = nodes.last_mut() {
        parent.child_count += 1;
    }

    let name = normalize_xml_name(event.name().as_ref());
    if name.is_empty() {
        return;
    }

    nodes.push(UserDataNode {
        name,
        text: String::new(),
        child_count: 0,
    });

    if is_empty {
        if let Some(builder) = builder {
            finish_user_data_node(builder, nodes);
        } else {
            nodes.pop();
        }
    }
}

fn push_user_data_text(nodes: &mut [UserDataNode], text: &str) {
    let Some(node) = nodes.last_mut() else {
        return;
    };

    if !node.text.is_empty() {
        node.text.push(' ');
    }
    node.text.push_str(text);
}

fn finish_user_data_node(builder: &mut EventBuilder, nodes: &mut Vec<UserDataNode>) {
    let Some(node) = nodes.pop() else {
        return;
    };

    let value = node.text.trim().to_string();
    if value.is_empty() && node.child_count > 0 {
        return;
    }

    let mut path = nodes
        .iter()
        .map(|parent| parent.name.as_str())
        .collect::<Vec<_>>();
    path.push(node.name.as_str());
    builder.data_fields.push(EventField {
        name: Some(path.join(".")),
        value,
    });
}

fn attribute_value(event: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    event
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == key)
        .map(|attribute| String::from_utf8_lossy(attribute.value.as_ref()).to_string())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn decode_text(text: &BytesText<'_>) -> String {
    String::from_utf8_lossy(text.as_ref())
        .into_owned()
        .trim()
        .to_string()
}

fn normalize_xml_name(name: &[u8]) -> String {
    let raw = String::from_utf8_lossy(name);
    raw.rsplit(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[derive(Default)]
struct EventBuilder {
    log_name: String,
    source: String,
    date: String,
    event_id_text: String,
    level_text: String,
    rendered_message: String,
    data_fields: Vec<EventField>,
    pending_data: Option<EventField>,
}

impl EventBuilder {
    fn begin_data(&mut self, name: Option<String>) {
        self.pending_data = Some(EventField {
            name,
            value: String::new(),
        });
    }

    fn finish_data(&mut self) {
        if let Some(field) = self.pending_data.take() {
            self.data_fields.push(field);
        }
    }

    fn push_text(&mut self, target: Option<TextTarget>, text: &str) {
        match target {
            Some(TextTarget::EventId) => self.event_id_text.push_str(text),
            Some(TextTarget::Level) => self.level_text.push_str(text),
            Some(TextTarget::Channel) => self.log_name.push_str(text),
            Some(TextTarget::Message) => {
                if !self.rendered_message.is_empty() {
                    self.rendered_message.push(' ');
                }
                self.rendered_message.push_str(text);
            }
            Some(TextTarget::Data) => {
                if let Some(field) = self.pending_data.as_mut() {
                    if !field.value.is_empty() {
                        field.value.push(' ');
                    }
                    field.value.push_str(text);
                }
            }
            None => {}
        }
    }

    fn build(mut self, queried_log_name: &str) -> Option<EventRecord> {
        self.finish_data();
        let event_id = self.event_id_text.trim().parse::<u32>().ok()?;
        let log_name = non_empty_or(self.log_name, queried_log_name.to_string());
        let source = non_empty_or(self.source, "unknown".to_string());
        let date = non_empty_or_none(self.date)?;
        let level = level_display(&self.level_text);
        let description = if let Some(message) = non_empty_or_none(self.rendered_message) {
            message
        } else {
            synthesize_description(event_id, &self.data_fields)
        };

        Some(EventRecord {
            log_name,
            source,
            date,
            event_id,
            level,
            description,
            data_fields: self.data_fields,
        })
    }
}

fn level_display(raw: &str) -> String {
    match raw.trim() {
        "1" => "Critical".to_string(),
        "2" => "Error".to_string(),
        "3" => "Warning".to_string(),
        "4" => "Information".to_string(),
        "5" => "Verbose".to_string(),
        other if !other.is_empty() => other.to_string(),
        _ => "Unknown".to_string(),
    }
}

fn synthesize_description(event_id: u32, data_fields: &[EventField]) -> String {
    match event_id {
        1552 => {
            let process_name = data_field_value(data_fields, "InterferingImageName");
            let pid = data_field_value(data_fields, "InterferingPID");
            let profsvc_pid = data_field_value(data_fields, "ProfsvcPID");

            let mut parts =
                vec!["User hive is loaded by another process (Registry Lock)".to_string()];
            if let (Some(process_name), Some(pid)) = (process_name, pid) {
                parts.push(format!("Process name: {process_name}, PID: {pid}."));
            } else if let Some(process_name) = process_name {
                parts.push(format!("Process name: {process_name}."));
            }
            if let Some(profsvc_pid) = profsvc_pid {
                parts.push(format!("ProfSvc PID: {profsvc_pid}."));
            }
            parts.join(" ")
        }
        1511 => {
            "Windows cannot find the local profile and is logging you on with a temporary profile."
                .to_string()
        }
        1512 => format_generic_description(
            "Windows cannot locate the local profile and is loading a temporary profile.",
            data_fields,
        ),
        1515 => format_generic_description(
            "Windows has backed up the local profile and is trying to log on with a temporary profile.",
            data_fields,
        ),
        1542 => format_generic_description(
            "Windows cannot load the classes registry file for the local profile.",
            data_fields,
        ),
        1500 => format_generic_description(
            "Windows cannot log you on because your profile cannot be loaded.",
            data_fields,
        ),
        _ => format_generic_description(
            &format!("Structured event payload captured for Event ID {event_id}."),
            data_fields,
        ),
    }
}

fn format_generic_description(prefix: &str, data_fields: &[EventField]) -> String {
    let payload = data_fields
        .iter()
        .filter_map(|field| {
            if field.value.trim().is_empty() {
                return None;
            }

            Some(match &field.name {
                Some(name) => format!("{name}={}", field.value.trim()),
                None => field.value.trim().to_string(),
            })
        })
        .collect::<Vec<_>>();

    if payload.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix} Payload: {}.", payload.join("; "))
    }
}

fn data_field_value<'a>(data_fields: &'a [EventField], name: &str) -> Option<&'a str> {
    data_fields
        .iter()
        .find(|field| {
            field_name_matches(field.name.as_deref(), name) && !field.value.trim().is_empty()
        })
        .map(|field| field.value.trim())
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

fn non_empty_or_none(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

fn non_empty_or(value: String, fallback: String) -> String {
    non_empty_or_none(value).unwrap_or(fallback)
}

#[cfg(test)]
mod tests {
    use super::{data_field_value, parse_wevtutil_xml};

    #[test]
    fn parses_profile_event_xml_and_extracts_named_event_data() {
        let contents = r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Microsoft-Windows-User Profiles Service'/><EventID>1552</EventID><Level>2</Level><TimeCreated SystemTime='2026-04-16T13:35:05.4178290Z'/><Channel>Application</Channel></System><EventData><Data Name='InterferingImageName'>C:\Program Files (x86)\Kaspersky Lab\Kaspersky 21.22\avp.exe</Data><Data Name='InterferingPID'>6640</Data><Data Name='ProfsvcPID'>2868</Data></EventData></Event>"#;

        let events = parse_wevtutil_xml(contents, "Application").expect("parse xml");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, 1552);
        assert_eq!(events[0].level, "Error");
        assert_eq!(events[0].log_name, "Application");
        assert_eq!(
            data_field_value(&events[0].data_fields, "InterferingImageName"),
            Some(r"C:\Program Files (x86)\Kaspersky Lab\Kaspersky 21.22\avp.exe")
        );
        assert!(events[0].description.contains("Registry Lock"));
        assert!(events[0].description.contains("Process name:"));
    }

    #[test]
    fn parses_multiple_events_from_single_wevtutil_xml_stream() {
        let contents = concat!(
            r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Microsoft-Windows-User Profiles Service'/><EventID>1511</EventID><Level>2</Level><TimeCreated SystemTime='2026-04-15T14:44:15.4020671Z'/><Channel>Application</Channel></System><EventData></EventData></Event>"#,
            r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Microsoft-Windows-User Profiles Service'/><EventID>1542</EventID><Level>2</Level><TimeCreated SystemTime='2026-04-15T14:44:15.2893973Z'/><Channel>Application</Channel></System><EventData><Data Name='Error'>Access is denied.</Data></EventData></Event>"#
        );

        let events = parse_wevtutil_xml(contents, "Application").expect("parse xml");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, 1511);
        assert_eq!(events[1].event_id, 1542);
        assert!(events[1].description.contains("Error=Access is denied."));
    }

    #[test]
    fn parses_userdata_nested_xml_into_structured_fields() {
        let contents = r#"<Event xmlns='http://schemas.microsoft.com/win/2004/08/events/event'><System><Provider Name='Microsoft-Windows-User Profiles Service'/><EventID>1552</EventID><Level>2</Level><TimeCreated SystemTime='2026-04-16T13:35:05.4178290Z'/><Channel>Microsoft-Windows-User Profile Service/Operational</Channel></System><UserData><ProfileFailure><InterferingImageName>C:\Program Files\Foo\avp.exe</InterferingImageName><InterferingPID>6640</InterferingPID><ProfsvcPID>2868</ProfsvcPID></ProfileFailure></UserData></Event>"#;

        let events = parse_wevtutil_xml(
            contents,
            "Microsoft-Windows-User Profile Service/Operational",
        )
        .expect("parse xml");
        assert_eq!(events.len(), 1);
        assert_eq!(
            data_field_value(&events[0].data_fields, "InterferingImageName"),
            Some(r"C:\Program Files\Foo\avp.exe")
        );
        assert_eq!(
            data_field_value(&events[0].data_fields, "InterferingPID"),
            Some("6640")
        );
        assert!(
            events[0]
                .description
                .contains("Process name: C:\\Program Files\\Foo\\avp.exe")
        );
    }
}
