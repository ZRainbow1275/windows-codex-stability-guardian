use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::Local;
use guardian_core::{
    GuardianError,
    audit::{DockerRepairAuditRecord, RepairAuditRecord},
    types::{ActionPlan, DomainReports, HealthReport, StatusLevel},
};
use guardian_observers::{codex, docker_wsl, profile};
use guardian_repair::{
    bundle::{self, BundleExportOptions},
    codex as codex_repair, docker_wsl as docker_repair,
};
use guardian_windows::paths::guardian_audit_dir;
use tracing::info;

use crate::{
    cli::{
        CheckArgs, Cli, Command, DiagnoseArgs, DiagnoseTarget, ExportArgs, ExportTarget,
        GlobalArgs, GuiArgs, RepairArgs, RepairTarget, TrayArgs,
    },
    gui, tray,
};

pub fn run(cli: Cli) -> Result<i32, GuardianError> {
    match cli.command {
        Command::Check(args) => handle_check(&cli.global, args),
        Command::Repair(args) => handle_repair(&cli.global, args),
        Command::Diagnose(args) => handle_diagnose(&cli.global, args),
        Command::Export(args) => handle_export(&cli.global, args),
        Command::Gui(args) => handle_gui(&cli.global, args),
        Command::Tray(args) => handle_tray(&cli.global, args),
    }
}

fn handle_check(global: &GlobalArgs, _args: CheckArgs) -> Result<i32, GuardianError> {
    let report = build_health_report()?;
    emit_report(global, &report)?;
    Ok(exit_code_for_check(report.status))
}

fn handle_repair(global: &GlobalArgs, args: RepairArgs) -> Result<i32, GuardianError> {
    match args.target {
        RepairTarget::Codex(args) => {
            let mut report = build_focused_codex_report(args.project_path.as_deref())?;
            report.actions = codex_repair::planned_actions(args.project_path.as_deref());

            if !global.dry_run && !global.confirm {
                report.notes.push(
                    "Codex repair is gated behind `--confirm`; use `--dry-run` to preview the managed repair chain or `--confirm` to execute it with backup, verification, and audit."
                        .to_string(),
                );
                emit_report(global, &report)?;
                return Ok(5);
            }

            if global.dry_run {
                report.notes.push(
                    "Dry-run only: the managed Codex repair chain did not modify the environment."
                        .to_string(),
                );
                emit_report(global, &report)?;
                return Ok(if report.domains.codex.status == StatusLevel::Ok {
                    0
                } else {
                    2
                });
            }

            let execution = codex_repair::execute_confirmed(args.project_path.as_deref())?;
            let audit_path = persist_codex_repair_audit(&execution)?;
            report = build_focused_codex_report(args.project_path.as_deref())?;
            report.actions = codex_repair::planned_actions(args.project_path.as_deref());
            report.domains.codex.summary = execution.outcome_summary();
            report
                .domains
                .codex
                .evidence
                .push(guardian_core::types::EvidenceItem::new(
                    "repair_outcome",
                    execution.outcome.as_str().to_string(),
                ));
            if let Some(state_db_path) = &execution.state_db_path {
                report
                    .domains
                    .codex
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_state_db",
                        state_db_path.display().to_string(),
                    ));
            }
            if let Some(stale_rows_before) = execution.stale_rows_before {
                report
                    .domains
                    .codex
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_stale_before",
                        stale_rows_before.to_string(),
                    ));
            }
            if let Some(stale_rows_after) = execution.stale_rows_after {
                report
                    .domains
                    .codex
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_stale_after",
                        stale_rows_after.to_string(),
                    ));
            }
            report
                .domains
                .codex
                .evidence
                .push(guardian_core::types::EvidenceItem::new(
                    "repair_audit_path",
                    audit_path.display().to_string(),
                ));
            if let Some(backup_path) = &execution.backup_path {
                report
                    .domains
                    .codex
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_backup_path",
                        backup_path.display().to_string(),
                    ));
            }
            if let Some(trust_repair) = &execution.trust_repair {
                report
                    .domains
                    .codex
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_trust_target_path",
                        trust_repair.target_project_path.display().to_string(),
                    ));
                report
                    .domains
                    .codex
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_trust_target_source",
                        trust_repair.target_source.clone(),
                    ));
                report
                    .domains
                    .codex
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_trust_config_path",
                        trust_repair.config_path.display().to_string(),
                    ));
                report
                    .domains
                    .codex
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_trust_created_config",
                        trust_repair.created_config.to_string(),
                    ));
                if let Some(config_backup_path) = &trust_repair.config_backup_path {
                    report
                        .domains
                        .codex
                        .evidence
                        .push(guardian_core::types::EvidenceItem::new(
                            "repair_trust_config_backup_path",
                            config_backup_path.display().to_string(),
                        ));
                }
                if !trust_repair.missing_keys_before.is_empty() {
                    report
                        .domains
                        .codex
                        .evidence
                        .push(guardian_core::types::EvidenceItem::new(
                            "repair_trust_missing_keys_before",
                            trust_repair.missing_keys_before.join(" | "),
                        ));
                }
                if !trust_repair.added_keys.is_empty() {
                    report
                        .domains
                        .codex
                        .evidence
                        .push(guardian_core::types::EvidenceItem::new(
                            "repair_trust_added_keys",
                            trust_repair.added_keys.join(" | "),
                        ));
                }
            }
            report.domains.codex.notes.extend(execution.notes());
            report.notes.push(format!(
                "Codex confirm mode executed the managed repair chain and persisted audit to {}",
                audit_path.display()
            ));

            emit_report(global, &report)?;
            Ok(
                if execution.is_successful() && report.domains.codex.status == StatusLevel::Ok {
                    0
                } else if execution.is_successful() {
                    2
                } else {
                    4
                },
            )
        }
        RepairTarget::Docker => {
            let base_report = build_health_report()?;
            let mut report = focused_docker_report(&base_report);
            report.actions = docker_repair::planned_actions();

            if !global.dry_run && !global.confirm {
                report.notes.push(
                    "Docker / WSL repair is gated behind `--confirm`; use `--dry-run` to preview the managed `.wslconfig` baseline repair chain."
                        .to_string(),
                );
                emit_report(global, &report)?;
                return Ok(5);
            }

            if global.dry_run {
                report.notes.push(
                    "Dry-run only: no Docker / WSL repair action was executed and no `.wslconfig` changes were applied."
                        .to_string(),
                );
                emit_report(global, &report)?;
                return Ok(if report.domains.docker_wsl.status == StatusLevel::Ok {
                    0
                } else {
                    2
                });
            }

            let execution = docker_repair::execute_confirmed()?;
            let audit_path = persist_docker_repair_audit(&execution)?;
            let refreshed_report = build_health_report()?;
            report = focused_docker_report(&refreshed_report);
            report.actions = docker_repair::planned_actions();
            report.domains.docker_wsl.summary = execution.outcome_summary();
            report.domains.docker_wsl.evidence.extend([
                guardian_core::types::EvidenceItem::new(
                    "repair_outcome",
                    execution.outcome.as_str().to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_wslconfig_path",
                    execution.wslconfig_path.display().to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_created_file",
                    execution.created_file.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_restart_required",
                    execution.restart_required.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_anomaly_detected",
                    execution.runtime_anomaly_detected.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_anomaly_after",
                    execution.runtime_anomaly_after.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_repair_attempted",
                    execution.runtime_repair_attempted.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_repair_blocked",
                    execution.runtime_repair_blocked.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_wait_poll_count",
                    execution.runtime_wait_poll_count.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_wait_stable_success_polls",
                    execution.runtime_wait_stable_success_polls.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_wait_timed_out",
                    execution.runtime_wait_timed_out.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_wait_elapsed_ms",
                    execution.runtime_wait_elapsed_ms.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_missing_before",
                    if execution.missing_keys_before.is_empty() {
                        "none".to_string()
                    } else {
                        execution.missing_keys_before.join(",")
                    },
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_missing_after",
                    if execution.missing_keys_after.is_empty() {
                        "none".to_string()
                    } else {
                        execution.missing_keys_after.join(",")
                    },
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_baseline_memory",
                    execution.baseline.memory_value(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_baseline_processors",
                    execution.baseline.processors.to_string(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_baseline_swap",
                    execution.baseline.swap_value(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_baseline_auto_memory_reclaim",
                    execution.baseline.auto_memory_reclaim.clone(),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_runtime_action",
                    execution
                        .runtime_action
                        .clone()
                        .unwrap_or_else(|| "none".to_string()),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_running_containers_before",
                    execution
                        .running_containers_before
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_docker_desktop_status_before",
                    execution
                        .docker_desktop_status_before
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_docker_desktop_status_after",
                    execution
                        .docker_desktop_status_after
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                ),
                guardian_core::types::EvidenceItem::new(
                    "repair_audit_path",
                    audit_path.display().to_string(),
                ),
            ]);
            if let Some(backup_path) = &execution.backup_path {
                report
                    .domains
                    .docker_wsl
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_backup_path",
                        backup_path.display().to_string(),
                    ));
            }
            if let Some(block_reason) = &execution.runtime_block_reason {
                report
                    .domains
                    .docker_wsl
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_runtime_block_reason",
                        block_reason.clone(),
                    ));
            }
            if let Some(runtime_error) = &execution.runtime_error {
                report
                    .domains
                    .docker_wsl
                    .evidence
                    .push(guardian_core::types::EvidenceItem::new(
                        "repair_runtime_error",
                        runtime_error.clone(),
                    ));
            }
            report.domains.docker_wsl.notes.extend(execution.notes());
            report.notes.push(format!(
                "Docker confirm mode completed the guarded Docker / WSL repair flow and persisted audit to {}",
                audit_path.display()
            ));

            emit_report(global, &report)?;
            Ok(
                if execution.is_successful() && report.domains.docker_wsl.status == StatusLevel::Ok
                {
                    0
                } else if execution.is_successful() {
                    2
                } else {
                    4
                },
            )
        }
    }
}

fn handle_diagnose(global: &GlobalArgs, args: DiagnoseArgs) -> Result<i32, GuardianError> {
    match args.target {
        DiagnoseTarget::Profile(profile_args) => {
            let report = build_health_report()?;
            let output_note = Some(
                profile_args
                    .output
                    .as_deref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<stdout>".to_string()),
            );
            let mut profile_report = build_profile_diagnosis_report(&report, output_note);
            if let Some(path) = &profile_args.output {
                let json = serde_json::to_string_pretty(&profile_report)?;
                fs::write(path, json)?;
                profile_report.notes.push(format!(
                    "Profile diagnosis JSON was written to {}",
                    path.display()
                ));
            }
            emit_report(global, &profile_report)?;
            Ok(exit_code_for_check(profile_report.status))
        }
    }
}

fn build_focused_codex_report(project_path: Option<&Path>) -> Result<HealthReport, GuardianError> {
    Ok(HealthReport::new(
        timestamp(),
        DomainReports::single_codex(codex::observe_with_target(project_path)?),
        Vec::new(),
        vec![
            "Codex confirm repair is live, trust drift detection is live, Docker D3 managed repair is live, guarded Docker/WSL runtime restart recovery for D1/D2/D4 is live when the machine can prove zero running containers, and profile event collection is live in read-only mode.".to_string(),
        ],
    ))
}

fn focused_docker_report(report: &HealthReport) -> HealthReport {
    HealthReport::new(
        timestamp(),
        DomainReports::single_docker_wsl(report.domains.docker_wsl.clone()),
        Vec::new(),
        report.notes.clone(),
    )
}

fn build_profile_diagnosis_report(
    report: &HealthReport,
    requested_output: Option<String>,
) -> HealthReport {
    let mut notes = vec!["Profile diagnostics stay read-only in all modes.".to_string()];
    if let Some(output) = requested_output {
        notes.push(format!("Requested output path: {output}"));
    }

    let mut profile_report = HealthReport::new(
        timestamp(),
        DomainReports::single_profile(report.domains.profile.clone()),
        vec![ActionPlan::new(
            "guardian diagnose profile --json".to_string(),
            "Emit the current profile diagnosis in JSON for later automation.".to_string(),
            false,
        )],
        notes,
    );
    append_profile_guided_recovery(&mut profile_report);
    profile_report
}

fn handle_export(global: &GlobalArgs, args: ExportArgs) -> Result<i32, GuardianError> {
    match args.target {
        ExportTarget::Bundle(bundle_args) => {
            let mut report = build_health_report()?;
            let profile_report = build_profile_diagnosis_report(&report, None);
            let execution = bundle::export_bundle(
                &report,
                &profile_report,
                &BundleExportOptions {
                    output_root: bundle_args.output.clone(),
                    create_zip_archive: bundle_args.zip,
                    retention_limit: bundle_args.retain,
                },
            )?;

            report.notes.push(format!(
                "Bundle export wrote diagnostic files to {}",
                execution.bundle_root.display()
            ));
            report.notes.push(format!(
                "Bundle manifest: {}",
                execution.manifest_path.display()
            ));
            report.notes.push(format!(
                "Bundle files: {}, {}, {}, {}",
                execution.health_report_path.display(),
                execution.profile_diagnosis_path.display(),
                execution.audit_summary_path.display(),
                execution.manifest_path.display()
            ));
            report.notes.push(format!(
                "Bundle audit summary captured {} existing audit record(s).",
                execution.audit_entries
            ));
            if let Some(archive_path) = &execution.archive_path {
                report.notes.push(format!(
                    "Bundle export also wrote zip archive to {}",
                    archive_path.display()
                ));
            }
            if execution.used_explicit_output {
                report.notes.push(
                    "Bundle output root came from the explicit `--output` argument.".to_string(),
                );
            } else {
                report.notes.push(
                    "Bundle output root used the default `%LOCALAPPDATA%\\guardian\\bundles\\bundle-YYYYMMDD-HHMMSS` location."
                        .to_string(),
                );
            }
            if let Some(limit) = execution.retention_limit {
                let scope = execution
                    .retention_parent
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                report.notes.push(format!(
                    "Bundle retention kept {} bundle family/families under {scope}, always including the current export (requested limit: {limit}).",
                    execution.retention_kept_family_count
                ));
                if execution.retention_deleted_paths.is_empty() {
                    report
                        .notes
                        .push("Bundle retention removed no older bundle artifacts.".to_string());
                } else {
                    report.notes.push(format!(
                        "Bundle retention removed {} older artifact(s): {}",
                        execution.retention_deleted_paths.len(),
                        execution
                            .retention_deleted_paths
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }

            emit_report(global, &report)?;
            Ok(exit_code_for_check(report.status))
        }
    }
}

fn handle_gui(global: &GlobalArgs, _args: GuiArgs) -> Result<i32, GuardianError> {
    if global.json || global.quiet || global.dry_run || global.confirm {
        return Err(GuardianError::invalid_state(
            "`guardian gui` does not accept global execution flags; use the window actions instead",
        ));
    }

    gui::run_gui()
}

fn handle_tray(global: &GlobalArgs, _args: TrayArgs) -> Result<i32, GuardianError> {
    if global.json {
        return Err(GuardianError::invalid_state(
            "`guardian tray --json` is not supported because tray mode runs as a long-lived UI loop",
        ));
    }

    tray::run_tray()
}

fn build_health_report() -> Result<HealthReport, GuardianError> {
    info!("running guardian health report");

    let codex_report = codex::observe()?;
    let docker_report = docker_wsl::observe()?;
    let profile_report = profile::observe()?;

    let mut actions = Vec::new();
    if codex_report.status != StatusLevel::Ok {
        actions.extend(codex_repair::planned_actions(None).into_iter().take(1));
    }
    if docker_report.status != StatusLevel::Ok {
        actions.push(ActionPlan::new(
            "guardian repair docker --dry-run".to_string(),
            "Preview the Docker and WSL recovery chain.".to_string(),
            false,
        ));
    }
    if profile_report.status != StatusLevel::Ok {
        actions.push(ActionPlan::new(
            "guardian diagnose profile --json".to_string(),
            "Export profile diagnostics without modifying the system.".to_string(),
            false,
        ));
    }

    Ok(HealthReport::new(
        timestamp(),
        DomainReports {
            codex: codex_report,
            docker_wsl: docker_report,
            profile: profile_report,
        },
        actions,
        vec![
            "Codex confirm repair is live, trust drift detection is live, Docker D3 managed repair is live, guarded Docker/WSL runtime restart recovery for D1/D2/D4 is live when the machine can prove zero running containers, and profile event collection is live in read-only mode.".to_string(),
        ],
    ))
}

fn persist_codex_repair_audit(
    execution: &codex_repair::CodexRepairExecution,
) -> Result<PathBuf, GuardianError> {
    let audit_dir = guardian_audit_dir().map_err(GuardianError::Io)?;
    fs::create_dir_all(&audit_dir)?;

    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let audit_path = audit_dir.join(format!("codex-repair-{timestamp}.json"));
    let audit_record = RepairAuditRecord {
        timestamp: Local::now().to_rfc3339(),
        action: "guardian repair codex --confirm".to_string(),
        outcome: execution.outcome.as_str().to_string(),
        state_db_path: execution
            .state_db_path
            .as_ref()
            .map(|path| path.display().to_string()),
        stale_rows_before: execution.stale_rows_before,
        stale_rows_after: execution.stale_rows_after,
        active_version: execution.active_version.clone(),
        backup_path: execution
            .backup_path
            .as_ref()
            .map(|path| path.display().to_string()),
        stdout_excerpt: execution.stdout_excerpt.clone(),
        stderr_excerpt: execution.stderr_excerpt.clone(),
        trust_target_path: execution
            .trust_repair
            .as_ref()
            .map(|repair| repair.target_project_path.display().to_string()),
        trust_target_source: execution
            .trust_repair
            .as_ref()
            .map(|repair| repair.target_source.clone()),
        trust_config_path: execution
            .trust_repair
            .as_ref()
            .map(|repair| repair.config_path.display().to_string()),
        trust_config_backup_path: execution
            .trust_repair
            .as_ref()
            .and_then(|repair| repair.config_backup_path.as_ref())
            .map(|path| path.display().to_string()),
        trust_missing_keys_before: execution
            .trust_repair
            .as_ref()
            .map(|repair| repair.missing_keys_before.clone())
            .unwrap_or_default(),
        trust_added_keys: execution
            .trust_repair
            .as_ref()
            .map(|repair| repair.added_keys.clone())
            .unwrap_or_default(),
    };

    fs::write(&audit_path, serde_json::to_string_pretty(&audit_record)?)?;
    Ok(audit_path)
}

fn persist_docker_repair_audit(
    execution: &docker_repair::DockerRepairExecution,
) -> Result<PathBuf, GuardianError> {
    let audit_dir = guardian_audit_dir().map_err(GuardianError::Io)?;
    fs::create_dir_all(&audit_dir)?;

    let timestamp = Local::now().format("%Y%m%d-%H%M%S").to_string();
    let audit_path = audit_dir.join(format!("docker-repair-{timestamp}.json"));
    let audit_record = DockerRepairAuditRecord {
        timestamp: Local::now().to_rfc3339(),
        action: "guardian repair docker --confirm".to_string(),
        outcome: execution.outcome.as_str().to_string(),
        wslconfig_path: execution.wslconfig_path.display().to_string(),
        backup_path: execution
            .backup_path
            .as_ref()
            .map(|path| path.display().to_string()),
        created_file: execution.created_file,
        restart_required: execution.restart_required,
        runtime_anomaly_detected: execution.runtime_anomaly_detected,
        runtime_anomaly_after: execution.runtime_anomaly_after,
        runtime_repair_attempted: execution.runtime_repair_attempted,
        runtime_repair_blocked: execution.runtime_repair_blocked,
        runtime_action: execution.runtime_action.clone(),
        runtime_block_reason: execution.runtime_block_reason.clone(),
        runtime_error: execution.runtime_error.clone(),
        runtime_wait_poll_count: execution.runtime_wait_poll_count,
        runtime_wait_stable_success_polls: execution.runtime_wait_stable_success_polls,
        runtime_wait_timed_out: execution.runtime_wait_timed_out,
        runtime_wait_elapsed_ms: execution.runtime_wait_elapsed_ms,
        running_containers_before: execution.running_containers_before,
        docker_desktop_status_before: execution.docker_desktop_status_before.clone(),
        docker_desktop_status_after: execution.docker_desktop_status_after.clone(),
        missing_keys_before: execution.missing_keys_before.clone(),
        missing_keys_after: execution.missing_keys_after.clone(),
        baseline_memory: execution.baseline.memory_value(),
        baseline_processors: execution.baseline.processors,
        baseline_swap: execution.baseline.swap_value(),
        baseline_auto_memory_reclaim: execution.baseline.auto_memory_reclaim.clone(),
    };

    fs::write(&audit_path, serde_json::to_string_pretty(&audit_record)?)?;
    Ok(audit_path)
}

fn append_profile_guided_recovery(report: &mut HealthReport) {
    let failure_classes = domain_failure_classes(&report.domains.profile);
    if failure_classes.is_empty() {
        return;
    }

    report.domains.profile.evidence.extend([
        guardian_core::types::EvidenceItem::new("guided_recovery_mode", "true"),
        guardian_core::types::EvidenceItem::new(
            "guided_recovery_failure_classes",
            failure_classes.join(","),
        ),
    ]);

    report.notes.push(
        "Profile diagnostics remain read-only by design; Guardian will not modify `ProfileList`, terminate security software, or attempt direct registry repair."
            .to_string(),
    );

    for step in profile_guided_recovery_steps(&report.domains.profile, &failure_classes) {
        report.notes.push(step);
    }
}

fn profile_guided_recovery_steps(
    report: &guardian_core::types::DomainReport,
    failure_classes: &[String],
) -> Vec<String> {
    let mut steps = Vec::new();
    let locking_process = domain_evidence_value(report, "locking_process_name")
        .unwrap_or("the locking process captured in Event 1552");

    if failure_classes.iter().any(|class| class == "P4") {
        steps.push(format!(
            "Guided recovery step 1: review whether `{locking_process}` should be handled through an administrator-managed exclusion or policy adjustment; Guardian will not terminate or modify security software automatically."
        ));
    }

    if failure_classes
        .iter()
        .any(|class| matches!(class.as_str(), "P1" | "P2" | "P3"))
    {
        steps.push(
            "Guided recovery step 2: review Windows Fast Startup before the next reproduction because hybrid shutdown can keep hive-lock side effects alive across reboots."
                .to_string(),
        );
        steps.push(
            "Guided recovery step 3: if the issue reproduces again, test under clean boot or a narrowed security policy so you can separate User Profile Service failures from third-party lock contention."
                .to_string(),
        );
        steps.push(
            "Guided recovery step 4: back up the affected Windows profile before any manual `ProfileList` or `.bak` registry work."
                .to_string(),
        );
        steps.push(
            "Guided recovery step 5: only after evidence capture and profile backup, move to guided manual registry recovery; Guardian V1 intentionally refuses to edit `ProfileList` automatically."
                .to_string(),
        );
    }

    if steps.is_empty() {
        steps.push(
            "Guided recovery step 1: collect a fresh profile diagnosis bundle before changing any Windows profile or security-software state."
                .to_string(),
        );
    }

    steps
}

fn domain_failure_classes(report: &guardian_core::types::DomainReport) -> Vec<String> {
    domain_evidence_value(report, "failure_classes")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty() && *item != "none")
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
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

fn emit_report(global: &GlobalArgs, report: &HealthReport) -> Result<(), GuardianError> {
    if global.json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }

    if global.quiet {
        println!("{}", report.status.as_str().to_uppercase());
        return Ok(());
    }

    println!("Guardian status: {}", report.status.as_str().to_uppercase());
    println!("Generated at: {}", report.timestamp);
    println!();

    print_domain("codex", &report.domains.codex);
    print_domain("docker_wsl", &report.domains.docker_wsl);
    print_domain("profile", &report.domains.profile);

    if !report.actions.is_empty() {
        println!("Actions:");
        for action in &report.actions {
            let confirm_suffix = if action.requires_confirmation {
                " (requires confirmation)"
            } else {
                ""
            };
            println!(
                "  - {}{}: {}",
                action.command, confirm_suffix, action.description
            );
        }
        println!();
    }

    if !report.notes.is_empty() {
        println!("Notes:");
        for note in &report.notes {
            println!("  - {note}");
        }
    }

    Ok(())
}

fn print_domain(name: &str, report: &guardian_core::types::DomainReport) {
    println!(
        "[{}] {} - {}",
        name,
        report.status.as_str().to_uppercase(),
        report.summary
    );

    for item in &report.evidence {
        println!("  * {}: {}", item.key, item.value);
    }
    for note in &report.notes {
        println!("  - {note}");
    }
    println!();
}

fn timestamp() -> String {
    Local::now().to_rfc3339()
}

fn exit_code_for_check(status: StatusLevel) -> i32 {
    match status {
        StatusLevel::Ok => 0,
        StatusLevel::Warn => 2,
        StatusLevel::Fail => 3,
    }
}
