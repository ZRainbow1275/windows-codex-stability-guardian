use std::{
    collections::BTreeSet,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
};

use crate::paths::codex_home_dir;

const UNC_PREFIX: &str = r"\\?\";

pub fn codex_config_path() -> io::Result<PathBuf> {
    Ok(codex_home_dir()?.join("config.toml"))
}

pub fn expected_project_trust_keys(project_path: &Path) -> Vec<String> {
    let lower = normalize_project_trust_lookup_key(&project_path.display().to_string());
    if lower.is_empty() {
        return Vec::new();
    }

    let mut keys = vec![lower.clone()];
    if let Some(non_unc) = lower.strip_prefix(UNC_PREFIX) {
        push_unique(&mut keys, non_unc.to_string());
    } else {
        push_unique(&mut keys, format!("{UNC_PREFIX}{lower}"));
    }
    keys
}

pub fn missing_project_trust_keys(
    config_text: &str,
    project_path: &Path,
) -> io::Result<Vec<String>> {
    let trusted_keys = trusted_project_lookup_keys(config_text)?;
    Ok(expected_project_trust_keys(project_path)
        .into_iter()
        .filter(|key| !trusted_keys.contains(key))
        .collect())
}

pub fn trusted_project_lookup_keys(config_text: &str) -> io::Result<BTreeSet<String>> {
    let mut trusted = BTreeSet::new();
    let mut current_project: Option<String> = None;
    let mut current_trust_level: Option<String> = None;

    for raw_line in config_text.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            finalize_project_section(&mut trusted, &mut current_project, &mut current_trust_level);
            if let Some(project_key) = parse_project_header(line)? {
                current_project = Some(project_key);
            }
            continue;
        }

        if current_project.is_none() || line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(trust_level) = parse_trust_level(line)? {
            current_trust_level = Some(trust_level);
        }
    }

    finalize_project_section(&mut trusted, &mut current_project, &mut current_trust_level);
    Ok(trusted)
}

pub fn append_trusted_project_entries(config_text: &str, missing_keys: &[String]) -> String {
    if missing_keys.is_empty() {
        return config_text.to_string();
    }

    let mut output = config_text.to_string();
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
    if !output.is_empty() && !output.ends_with("\n\n") {
        output.push('\n');
    }

    for key in missing_keys {
        let escaped = escape_toml_basic_string(key);
        output.push_str(&format!(
            "[projects.\"{escaped}\"]\ntrust_level = \"trusted\"\n\n"
        ));
    }

    output
}

fn normalize_project_trust_lookup_key(value: &str) -> String {
    if cfg!(target_os = "windows") {
        value.to_ascii_lowercase()
    } else {
        value.to_string()
    }
}

fn parse_project_header(line: &str) -> io::Result<Option<String>> {
    let Some(inner) = line
        .strip_prefix("[projects.\"")
        .and_then(|value| value.strip_suffix("\"]"))
    else {
        return Ok(None);
    };

    Ok(Some(normalize_project_trust_lookup_key(
        &decode_toml_basic_string(inner)?,
    )))
}

fn parse_trust_level(line: &str) -> io::Result<Option<String>> {
    let Some(rest) = line.strip_prefix("trust_level") else {
        return Ok(None);
    };
    let Some(rest) = rest.trim_start().strip_prefix('=') else {
        return Ok(None);
    };

    Ok(Some(normalize_project_trust_lookup_key(
        &decode_quoted_toml_basic_string(rest.trim())?,
    )))
}

fn finalize_project_section(
    trusted: &mut BTreeSet<String>,
    current_project: &mut Option<String>,
    current_trust_level: &mut Option<String>,
) {
    if let (Some(project), Some(trust_level)) = (current_project.take(), current_trust_level.take())
        && trust_level == "trusted"
    {
        trusted.insert(project);
    }
}

fn decode_quoted_toml_basic_string(input: &str) -> io::Result<String> {
    let mut chars = input.chars();
    if chars.next() != Some('"') {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("expected TOML basic string, got `{input}`"),
        ));
    }

    let mut decoded = String::new();
    let mut escape = false;
    let mut unicode_escape: Option<(usize, String)> = None;

    for ch in chars {
        if let Some((expected_len, value)) = unicode_escape.as_mut() {
            value.push(ch);
            if value.len() == *expected_len {
                let codepoint = u32::from_str_radix(value, 16).map_err(|error| {
                    io::Error::new(
                        ErrorKind::InvalidData,
                        format!("invalid TOML unicode escape `{value}`: {error}"),
                    )
                })?;
                let rendered = char::from_u32(codepoint).ok_or_else(|| {
                    io::Error::new(
                        ErrorKind::InvalidData,
                        format!("invalid TOML unicode codepoint `{value}`"),
                    )
                })?;
                decoded.push(rendered);
                unicode_escape = None;
            }
            continue;
        }

        if escape {
            match ch {
                '\\' => decoded.push('\\'),
                '"' => decoded.push('"'),
                'b' => decoded.push('\u{0008}'),
                'f' => decoded.push('\u{000C}'),
                'n' => decoded.push('\n'),
                'r' => decoded.push('\r'),
                't' => decoded.push('\t'),
                'u' => unicode_escape = Some((4, String::new())),
                'U' => unicode_escape = Some((8, String::new())),
                other => {
                    return Err(io::Error::new(
                        ErrorKind::InvalidData,
                        format!("unsupported TOML escape `\\{other}`"),
                    ));
                }
            }
            escape = false;
            continue;
        }

        match ch {
            '\\' => escape = true,
            '"' => return Ok(decoded),
            other => decoded.push(other),
        }
    }

    Err(io::Error::new(
        ErrorKind::InvalidData,
        format!("unterminated TOML basic string `{input}`"),
    ))
}

fn decode_toml_basic_string(input: &str) -> io::Result<String> {
    decode_quoted_toml_basic_string(&format!("\"{input}\""))
}

fn escape_toml_basic_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        append_trusted_project_entries, expected_project_trust_keys, missing_project_trust_keys,
        trusted_project_lookup_keys,
    };
    use std::path::Path;

    #[test]
    fn expected_keys_include_plain_and_unc_lowercase_variants() {
        let keys = expected_project_trust_keys(Path::new(r"D:\Desktop\Inkforge"));
        assert_eq!(
            keys,
            vec![
                r"d:\desktop\inkforge".to_string(),
                r"\\?\d:\desktop\inkforge".to_string(),
            ]
        );
    }

    #[test]
    fn parser_only_collects_trusted_project_entries() {
        let keys = trusted_project_lookup_keys(
            "[projects.\"d:\\\\desktop\\\\inkforge\"]\ntrust_level = \"trusted\"\n\n[projects.\"D:\\\\Desktop\\\\Other\"]\ntrust_level = \"ask\"\n",
        )
        .expect("parse trusted keys");

        assert!(keys.contains(r"d:\desktop\inkforge"));
        assert!(!keys.contains(r"d:\desktop\other"));
    }

    #[test]
    fn missing_keys_ignore_existing_trusted_entries() {
        let missing = missing_project_trust_keys(
            "[projects.\"d:\\\\desktop\\\\inkforge\"]\ntrust_level = \"trusted\"\n",
            Path::new(r"D:\Desktop\Inkforge"),
        )
        .expect("compute missing keys");

        assert_eq!(missing, vec![r"\\?\d:\desktop\inkforge".to_string()]);
    }

    #[test]
    fn append_entries_writes_projects_table_blocks() {
        let rendered = append_trusted_project_entries(
            "# existing config\n",
            &[
                r"d:\desktop\inkforge".to_string(),
                r"\\?\d:\desktop\inkforge".to_string(),
            ],
        );

        assert!(rendered.contains("[projects.\"d:\\\\desktop\\\\inkforge\"]"));
        assert!(rendered.contains("[projects.\"\\\\\\\\?\\\\d:\\\\desktop\\\\inkforge\"]"));
        assert!(rendered.contains("trust_level = \"trusted\""));
    }
}
