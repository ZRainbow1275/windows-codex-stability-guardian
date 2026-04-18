use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    path::{Path, PathBuf},
};

use chrono::Local;
use guardian_core::{
    GuardianError,
    types::{ActionPlan, HealthReport},
};
use guardian_windows::paths::{guardian_audit_dir, guardian_bundle_dir};
use serde::Serialize;
use serde_json::Value;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const HEALTH_REPORT_FILE: &str = "health-report.json";
const PROFILE_DIAGNOSIS_FILE: &str = "profile-diagnosis.json";
const AUDIT_SUMMARY_FILE: &str = "audit-summary.json";
const MANIFEST_FILE: &str = "bundle-manifest.json";
const BUNDLE_PREFIX: &str = "bundle-";
const ZIP_EXTENSION: &str = "zip";

pub fn planned_actions() -> Vec<ActionPlan> {
    vec![ActionPlan::new(
        "guardian export bundle".to_string(),
        "Write the current diagnostic bundle to disk for later review.".to_string(),
        false,
    )]
}

#[derive(Debug, Clone, Default)]
pub struct BundleExportOptions {
    pub output_root: Option<PathBuf>,
    pub create_zip_archive: bool,
    pub retention_limit: Option<usize>,
}

impl BundleExportOptions {
    fn output_root(&self) -> Option<&Path> {
        self.output_root.as_deref()
    }
}

#[derive(Debug, Clone)]
pub struct BundleExportResult {
    pub bundle_root: PathBuf,
    pub health_report_path: PathBuf,
    pub profile_diagnosis_path: PathBuf,
    pub audit_summary_path: PathBuf,
    pub manifest_path: PathBuf,
    pub archive_path: Option<PathBuf>,
    pub audit_entries: usize,
    pub used_explicit_output: bool,
    pub retention_limit: Option<usize>,
    pub retention_parent: Option<PathBuf>,
    pub retention_kept_family_count: usize,
    pub retention_deleted_paths: Vec<PathBuf>,
}

#[derive(Debug, Serialize)]
struct BundleManifest {
    generated_at: String,
    bundle_root: String,
    health_report_path: String,
    profile_diagnosis_path: String,
    audit_summary_path: String,
    manifest_path: String,
    archive_path: Option<String>,
    audit_entries: usize,
    used_explicit_output: bool,
    retention_limit: Option<usize>,
    retention_parent: Option<String>,
    retention_kept_family_count: usize,
    retention_deleted_paths: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuditSummary {
    generated_at: String,
    source_dir: String,
    entries: Vec<AuditSummaryEntry>,
}

#[derive(Debug, Serialize)]
struct AuditSummaryEntry {
    file_name: String,
    path: String,
    timestamp: Option<String>,
    action: Option<String>,
    outcome: Option<String>,
    parse_error: Option<String>,
}

#[derive(Debug, Clone)]
struct BundleRetentionContext {
    limit: usize,
    parent: PathBuf,
    current_family: String,
}

#[derive(Debug, Clone, Default)]
struct BundleRetentionResult {
    parent: Option<PathBuf>,
    kept_family_count: usize,
    deleted_paths: Vec<PathBuf>,
}

pub fn export_bundle(
    report: &HealthReport,
    profile_report: &HealthReport,
    options: &BundleExportOptions,
) -> Result<BundleExportResult, GuardianError> {
    let default_bundle_parent = guardian_bundle_dir().map_err(GuardianError::Io)?;
    let bundle_root = resolve_bundle_root(&default_bundle_parent, options.output_root());
    let audit_dir = guardian_audit_dir().map_err(GuardianError::Io)?;

    write_bundle_to_directory(
        &bundle_root,
        &audit_dir,
        report,
        profile_report,
        options.output_root().is_some(),
        options.create_zip_archive,
        options.retention_limit,
    )
}

fn write_bundle_to_directory(
    bundle_root: &Path,
    audit_dir: &Path,
    report: &HealthReport,
    profile_report: &HealthReport,
    used_explicit_output: bool,
    create_zip_archive: bool,
    retention_limit: Option<usize>,
) -> Result<BundleExportResult, GuardianError> {
    let retention_context = build_retention_context(bundle_root, retention_limit)?;
    ensure_bundle_root(bundle_root)?;

    let audit_summary = collect_audit_summary_from_dir(audit_dir)?;
    let health_report_path = bundle_root.join(HEALTH_REPORT_FILE);
    let profile_diagnosis_path = bundle_root.join(PROFILE_DIAGNOSIS_FILE);
    let audit_summary_path = bundle_root.join(AUDIT_SUMMARY_FILE);
    let manifest_path = bundle_root.join(MANIFEST_FILE);
    let archive_path = if create_zip_archive {
        Some(archive_path_for_bundle(bundle_root)?)
    } else {
        None
    };

    fs::write(&health_report_path, serde_json::to_string_pretty(report)?)?;
    fs::write(
        &profile_diagnosis_path,
        serde_json::to_string_pretty(profile_report)?,
    )?;
    fs::write(
        &audit_summary_path,
        serde_json::to_string_pretty(&audit_summary)?,
    )?;

    let retention_result = apply_bundle_retention(retention_context)?;
    let manifest = BundleManifest {
        generated_at: Local::now().to_rfc3339(),
        bundle_root: bundle_root.display().to_string(),
        health_report_path: health_report_path.display().to_string(),
        profile_diagnosis_path: profile_diagnosis_path.display().to_string(),
        audit_summary_path: audit_summary_path.display().to_string(),
        manifest_path: manifest_path.display().to_string(),
        archive_path: archive_path.as_ref().map(|path| path.display().to_string()),
        audit_entries: audit_summary.entries.len(),
        used_explicit_output,
        retention_limit,
        retention_parent: retention_result
            .parent
            .as_ref()
            .map(|path| path.display().to_string()),
        retention_kept_family_count: retention_result.kept_family_count,
        retention_deleted_paths: retention_result
            .deleted_paths
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
    };
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;

    if let Some(archive_path) = &archive_path {
        write_bundle_archive(
            archive_path,
            &[
                (HEALTH_REPORT_FILE, &health_report_path),
                (PROFILE_DIAGNOSIS_FILE, &profile_diagnosis_path),
                (AUDIT_SUMMARY_FILE, &audit_summary_path),
                (MANIFEST_FILE, &manifest_path),
            ],
        )?;
    }

    Ok(BundleExportResult {
        bundle_root: bundle_root.to_path_buf(),
        health_report_path,
        profile_diagnosis_path,
        audit_summary_path,
        manifest_path,
        archive_path,
        audit_entries: audit_summary.entries.len(),
        used_explicit_output,
        retention_limit,
        retention_parent: retention_result.parent,
        retention_kept_family_count: retention_result.kept_family_count,
        retention_deleted_paths: retention_result.deleted_paths,
    })
}

fn resolve_bundle_root(default_bundle_parent: &Path, output_root: Option<&Path>) -> PathBuf {
    output_root.map(Path::to_path_buf).unwrap_or_else(|| {
        default_bundle_parent.join(format!("bundle-{}", Local::now().format("%Y%m%d-%H%M%S")))
    })
}

fn build_retention_context(
    bundle_root: &Path,
    retention_limit: Option<usize>,
) -> Result<Option<BundleRetentionContext>, GuardianError> {
    let Some(limit) = retention_limit else {
        return Ok(None);
    };
    if limit == 0 {
        return Err(GuardianError::invalid_state(
            "`--retain` must be greater than zero".to_string(),
        ));
    }
    let parent = bundle_root.parent().ok_or_else(|| {
        GuardianError::invalid_state(format!(
            "bundle output path {} does not have a parent directory",
            bundle_root.display()
        ))
    })?;
    let current_family = bundle_family_name_from_directory(bundle_root).ok_or_else(|| {
        GuardianError::invalid_state(
            "`--retain` requires the bundle output directory name to start with `bundle-`"
                .to_string(),
        )
    })?;

    Ok(Some(BundleRetentionContext {
        limit,
        parent: parent.to_path_buf(),
        current_family,
    }))
}

fn ensure_bundle_root(bundle_root: &Path) -> Result<(), GuardianError> {
    if bundle_root.exists() && !bundle_root.is_dir() {
        return Err(GuardianError::invalid_state(format!(
            "bundle output path {} already exists as a file",
            bundle_root.display()
        )));
    }

    fs::create_dir_all(bundle_root)?;
    Ok(())
}

fn apply_bundle_retention(
    context: Option<BundleRetentionContext>,
) -> Result<BundleRetentionResult, GuardianError> {
    let Some(context) = context else {
        return Ok(BundleRetentionResult::default());
    };

    let family_artifacts = collect_bundle_family_artifacts(&context.parent)?;
    let mut ordered_families = family_artifacts.keys().cloned().collect::<Vec<_>>();
    ordered_families.sort_by(|left, right| right.cmp(left));

    let mut keep_families = vec![context.current_family.clone()];
    for family in ordered_families {
        if family == context.current_family || keep_families.len() >= context.limit {
            continue;
        }
        keep_families.push(family);
    }
    let keep_family_set = keep_families.into_iter().collect::<BTreeSet<_>>();

    let mut deleted_paths = Vec::new();
    for (family, mut paths) in family_artifacts {
        if keep_family_set.contains(&family) {
            continue;
        }

        paths.sort_by_key(|path| path.is_dir());
        for path in paths {
            remove_bundle_artifact(&path)?;
            deleted_paths.push(path);
        }
    }

    Ok(BundleRetentionResult {
        parent: Some(context.parent),
        kept_family_count: keep_family_set.len(),
        deleted_paths,
    })
}

fn collect_bundle_family_artifacts(
    parent: &Path,
) -> Result<BTreeMap<String, Vec<PathBuf>>, GuardianError> {
    let mut families = BTreeMap::<String, Vec<PathBuf>>::new();
    if !parent.exists() {
        return Ok(families);
    }

    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        let family = if file_type.is_dir() {
            bundle_family_name_from_directory(&path)
        } else if file_type.is_file() {
            bundle_family_name_from_archive(&path)
        } else {
            None
        };

        if let Some(family) = family {
            families.entry(family).or_default().push(path);
        }
    }

    Ok(families)
}

fn bundle_family_name_from_directory(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if name.starts_with(BUNDLE_PREFIX) {
        Some(name.to_string())
    } else {
        None
    }
}

fn bundle_family_name_from_archive(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some(ZIP_EXTENSION) {
        return None;
    }

    let stem = path.file_stem()?.to_str()?;
    if stem.starts_with(BUNDLE_PREFIX) {
        Some(stem.to_string())
    } else {
        None
    }
}

fn remove_bundle_artifact(path: &Path) -> Result<(), GuardianError> {
    if path.is_dir() {
        fs::remove_dir_all(path)?;
    } else if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

fn archive_path_for_bundle(bundle_root: &Path) -> Result<PathBuf, GuardianError> {
    let parent = bundle_root.parent().ok_or_else(|| {
        GuardianError::invalid_state(format!(
            "bundle output path {} does not have a parent directory",
            bundle_root.display()
        ))
    })?;
    let name = bundle_root
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            GuardianError::invalid_state(format!(
                "bundle output path {} does not have a valid UTF-8 directory name",
                bundle_root.display()
            ))
        })?;

    Ok(parent.join(format!("{name}.{ZIP_EXTENSION}")))
}

fn write_bundle_archive(
    archive_path: &Path,
    bundle_files: &[(&str, &Path)],
) -> Result<(), GuardianError> {
    if archive_path.exists() && archive_path.is_dir() {
        return Err(GuardianError::invalid_state(format!(
            "bundle archive path {} already exists as a directory",
            archive_path.display()
        )));
    }

    let temporary_archive_path = temporary_archive_path(archive_path);
    let create_result: Result<(), GuardianError> = (|| {
        let archive_file = File::create(&temporary_archive_path)?;
        let mut zip_writer = ZipWriter::new(archive_file);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

        for (entry_name, source_path) in bundle_files {
            zip_writer
                .start_file((*entry_name).to_string(), options)
                .map_err(zip_error)?;
            let mut source_file = File::open(source_path)?;
            std::io::copy(&mut source_file, &mut zip_writer)?;
        }

        zip_writer.finish().map_err(zip_error)?;
        Ok(())
    })();

    if create_result.is_err() {
        let _ = fs::remove_file(&temporary_archive_path);
    }
    create_result?;

    if archive_path.exists() {
        fs::remove_file(archive_path)?;
    }
    fs::rename(&temporary_archive_path, archive_path)?;
    Ok(())
}

fn temporary_archive_path(archive_path: &Path) -> PathBuf {
    let file_name = archive_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("bundle.zip");
    archive_path.with_file_name(format!(
        "{file_name}.tmp-{}",
        Local::now().format("%Y%m%d-%H%M%S%3f")
    ))
}

fn zip_error(error: zip::result::ZipError) -> GuardianError {
    GuardianError::invalid_state(format!("bundle archive write failed: {error}"))
}

fn collect_audit_summary_from_dir(audit_dir: &Path) -> Result<AuditSummary, GuardianError> {
    let mut entries = Vec::new();

    if audit_dir.exists() {
        for entry in fs::read_dir(audit_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }

            let file_name = entry.file_name().to_string_lossy().into_owned();
            let contents = fs::read_to_string(&path)?;
            let parsed = serde_json::from_str::<Value>(&contents);

            let (timestamp, action, outcome, parse_error) = match parsed {
                Ok(value) => (
                    json_string(&value, "timestamp"),
                    json_string(&value, "action"),
                    json_string(&value, "outcome"),
                    None,
                ),
                Err(error) => (None, None, None, Some(error.to_string())),
            };

            entries.push(AuditSummaryEntry {
                file_name,
                path: path.display().to_string(),
                timestamp,
                action,
                outcome,
                parse_error,
            });
        }

        entries.sort_by(|left, right| right.file_name.cmp(&left.file_name));
    }

    Ok(AuditSummary {
        generated_at: Local::now().to_rfc3339(),
        source_dir: audit_dir.display().to_string(),
        entries,
    })
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        fs::File,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use guardian_core::types::{DomainReport, DomainReports, HealthReport, StatusLevel};
    use zip::ZipArchive;

    use super::{
        AUDIT_SUMMARY_FILE, BundleExportResult, HEALTH_REPORT_FILE, MANIFEST_FILE,
        PROFILE_DIAGNOSIS_FILE, apply_bundle_retention, archive_path_for_bundle,
        build_retention_context, collect_audit_summary_from_dir, resolve_bundle_root,
        write_bundle_to_directory,
    };

    #[test]
    fn resolves_default_bundle_root_under_parent_directory() {
        let parent = Path::new("C:\\guardian\\bundles");
        let resolved = resolve_bundle_root(parent, None);

        assert_eq!(resolved.parent(), Some(parent));
        assert!(
            resolved
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("bundle-"))
        );
    }

    #[test]
    fn writes_bundle_payload_and_audit_summary() {
        let temp_root = unique_temp_dir("bundle-export");
        let bundle_root = temp_root.join("bundle");
        let audit_dir = temp_root.join("audits");
        fs::create_dir_all(&audit_dir).expect("create audit dir");
        fs::write(
            audit_dir.join("codex-repair-20260416-101010.json"),
            r#"{"timestamp":"2026-04-16T10:10:10+08:00","action":"guardian repair codex --confirm","outcome":"noop"}"#,
        )
        .expect("write audit record");

        let report = sample_report();
        let result = write_bundle_to_directory(
            &bundle_root,
            &audit_dir,
            &report,
            &report,
            false,
            false,
            None,
        )
        .expect("bundle export should succeed");

        assert_bundle_files_exist(&result);
        assert_eq!(result.audit_entries, 1);
        assert!(result.archive_path.is_none());
        assert!(result.retention_deleted_paths.is_empty());

        let audit_summary =
            fs::read_to_string(&result.audit_summary_path).expect("read audit summary json");
        assert!(audit_summary.contains("guardian repair codex --confirm"));
        assert!(audit_summary.contains("codex-repair-20260416-101010.json"));

        let manifest =
            fs::read_to_string(&result.manifest_path).expect("read bundle manifest json");
        assert!(manifest.contains(HEALTH_REPORT_FILE));
        assert!(manifest.contains(PROFILE_DIAGNOSIS_FILE));
        assert!(manifest.contains(AUDIT_SUMMARY_FILE));
        assert!(manifest.contains(MANIFEST_FILE));
        assert!(manifest.contains("\"used_explicit_output\": false"));
        assert!(manifest.contains("\"archive_path\": null"));
    }

    #[test]
    fn writes_zip_archive_when_requested() {
        let temp_root = unique_temp_dir("bundle-export-zip");
        let bundle_root = temp_root.join("bundle-20260417-010101");
        let audit_dir = temp_root.join("audits");
        fs::create_dir_all(&audit_dir).expect("create audit dir");

        let report = sample_report();
        let result =
            write_bundle_to_directory(&bundle_root, &audit_dir, &report, &report, true, true, None)
                .expect("bundle export should succeed");

        let archive_path = result.archive_path.expect("zip archive should exist");
        assert!(archive_path.exists());

        let archive_file = File::open(&archive_path).expect("open archive");
        let mut archive = ZipArchive::new(archive_file).expect("parse archive");
        let mut names = (0..archive.len())
            .map(|index| {
                archive
                    .by_index(index)
                    .expect("zip entry")
                    .name()
                    .to_string()
            })
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                AUDIT_SUMMARY_FILE.to_string(),
                MANIFEST_FILE.to_string(),
                HEALTH_REPORT_FILE.to_string(),
                PROFILE_DIAGNOSIS_FILE.to_string(),
            ]
        );

        let manifest =
            fs::read_to_string(&result.manifest_path).expect("read bundle manifest json");
        let manifest_json =
            serde_json::from_str::<serde_json::Value>(&manifest).expect("parse manifest json");
        assert_eq!(
            manifest_json
                .get("archive_path")
                .and_then(|value| value.as_str()),
            Some(archive_path.display().to_string().as_str())
        );
    }

    #[test]
    fn rejects_file_path_as_bundle_root() {
        let temp_root = unique_temp_dir("bundle-export-file");
        let bundle_root = temp_root.join("bundle-output.json");
        let audit_dir = temp_root.join("audits");
        fs::create_dir_all(&audit_dir).expect("create audit dir");
        fs::write(&bundle_root, "{}").expect("create file collision");

        let report = sample_report();
        let error = write_bundle_to_directory(
            &bundle_root,
            &audit_dir,
            &report,
            &report,
            true,
            false,
            None,
        )
        .expect_err("file collision should be rejected");

        assert!(error.to_string().contains("already exists as a file"));
    }

    #[test]
    fn retains_current_bundle_family_and_newest_others() {
        let temp_root = unique_temp_dir("bundle-retention");
        let parent = temp_root.join("managed");
        fs::create_dir_all(&parent).expect("create parent");
        let audit_dir = temp_root.join("audits");
        fs::create_dir_all(&audit_dir).expect("create audit dir");

        create_bundle_family(&parent, "bundle-20260417-000001", true);
        create_bundle_family(&parent, "bundle-20260417-000002", true);

        let bundle_root = parent.join("bundle-20260417-000003");
        let report = sample_report();
        let result = write_bundle_to_directory(
            &bundle_root,
            &audit_dir,
            &report,
            &report,
            true,
            true,
            Some(2),
        )
        .expect("bundle export with retention should succeed");

        assert!(parent.join("bundle-20260417-000003").exists());
        assert!(parent.join("bundle-20260417-000003.zip").exists());
        assert!(parent.join("bundle-20260417-000002").exists());
        assert!(parent.join("bundle-20260417-000002.zip").exists());
        assert!(!parent.join("bundle-20260417-000001").exists());
        assert!(!parent.join("bundle-20260417-000001.zip").exists());
        assert_eq!(result.retention_limit, Some(2));
        assert_eq!(result.retention_kept_family_count, 2);
        assert_eq!(result.retention_deleted_paths.len(), 2);
    }

    #[test]
    fn retain_requires_bundle_family_name() {
        let temp_root = unique_temp_dir("bundle-retention-invalid");
        let bundle_root = temp_root.join("custom-output");
        let error = build_retention_context(&bundle_root, Some(2))
            .expect_err("custom output root should be rejected");

        assert!(error.to_string().contains("`--retain` requires"));
    }

    #[test]
    fn retain_zero_is_rejected() {
        let temp_root = unique_temp_dir("bundle-retention-zero");
        let bundle_root = temp_root.join("bundle-20260417-000011");
        let error = build_retention_context(&bundle_root, Some(0))
            .expect_err("zero retention should be rejected");

        assert!(error.to_string().contains("greater than zero"));
    }

    #[test]
    fn retention_context_tracks_bundle_parent() {
        let temp_root = unique_temp_dir("bundle-retention-context");
        let bundle_root = temp_root.join("bundle-20260417-000010");
        let context = build_retention_context(&bundle_root, Some(3))
            .expect("context should build")
            .expect("retention context should exist");

        assert_eq!(context.limit, 3);
        assert_eq!(context.parent, temp_root);
        assert_eq!(context.current_family, "bundle-20260417-000010");
    }

    #[test]
    fn archive_path_uses_bundle_name_with_zip_extension() {
        let bundle_root = Path::new("C:\\guardian\\bundles\\bundle-20260417-000010");
        let archive_path = archive_path_for_bundle(bundle_root).expect("archive path");

        assert_eq!(
            archive_path,
            PathBuf::from("C:\\guardian\\bundles\\bundle-20260417-000010.zip")
        );
    }

    #[test]
    fn applies_empty_retention_when_directory_is_missing() {
        let temp_root = unique_temp_dir("bundle-retention-missing");
        let parent = temp_root.join("missing-parent");
        let context = build_retention_context(&parent.join("bundle-20260417-000020"), Some(2))
            .expect("context")
            .expect("retention context");
        let result = apply_bundle_retention(Some(context)).expect("retention result");

        assert_eq!(result.kept_family_count, 1);
        assert!(result.deleted_paths.is_empty());
    }

    #[test]
    fn collects_empty_audit_summary_when_directory_is_missing() {
        let temp_root = unique_temp_dir("bundle-export-missing-audits");
        let audit_dir = temp_root.join("missing-audits");
        let summary = collect_audit_summary_from_dir(&audit_dir).expect("collect audit summary");

        assert!(summary.entries.is_empty());
        assert_eq!(summary.source_dir, audit_dir.display().to_string());
    }

    fn create_bundle_family(parent: &Path, family_name: &str, with_archive: bool) {
        let bundle_dir = parent.join(family_name);
        fs::create_dir_all(&bundle_dir).expect("create bundle dir");
        fs::write(bundle_dir.join(HEALTH_REPORT_FILE), "{}").expect("write placeholder");
        if with_archive {
            fs::write(parent.join(format!("{family_name}.zip")), "zip").expect("write archive");
        }
    }

    fn assert_bundle_files_exist(result: &BundleExportResult) {
        assert!(result.bundle_root.is_dir());
        assert!(result.health_report_path.exists());
        assert!(result.profile_diagnosis_path.exists());
        assert!(result.audit_summary_path.exists());
        assert!(result.manifest_path.exists());
        assert!(result.manifest_path.ends_with(MANIFEST_FILE));
    }

    fn sample_report() -> HealthReport {
        HealthReport::new(
            "2026-04-16T00:00:00+08:00".to_string(),
            DomainReports {
                codex: DomainReport::new(StatusLevel::Ok, "codex ok", Vec::new(), Vec::new()),
                docker_wsl: DomainReport::new(StatusLevel::Ok, "docker ok", Vec::new(), Vec::new()),
                profile: DomainReport::new(
                    StatusLevel::Warn,
                    "profile warning",
                    Vec::new(),
                    vec!["guided recovery".to_string()],
                ),
            },
            Vec::new(),
            vec!["bundle export note".to_string()],
        )
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("guardian-{prefix}-{unique}"));
        fs::create_dir_all(&root).expect("create temp root");
        root
    }
}
