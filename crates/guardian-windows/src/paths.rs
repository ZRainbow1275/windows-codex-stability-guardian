use std::{
    env, fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    time::SystemTime,
};

pub fn collector_name() -> &'static str {
    "guardian-windows-paths"
}

pub fn user_profile_dir() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("USERPROFILE") {
        return Ok(PathBuf::from(path));
    }

    match (env::var_os("HOMEDRIVE"), env::var_os("HOMEPATH")) {
        (Some(drive), Some(path)) if cfg!(target_os = "windows") => {
            return Ok(PathBuf::from(format!(
                "{}{}",
                drive.to_string_lossy(),
                path.to_string_lossy()
            )));
        }
        _ => {}
    }

    if let Some(path) = env::var_os("HOME") {
        return Ok(PathBuf::from(path));
    }

    Err(io::Error::new(
        ErrorKind::NotFound,
        "Neither USERPROFILE nor HOME is defined",
    ))
}

pub fn codex_home_dir() -> io::Result<PathBuf> {
    Ok(user_profile_dir()?.join(".codex"))
}

pub fn codex_state_db_candidates(codex_home: &Path) -> io::Result<Vec<PathBuf>> {
    if !codex_home.exists() {
        return Ok(Vec::new());
    }

    let mut candidates = Vec::new();
    for entry in fs::read_dir(codex_home)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with("state_") && name.ends_with(".sqlite") {
            candidates.push(path);
        }
    }

    Ok(candidates)
}

pub fn latest_codex_state_db(codex_home: &Path) -> io::Result<Option<PathBuf>> {
    let candidates = codex_state_db_candidates(codex_home)?;
    Ok(candidates
        .into_iter()
        .max_by_key(|path| (codex_state_db_index(path), modified_at(path))))
}

pub fn wslconfig_path() -> io::Result<PathBuf> {
    Ok(user_profile_dir()?.join(".wslconfig"))
}

pub fn guardian_data_dir() -> io::Result<PathBuf> {
    if let Some(path) = env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("guardian"));
    }

    Ok(user_profile_dir()?.join(".guardian"))
}

pub fn guardian_audit_dir() -> io::Result<PathBuf> {
    Ok(guardian_data_dir()?.join("audits"))
}

pub fn guardian_bundle_dir() -> io::Result<PathBuf> {
    Ok(guardian_data_dir()?.join("bundles"))
}

pub fn guardian_backup_dir() -> io::Result<PathBuf> {
    Ok(guardian_data_dir()?.join("backups"))
}

pub fn codex_tui_log_candidates() -> io::Result<Vec<PathBuf>> {
    let codex_home = codex_home_dir()?;
    Ok(vec![
        codex_home.join("log").join("codex-tui.log"),
        codex_home.join("codex-tui.log"),
    ])
}

fn codex_state_db_index(path: &Path) -> i64 {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return -1;
    };
    let Some(index_text) = name
        .strip_prefix("state_")
        .and_then(|value| value.strip_suffix(".sqlite"))
    else {
        return -1;
    };
    index_text.parse::<i64>().unwrap_or(-1)
}

fn modified_at(path: &Path) -> SystemTime {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

#[cfg(test)]
mod tests {
    use super::{codex_state_db_candidates, latest_codex_state_db};
    use std::{
        env, fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn latest_codex_state_db_prefers_highest_numeric_suffix() {
        let root = unique_temp_dir("guardian-paths-state-index");
        fs::create_dir_all(&root).expect("create temp codex home");
        fs::write(root.join("state_5.sqlite"), "").expect("write state 5");
        fs::write(root.join("state_12.sqlite"), "").expect("write state 12");

        let selected = latest_codex_state_db(&root)
            .expect("discover latest state db")
            .expect("expected a selected state db");
        assert!(selected.ends_with("state_12.sqlite"));
    }

    #[test]
    fn state_db_candidates_ignore_non_matching_files() {
        let root = unique_temp_dir("guardian-paths-state-filter");
        fs::create_dir_all(&root).expect("create temp codex home");
        fs::write(root.join("state_5.sqlite"), "").expect("write state 5");
        fs::write(root.join("state_backup.txt"), "").expect("write other file");

        let candidates = codex_state_db_candidates(&root).expect("discover state candidates");
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].ends_with("state_5.sqlite"));
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before epoch")
            .as_nanos();
        env::temp_dir().join(format!("{prefix}-{unique}"))
    }
}
