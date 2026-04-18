use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use chrono::Local;
use guardian_core::{
    GuardianError,
    types::{HealthReport, StatusLevel},
};
use guardian_windows::{paths::guardian_audit_dir, process::decode_output};

pub(crate) const GUARDIAN_PRODUCT_NAME_ZH: &str = "Guardian 稳定性控制台";

pub(crate) struct JsonCommandSuccess {
    pub exit_code: i32,
    pub report: HealthReport,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) fn run_health_report_command(
    current_exe: &Path,
    args: &[OsString],
    action_label: &str,
) -> Result<JsonCommandSuccess, GuardianError> {
    let output = Command::new(current_exe)
        .args(args)
        .output()
        .map_err(GuardianError::Io)?;
    let stdout = decode_output(&output.stdout).trim().to_string();
    let stderr = decode_output(&output.stderr).trim().to_string();
    let exit_code = output.status.code().unwrap_or(-1);

    let report = serde_json::from_str::<HealthReport>(&stdout).map_err(|error| {
        let stderr_detail = if stderr.is_empty() {
            "<empty>".to_string()
        } else {
            stderr.clone()
        };
        GuardianError::invalid_state(format!(
            "action `{action_label}` returned unparsable JSON (exit={exit_code}, stderr={stderr_detail}): {error}"
        ))
    })?;

    Ok(JsonCommandSuccess {
        exit_code,
        report,
        stdout,
        stderr,
    })
}

pub(crate) fn open_path(path: &Path) -> Result<(), GuardianError> {
    Command::new("explorer")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(GuardianError::Io)
}

pub(crate) fn bundle_root_from_notes(notes: &[String]) -> Option<PathBuf> {
    notes
        .iter()
        .find_map(|note| note.strip_prefix("Bundle export wrote diagnostic files to "))
        .map(PathBuf::from)
}

pub(crate) fn bundle_archive_from_notes(notes: &[String]) -> Option<PathBuf> {
    notes
        .iter()
        .find_map(|note| note.strip_prefix("Bundle export also wrote zip archive to "))
        .map(PathBuf::from)
}

pub(crate) fn profile_diagnosis_from_notes(notes: &[String]) -> Option<PathBuf> {
    notes
        .iter()
        .find_map(|note| note.strip_prefix("Profile diagnosis JSON was written to "))
        .map(PathBuf::from)
}

pub(crate) fn build_profile_diagnosis_output_path() -> Result<PathBuf, GuardianError> {
    let output = guardian_audit_dir()
        .map_err(GuardianError::Io)?
        .join(format!(
            "profile-diagnosis-{}.json",
            Local::now().format("%Y%m%d-%H%M%S")
        ));
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(GuardianError::Io)?;
    }
    Ok(output)
}

pub(crate) fn localized_open_detail(path: &Path) -> String {
    format!("已打开 {}", path.display())
}

pub(crate) fn status_name_zh(status: StatusLevel) -> &'static str {
    match status {
        StatusLevel::Ok => "正常",
        StatusLevel::Warn => "警告",
        StatusLevel::Fail => "失败",
    }
}

pub(crate) fn domain_title(domain: &str) -> &'static str {
    match domain {
        "codex" => "Codex 会话恢复",
        "docker_wsl" => "Docker / WSL 运行时",
        "profile" => "Windows Profile 风险",
        _ => "Guardian 诊断域",
    }
}

pub(crate) fn localized_dominant_summary(report: &HealthReport) -> String {
    [
        ("profile", &report.domains.profile),
        ("docker_wsl", &report.domains.docker_wsl),
        ("codex", &report.domains.codex),
    ]
    .into_iter()
    .find(|(_, domain)| domain.status == report.status)
    .map(|(domain, report)| localized_domain_summary(domain, &report.summary))
    .unwrap_or_else(|| "Guardian 已生成当前机器状态摘要。".to_string())
}

pub(crate) fn localized_domain_summary(domain: &str, summary: &str) -> String {
    match domain {
        "codex" => localized_codex_summary(summary),
        "docker_wsl" => localized_docker_summary(summary),
        "profile" => localized_profile_summary(summary),
        _ => summary.to_string(),
    }
}

pub(crate) fn localized_report_note(note: &str) -> String {
    if note == "Cross-domain collection was skipped for this focused command." {
        return "这是一次聚焦命令，已跳过其余诊断域的采集。".to_string();
    }
    if note == "Expected `%USERPROFILE%/.codex` to exist on this Windows machine." {
        return "期望本机存在 `%USERPROFILE%/.codex` 目录，但当前未找到。".to_string();
    }
    if let Some(version) = note
        .strip_prefix("Codex version `")
        .and_then(|value| value.strip_suffix("` matches a known picker-risk window."))
    {
        return format!("Codex 版本 `{version}` 命中了已知的 picker 风险窗口。");
    }
    if let Some(error) = note.strip_prefix("Unable to execute `codex --version`: ") {
        return format!("无法执行 `codex --version`：{error}");
    }
    if note == "`history.jsonl` is missing." {
        return "缺少 `history.jsonl`，历史记录链路不完整。".to_string();
    }
    if note == "No session files were found under `.codex/sessions`." {
        return "在 `.codex/sessions` 下未发现任何会话文件。".to_string();
    }
    if note == "Detected stale rows in the latest Codex state database." {
        return "最新 Codex state 数据库中检测到 stale rows。".to_string();
    }
    if let Some(remainder) = note.strip_prefix("Unable to inspect the latest state database `")
        && let Some((path, error)) = remainder.split_once("`: ")
    {
        return format!("无法检查最新 state 数据库 `{path}`：{error}");
    }
    if note == "No `state_*.sqlite` file was found under `.codex`." {
        return "在 `.codex` 下未找到任何 `state_*.sqlite` 文件。".to_string();
    }
    if note
        == "Recent Codex TUI log lines include `Loading sessions`, which matches the slow-path classifier."
    {
        return "最近的 Codex TUI 日志包含 `Loading sessions`，命中了 slow-path 分类。".to_string();
    }
    if note == "Recent Codex TUI log lines include configuration/access errors." {
        return "最近的 Codex TUI 日志出现了配置或访问错误。".to_string();
    }
    if note == "No `codex-tui.log` was found under the expected `.codex/log` or `.codex` locations."
    {
        return "在预期的 `.codex/log` 或 `.codex` 位置未找到 `codex-tui.log`。".to_string();
    }

    if let Some(error) = note.strip_prefix("Unable to collect `docker version`: ") {
        return format!("无法采集 `docker version`：{error}");
    }
    if let Some(error) = note.strip_prefix("Unable to collect `docker info`: ") {
        return format!("无法采集 `docker info`：{error}");
    }
    if note
        == "`wsl -l -v` did not list the `docker-desktop` distro, which matches the utility VM anomaly classifier."
    {
        return "`wsl -l -v` 未列出 `docker-desktop` 发行版，命中了 utility VM 异常分类。"
            .to_string();
    }
    if let Some(error) = note.strip_prefix("Unable to collect `wsl -l -v`: ") {
        return format!("无法采集 `wsl -l -v`：{error}");
    }
    if note == "The current `.wslconfig` is missing at least one documented baseline key." {
        return "当前 `.wslconfig` 至少缺少一个已记录的基线键。".to_string();
    }
    if note == "`.wslconfig` does not exist yet, so no WSL resource baseline is configured." {
        return "当前还不存在 `.wslconfig`，因此尚未配置 WSL 资源基线。".to_string();
    }

    if note
        == "Profile evidence points to security software involvement; Guardian must stay in guided recovery mode."
    {
        return "Profile 证据显示安全软件正在介入，本路径必须保持为引导式恢复。".to_string();
    }
    if note == "Recent User Profiles Service events indicate a registry-lock condition (P1)." {
        return "最近的用户配置文件服务事件显示存在注册表锁冲突（P1）。".to_string();
    }
    if note == "Recent profile events match the temporary-profile risk chain (P2)." {
        return "最近的 Profile 事件命中了临时配置文件风险链（P2）。".to_string();
    }
    if note == "Recent profile events include hive load or unload failures (P3)." {
        return "最近的 Profile 事件包含 hive 加载或卸载失败（P3）。".to_string();
    }

    if note == "Profile diagnostics stay read-only in all modes." {
        return "Profile 诊断在所有模式下都保持只读。".to_string();
    }
    if let Some(path) = note.strip_prefix("Requested output path: ") {
        return format!("请求写入的输出路径：{path}");
    }
    if note
        == "Profile diagnostics remain read-only by design; Guardian will not modify `ProfileList`, terminate security software, or attempt direct registry repair."
    {
        return "Profile 诊断按设计保持只读；Guardian 不会修改 `ProfileList`、不会终止安全软件，也不会尝试直接修复注册表。"
            .to_string();
    }
    if let Some(remainder) =
        note.strip_prefix("Guided recovery step 1: review whether `")
            && let Some((process_name, _)) = remainder.split_once(
                "` should be handled through an administrator-managed exclusion or policy adjustment; Guardian will not terminate or modify security software automatically.",
            )
    {
        return format!(
            "引导恢复步骤 1：请确认 `{process_name}` 是否应通过管理员维护的排除项或策略调整处理；Guardian 不会自动终止或修改安全软件。"
        );
    }
    if note
        == "Guided recovery step 2: review Windows Fast Startup before the next reproduction because hybrid shutdown can keep hive-lock side effects alive across reboots."
    {
        return "引导恢复步骤 2：下次复现前请检查 Windows Fast Startup，因为混合关机会让 hive 锁副作用跨重启保留。"
            .to_string();
    }
    if note
        == "Guided recovery step 3: if the issue reproduces again, test under clean boot or a narrowed security policy so you can separate User Profile Service failures from third-party lock contention."
    {
        return "引导恢复步骤 3：如果问题再次出现，请在干净启动或更窄的安全策略下复测，以区分用户配置文件服务故障与第三方锁竞争。"
            .to_string();
    }
    if note
        == "Guided recovery step 4: back up the affected Windows profile before any manual `ProfileList` or `.bak` registry work."
    {
        return "引导恢复步骤 4：在任何手工处理 `ProfileList` 或 `.bak` 注册表项之前，先备份受影响的 Windows Profile。"
            .to_string();
    }
    if note
        == "Guided recovery step 5: only after evidence capture and profile backup, move to guided manual registry recovery; Guardian V1 intentionally refuses to edit `ProfileList` automatically."
    {
        return "引导恢复步骤 5：只有在完成证据留存与 Profile 备份后，才进入引导式手工注册表恢复；Guardian V1 会明确拒绝自动修改 `ProfileList`。"
            .to_string();
    }
    if let Some(path) = note.strip_prefix("Profile diagnosis JSON was written to ") {
        return format!("Profile 诊断 JSON 已写入：{path}");
    }

    if let Some(path) = note.strip_prefix(
        "Codex confirm mode executed the trusted stale-row repair chain and persisted audit to ",
    ) {
        return format!("Codex 确认修复已执行可信 stale-row 修复链，并将审计结果写入：{path}");
    }
    if note == "Dry-run only: the trusted Codex repair script was not executed." {
        return "当前仅执行 dry-run：可信的 Codex 修复脚本尚未真正运行。".to_string();
    }
    if note
        == "Codex repair is gated behind `--confirm`; use `--dry-run` to preview the live repair chain or `--confirm` to execute it with backup and audit."
    {
        return "Codex 修复必须显式加上 `--confirm`；可先用 `--dry-run` 预览真实修复链，再用 `--confirm` 执行并生成备份与审计。"
            .to_string();
    }
    if let Some(path) = note.strip_prefix(
        "Docker confirm mode completed the guarded Docker / WSL repair flow and persisted audit to ",
    ) {
        return format!(
            "Docker / WSL 确认修复已完成受保护修复流，并将审计结果写入：{path}"
        );
    }
    if note
        == "Dry-run only: no Docker / WSL repair action was executed and no `.wslconfig` changes were applied."
    {
        return "当前仅执行 dry-run：尚未真正执行 Docker / WSL 修复，也没有改动 `.wslconfig`。"
            .to_string();
    }
    if note
        == "Docker / WSL repair is gated behind `--confirm`; use `--dry-run` to preview the managed `.wslconfig` baseline repair chain."
    {
        return "Docker / WSL 修复必须显式加上 `--confirm`；可先用 `--dry-run` 预览受控的 `.wslconfig` 基线修复链。"
            .to_string();
    }

    if let Some(path) = note.strip_prefix("Bundle export wrote diagnostic files to ") {
        return format!("诊断文件已导出到：{path}");
    }
    if let Some(path) = note.strip_prefix("Bundle export also wrote zip archive to ") {
        return format!("诊断压缩包已写入：{path}");
    }
    if note
        == "Codex confirm repair is live, Docker D3 managed repair is live, guarded Docker/WSL runtime restart recovery for D1/D2/D4 is live when the machine can prove zero running containers, and profile event collection is live in read-only mode."
    {
        return "Guardian 当前能力：Codex 确认修复已上线；Docker D3 托管修复已上线；当机器能证明当前没有运行中的容器时，Docker/WSL 针对 D1/D2/D4 的受保护运行时重启恢复可用；Profile 事件采集保持只读。"
            .to_string();
    }

    note.to_string()
}

pub(crate) fn localized_action_description(description: &str) -> String {
    match description {
        "Preview the low-risk Codex repair chain." => "先预览低风险的 Codex 修复链路。".to_string(),
        "Preview the Docker and WSL recovery chain." => {
            "先预览 Docker / WSL 恢复链路。".to_string()
        }
        "Export profile diagnostics without modifying the system." => {
            "导出 Profile 诊断结果，不修改系统。".to_string()
        }
        "Emit the current profile diagnosis in JSON for later automation." => {
            "以 JSON 形式导出当前 Profile 诊断，便于后续自动化处理。".to_string()
        }
        _ => description.to_string(),
    }
}

pub(crate) fn localized_detail_text(detail: &str) -> String {
    if let Some(path) = detail.strip_prefix("bundle saved to ") {
        return format!("诊断包已保存到 {path}");
    }
    if let Some(path) = detail.strip_prefix("bundle zip saved to ") {
        return format!("诊断压缩包已保存到 {path}");
    }
    if let Some(path) = detail.strip_prefix("profile diagnosis saved to ") {
        return format!("Profile 诊断结果已保存到 {path}");
    }
    detail.to_string()
}

fn localized_codex_summary(summary: &str) -> String {
    if summary == "Codex home directory is missing, so no local Codex evidence could be collected."
    {
        return "缺少 Codex 主目录，因此无法采集本机 Codex 证据。".to_string();
    }
    if let Some(remainder) = summary.strip_prefix("Collected live Codex evidence from `")
        && let Some((home, rest)) = remainder.split_once("` with ")
        && let Some((session_count, classifier_count)) = rest.split_once(" session file(s) and ")
        && let Some(classifier_count) = classifier_count.strip_suffix(" failure classifier(s).")
    {
        return format!(
            "已从 `{home}` 采集到实时 Codex 证据：{session_count} 个会话文件，{classifier_count} 个失败分类。"
        );
    }
    summary.to_string()
}

fn localized_docker_summary(summary: &str) -> String {
    if let Some(classifier_count) = summary
        .strip_prefix("Collected live Docker, WSL, and `.wslconfig` evidence with ")
        .and_then(|value| value.strip_suffix(" failure classifier(s)."))
    {
        return format!(
            "已采集 Docker、WSL 与 `.wslconfig` 的实时证据：{classifier_count} 个失败分类。"
        );
    }
    summary.to_string()
}

fn localized_profile_summary(summary: &str) -> String {
    if summary
        == "No recent critical User Profile Service events were found in Application or Operational logs."
    {
        return "Application 与 Operational 日志中未发现近期关键用户配置文件服务事件。".to_string();
    }
    if let Some(failure_classes) = summary
        .strip_prefix("Detected recent User Profile Service evidence for ")
        .and_then(|value| value.strip_suffix('.'))
    {
        return format!("检测到近期用户配置文件服务证据：{failure_classes}。");
    }
    summary.to_string()
}

#[cfg(test)]
mod tests {
    use guardian_core::types::{DomainReport, DomainReports, HealthReport, StatusLevel};

    use super::{localized_detail_text, localized_domain_summary, localized_report_note};

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

    #[test]
    fn translates_codex_summary_for_gui() {
        let summary = localized_domain_summary(
            "codex",
            "Collected live Codex evidence from `%USERPROFILE%\\.codex` with 3 session file(s) and 1 failure classifier(s).",
        );
        assert!(summary.contains("%USERPROFILE%\\.codex"));
        assert!(summary.contains("3"));
        assert!(summary.contains("1"));
    }

    #[test]
    fn translates_profile_guided_recovery_note() {
        let note = localized_report_note(
            "Guided recovery step 4: back up the affected Windows profile before any manual `ProfileList` or `.bak` registry work.",
        );
        assert!(note.contains("引导恢复步骤 4"));
        assert!(note.contains("ProfileList"));
    }

    #[test]
    fn translates_export_detail_paths() {
        let detail = localized_detail_text("bundle zip saved to C:\\guardian\\bundle.zip");
        assert_eq!(detail, "诊断压缩包已保存到 C:\\guardian\\bundle.zip");
    }
}
