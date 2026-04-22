use std::{
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    thread,
};

use guardian_core::{
    GuardianError,
    types::{HealthReport, StatusLevel},
};
use guardian_windows::paths::{guardian_audit_dir, guardian_bundle_dir, guardian_data_dir};
use tray_icon::{
    Icon, TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu},
};
use windows::{
    Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONWARNING, MB_YESNO, MESSAGEBOX_STYLE, MessageBoxW,
    },
    core::PCWSTR,
};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy},
    window::WindowId,
};

use crate::shell::{
    GUARDIAN_PRODUCT_NAME_ZH, build_profile_diagnosis_output_path, bundle_archive_from_notes,
    bundle_root_from_notes, localized_detail_text, localized_dominant_summary,
    localized_open_detail, open_path, profile_diagnosis_from_notes, run_health_report_command,
    status_name_zh,
};

const MENU_ID_STATUS: &str = "status";
const MENU_ID_LAST_ACTION: &str = "last_action";
const MENU_ID_REFRESH: &str = "refresh";
const MENU_ID_DIAGNOSE_PROFILE: &str = "diagnose_profile";
const MENU_ID_REPAIR_CODEX: &str = "repair_codex_confirm";
const MENU_ID_REPAIR_DOCKER: &str = "repair_docker_confirm";
const MENU_ID_EXPORT_BUNDLE: &str = "export_bundle";
const MENU_ID_EXPORT_BUNDLE_ZIP: &str = "export_bundle_zip";
const MENU_ID_EXPORT_BUNDLE_ZIP_RETAIN: &str = "export_bundle_zip_retain";
const MENU_ID_OPEN_LATEST_BUNDLE: &str = "open_latest_bundle";
const MENU_ID_OPEN_LATEST_BUNDLE_ZIP: &str = "open_latest_bundle_zip";
const MENU_ID_OPEN_LAST_PROFILE: &str = "open_last_profile";
const MENU_ID_OPEN_BUNDLES_ROOT: &str = "open_bundles_root";
const MENU_ID_OPEN_AUDITS_ROOT: &str = "open_audits_root";
const MENU_ID_OPEN_GUARDIAN_DATA: &str = "open_guardian_data_root";
const MENU_ID_EXIT: &str = "exit";
const TRAY_TEST_STATE_PATH_ENV: &str = "GUARDIAN_TRAY_TEST_STATE_PATH";
const TRAY_TEST_COMMAND_PATH_ENV: &str = "GUARDIAN_TRAY_TEST_COMMAND_PATH";

pub fn run_tray() -> Result<i32, GuardianError> {
    let event_loop = EventLoop::<TrayUserEvent>::with_user_event()
        .build()
        .map_err(|error| GuardianError::invalid_state(format!("构建托盘事件循环失败：{error}")))?;
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| forward_menu_event(&proxy, event)));

    let current_exe = env::current_exe().map_err(GuardianError::Io)?;
    let mut app = GuardianTrayApp::new(current_exe, event_loop.create_proxy());
    let run_result = event_loop.run_app(&mut app);

    MenuEvent::set_event_handler::<fn(MenuEvent)>(None);

    run_result
        .map_err(|error| GuardianError::invalid_state(format!("托盘事件循环执行失败：{error}")))?;

    if let Some(error) = app.startup_error {
        return Err(error);
    }

    Ok(app.exit_code)
}

fn forward_menu_event(proxy: &EventLoopProxy<TrayUserEvent>, event: MenuEvent) {
    let _ = proxy.send_event(TrayUserEvent::Menu(event));
}

#[derive(Debug)]
enum TrayUserEvent {
    Menu(MenuEvent),
    ActionFinished(Box<TrayActionResult>),
    TestCommand(TrayTestCommand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayTestCommand {
    Action(TrayAction),
    Exit,
}

#[derive(Debug)]
struct TrayActionResult {
    action: TrayAction,
    result: Result<TrayActionSuccess, String>,
}

#[derive(Debug)]
struct TrayActionSuccess {
    exit_code: i32,
    report: Option<HealthReport>,
    detail: String,
    latest_bundle_root: Option<PathBuf>,
    latest_bundle_archive: Option<PathBuf>,
    latest_profile_output: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    RefreshStatus,
    DiagnoseProfile,
    RepairCodexConfirm,
    RepairDockerConfirm,
    ExportBundle,
    ExportBundleZip,
    ExportBundleZipRetain,
    OpenLatestBundle,
    OpenLatestBundleZip,
    OpenLastProfileDiagnosis,
    OpenBundlesRoot,
    OpenAuditsRoot,
    OpenGuardianDataRoot,
}

impl TrayAction {
    fn from_menu_id(id: &MenuId) -> Option<Self> {
        match id.0.as_str() {
            MENU_ID_REFRESH => Some(Self::RefreshStatus),
            MENU_ID_DIAGNOSE_PROFILE => Some(Self::DiagnoseProfile),
            MENU_ID_REPAIR_CODEX => Some(Self::RepairCodexConfirm),
            MENU_ID_REPAIR_DOCKER => Some(Self::RepairDockerConfirm),
            MENU_ID_EXPORT_BUNDLE => Some(Self::ExportBundle),
            MENU_ID_EXPORT_BUNDLE_ZIP => Some(Self::ExportBundleZip),
            MENU_ID_EXPORT_BUNDLE_ZIP_RETAIN => Some(Self::ExportBundleZipRetain),
            MENU_ID_OPEN_LATEST_BUNDLE => Some(Self::OpenLatestBundle),
            MENU_ID_OPEN_LATEST_BUNDLE_ZIP => Some(Self::OpenLatestBundleZip),
            MENU_ID_OPEN_LAST_PROFILE => Some(Self::OpenLastProfileDiagnosis),
            MENU_ID_OPEN_BUNDLES_ROOT => Some(Self::OpenBundlesRoot),
            MENU_ID_OPEN_AUDITS_ROOT => Some(Self::OpenAuditsRoot),
            MENU_ID_OPEN_GUARDIAN_DATA => Some(Self::OpenGuardianDataRoot),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::RefreshStatus => "刷新整机检查",
            Self::DiagnoseProfile => "只读诊断 Profile",
            Self::RepairCodexConfirm => "确认修复 Codex",
            Self::RepairDockerConfirm => "确认修复 Docker / WSL",
            Self::ExportBundle => "导出诊断包",
            Self::ExportBundleZip => "导出诊断包并压缩",
            Self::ExportBundleZipRetain => "导出并保留 5 份",
            Self::OpenLatestBundle => "打开最新诊断包",
            Self::OpenLatestBundleZip => "打开最新压缩包",
            Self::OpenLastProfileDiagnosis => "打开最新 Profile 诊断",
            Self::OpenBundlesRoot => "打开诊断包目录",
            Self::OpenAuditsRoot => "打开审计目录",
            Self::OpenGuardianDataRoot => "打开 Guardian 数据目录",
        }
    }

    fn args(self) -> Option<&'static [&'static str]> {
        match self {
            Self::RefreshStatus => Some(&["check", "--json"]),
            Self::DiagnoseProfile => None,
            Self::RepairCodexConfirm => Some(&["repair", "codex", "--confirm", "--json"]),
            Self::RepairDockerConfirm => Some(&["repair", "docker", "--confirm", "--json"]),
            Self::ExportBundle => Some(&["export", "bundle", "--json"]),
            Self::ExportBundleZip => Some(&["export", "bundle", "--json", "--zip"]),
            Self::ExportBundleZipRetain => {
                Some(&["export", "bundle", "--json", "--zip", "--retain", "5"])
            }
            Self::OpenLatestBundle
            | Self::OpenLatestBundleZip
            | Self::OpenLastProfileDiagnosis
            | Self::OpenBundlesRoot
            | Self::OpenAuditsRoot
            | Self::OpenGuardianDataRoot => None,
        }
    }

    fn is_async_guarded(self) -> bool {
        matches!(
            self,
            Self::RefreshStatus
                | Self::DiagnoseProfile
                | Self::RepairCodexConfirm
                | Self::RepairDockerConfirm
                | Self::ExportBundle
                | Self::ExportBundleZip
                | Self::ExportBundleZipRetain
                | Self::OpenLatestBundle
                | Self::OpenLatestBundleZip
        )
    }

    fn is_bundle_export(self) -> bool {
        matches!(
            self,
            Self::ExportBundle | Self::ExportBundleZip | Self::ExportBundleZipRetain
        )
    }

    fn produces_bundle_archive(self) -> bool {
        matches!(self, Self::ExportBundleZip | Self::ExportBundleZipRetain)
    }

    fn requires_confirmation(self) -> bool {
        matches!(self, Self::RepairCodexConfirm | Self::RepairDockerConfirm)
    }

    fn creates_root_directory_if_missing(self) -> bool {
        matches!(
            self,
            Self::OpenBundlesRoot | Self::OpenAuditsRoot | Self::OpenGuardianDataRoot
        )
    }
}

impl TrayTestCommand {
    fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "RefreshStatus" => Some(Self::Action(TrayAction::RefreshStatus)),
            "DiagnoseProfile" => Some(Self::Action(TrayAction::DiagnoseProfile)),
            "RepairCodexConfirm" => Some(Self::Action(TrayAction::RepairCodexConfirm)),
            "RepairDockerConfirm" => Some(Self::Action(TrayAction::RepairDockerConfirm)),
            "ExportBundle" => Some(Self::Action(TrayAction::ExportBundle)),
            "ExportBundleZip" => Some(Self::Action(TrayAction::ExportBundleZip)),
            "ExportBundleZipRetain" => Some(Self::Action(TrayAction::ExportBundleZipRetain)),
            "OpenLatestBundle" => Some(Self::Action(TrayAction::OpenLatestBundle)),
            "OpenLatestBundleZip" => Some(Self::Action(TrayAction::OpenLatestBundleZip)),
            "OpenLastProfileDiagnosis" => Some(Self::Action(TrayAction::OpenLastProfileDiagnosis)),
            "OpenBundlesRoot" => Some(Self::Action(TrayAction::OpenBundlesRoot)),
            "OpenAuditsRoot" => Some(Self::Action(TrayAction::OpenAuditsRoot)),
            "OpenGuardianDataRoot" => Some(Self::Action(TrayAction::OpenGuardianDataRoot)),
            "Exit" => Some(Self::Exit),
            _ => None,
        }
    }
}

struct GuardianTrayApp {
    current_exe: PathBuf,
    event_proxy: EventLoopProxy<TrayUserEvent>,
    tray_icon: Option<TrayIcon>,
    status_item: Option<MenuItem>,
    last_action_item: Option<MenuItem>,
    refresh_item: Option<MenuItem>,
    diagnose_profile_item: Option<MenuItem>,
    repair_codex_item: Option<MenuItem>,
    repair_docker_item: Option<MenuItem>,
    export_bundle_item: Option<MenuItem>,
    export_bundle_zip_item: Option<MenuItem>,
    export_bundle_zip_retain_item: Option<MenuItem>,
    open_latest_bundle_item: Option<MenuItem>,
    open_latest_bundle_zip_item: Option<MenuItem>,
    open_last_profile_item: Option<MenuItem>,
    startup_error: Option<GuardianError>,
    latest_bundle_root: Option<PathBuf>,
    latest_bundle_archive: Option<PathBuf>,
    latest_profile_output: Option<PathBuf>,
    action_in_flight: bool,
    exit_code: i32,
    test_state_path: Option<PathBuf>,
    status_text: String,
    last_action_text: String,
    tooltip_text: String,
}

impl GuardianTrayApp {
    fn new(current_exe: PathBuf, event_proxy: EventLoopProxy<TrayUserEvent>) -> Self {
        let status_text = "状态：启动中 - 正在初始化托盘控制台".to_string();
        let last_action_text = "最近动作：等待托盘启动".to_string();
        let tooltip_text = format!("{GUARDIAN_PRODUCT_NAME_ZH} 正在启动");
        if let Some(command_path) = env::var_os(TRAY_TEST_COMMAND_PATH_ENV).map(PathBuf::from) {
            spawn_tray_test_command_watcher(command_path, event_proxy.clone());
        }

        Self {
            current_exe,
            event_proxy,
            tray_icon: None,
            status_item: None,
            last_action_item: None,
            refresh_item: None,
            diagnose_profile_item: None,
            repair_codex_item: None,
            repair_docker_item: None,
            export_bundle_item: None,
            export_bundle_zip_item: None,
            export_bundle_zip_retain_item: None,
            open_latest_bundle_item: None,
            open_latest_bundle_zip_item: None,
            open_last_profile_item: None,
            startup_error: None,
            latest_bundle_root: None,
            latest_bundle_archive: None,
            latest_profile_output: None,
            action_in_flight: false,
            exit_code: 0,
            test_state_path: env::var_os(TRAY_TEST_STATE_PATH_ENV).map(PathBuf::from),
            status_text,
            last_action_text,
            tooltip_text,
        }
    }

    fn initialize_tray(&mut self, event_loop: &ActiveEventLoop) -> Result<(), GuardianError> {
        let status_item = MenuItem::with_id(MENU_ID_STATUS, &self.status_text, false, None);
        let last_action_item =
            MenuItem::with_id(MENU_ID_LAST_ACTION, &self.last_action_text, false, None);
        let refresh_item = MenuItem::with_id(
            MENU_ID_REFRESH,
            TrayAction::RefreshStatus.label(),
            true,
            None,
        );
        let diagnose_profile_item = MenuItem::with_id(
            MENU_ID_DIAGNOSE_PROFILE,
            TrayAction::DiagnoseProfile.label(),
            true,
            None,
        );
        let repair_codex_item = MenuItem::with_id(
            MENU_ID_REPAIR_CODEX,
            TrayAction::RepairCodexConfirm.label(),
            true,
            None,
        );
        let repair_docker_item = MenuItem::with_id(
            MENU_ID_REPAIR_DOCKER,
            TrayAction::RepairDockerConfirm.label(),
            true,
            None,
        );
        let export_bundle_item = MenuItem::with_id(
            MENU_ID_EXPORT_BUNDLE,
            TrayAction::ExportBundle.label(),
            true,
            None,
        );
        let export_bundle_zip_item = MenuItem::with_id(
            MENU_ID_EXPORT_BUNDLE_ZIP,
            TrayAction::ExportBundleZip.label(),
            true,
            None,
        );
        let export_bundle_zip_retain_item = MenuItem::with_id(
            MENU_ID_EXPORT_BUNDLE_ZIP_RETAIN,
            TrayAction::ExportBundleZipRetain.label(),
            true,
            None,
        );
        let open_latest_bundle_item = MenuItem::with_id(
            MENU_ID_OPEN_LATEST_BUNDLE,
            TrayAction::OpenLatestBundle.label(),
            false,
            None,
        );
        let open_latest_bundle_zip_item = MenuItem::with_id(
            MENU_ID_OPEN_LATEST_BUNDLE_ZIP,
            TrayAction::OpenLatestBundleZip.label(),
            false,
            None,
        );
        let open_last_profile_item = MenuItem::with_id(
            MENU_ID_OPEN_LAST_PROFILE,
            TrayAction::OpenLastProfileDiagnosis.label(),
            false,
            None,
        );
        let open_bundles_root_item = MenuItem::with_id(
            MENU_ID_OPEN_BUNDLES_ROOT,
            TrayAction::OpenBundlesRoot.label(),
            true,
            None,
        );
        let open_audits_root_item = MenuItem::with_id(
            MENU_ID_OPEN_AUDITS_ROOT,
            TrayAction::OpenAuditsRoot.label(),
            true,
            None,
        );
        let open_guardian_data_item = MenuItem::with_id(
            MENU_ID_OPEN_GUARDIAN_DATA,
            TrayAction::OpenGuardianDataRoot.label(),
            true,
            None,
        );
        let diagnose_menu =
            Submenu::with_items("只读诊断", true, &[&refresh_item, &diagnose_profile_item])
                .map_err(|error| {
                    GuardianError::invalid_state(format!("构建“只读诊断”子菜单失败：{error}"))
                })?;
        let repair_menu =
            Submenu::with_items("确认修复", true, &[&repair_codex_item, &repair_docker_item])
                .map_err(|error| {
                    GuardianError::invalid_state(format!("构建“确认修复”子菜单失败：{error}"))
                })?;
        let export_menu = Submenu::with_items(
            "导出与结果",
            true,
            &[
                &export_bundle_item,
                &export_bundle_zip_item,
                &export_bundle_zip_retain_item,
                &PredefinedMenuItem::separator(),
                &open_latest_bundle_item,
                &open_latest_bundle_zip_item,
                &open_last_profile_item,
            ],
        )
        .map_err(|error| {
            GuardianError::invalid_state(format!("构建“导出与结果”子菜单失败：{error}"))
        })?;
        let open_menu = Submenu::with_items(
            "打开目录",
            true,
            &[
                &open_bundles_root_item,
                &open_audits_root_item,
                &open_guardian_data_item,
            ],
        )
        .map_err(|error| {
            GuardianError::invalid_state(format!("构建“打开目录”子菜单失败：{error}"))
        })?;
        let exit_item = MenuItem::with_id(MENU_ID_EXIT, "退出控制台托盘", true, None);

        let menu = Menu::new();
        menu.append_items(&[
            &status_item,
            &last_action_item,
            &PredefinedMenuItem::separator(),
            &diagnose_menu,
            &repair_menu,
            &export_menu,
            &open_menu,
            &PredefinedMenuItem::separator(),
            &exit_item,
        ])
        .map_err(|error| GuardianError::invalid_state(format!("构建托盘菜单失败：{error}")))?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip(self.tooltip_text.clone())
            .with_menu_on_left_click(false)
            .with_icon(build_status_icon(StatusLevel::Warn)?)
            .build()
            .map_err(|error| GuardianError::invalid_state(format!("构建托盘图标失败：{error}")))?;

        self.status_item = Some(status_item);
        self.last_action_item = Some(last_action_item);
        self.refresh_item = Some(refresh_item);
        self.diagnose_profile_item = Some(diagnose_profile_item);
        self.repair_codex_item = Some(repair_codex_item);
        self.repair_docker_item = Some(repair_docker_item);
        self.export_bundle_item = Some(export_bundle_item);
        self.export_bundle_zip_item = Some(export_bundle_zip_item);
        self.export_bundle_zip_retain_item = Some(export_bundle_zip_retain_item);
        self.open_latest_bundle_item = Some(open_latest_bundle_item);
        self.open_latest_bundle_zip_item = Some(open_latest_bundle_zip_item);
        self.open_last_profile_item = Some(open_last_profile_item);
        self.tray_icon = Some(tray_icon);
        self.write_test_state_snapshot();
        self.set_last_action_text("最近动作：托盘已初始化，正在执行启动检查");
        self.dispatch_action(TrayAction::RefreshStatus);
        event_loop.set_control_flow(winit::event_loop::ControlFlow::Wait);
        Ok(())
    }

    fn dispatch_action(&mut self, action: TrayAction) {
        if action.is_async_guarded() && self.action_in_flight {
            self.set_last_action_text(&format!(
                "最近动作：已忽略 {}，因为另一个托盘动作仍在执行",
                action.label()
            ));
            return;
        }

        if action.requires_confirmation() && !confirm_tray_action(action) {
            self.set_last_action_text(&format!("最近动作：已取消 {}", action.label()));
            return;
        }

        if action.is_async_guarded() {
            self.action_in_flight = true;
            self.set_async_action_items_enabled(false);
        }

        self.set_last_action_text(&format!("最近动作：{}执行中", action.label()));
        spawn_tray_action(
            action,
            self.current_exe.clone(),
            self.event_proxy.clone(),
            self.latest_bundle_root.clone(),
            self.latest_bundle_archive.clone(),
            self.latest_profile_output.clone(),
        );
    }

    fn apply_action_result(
        &mut self,
        action: TrayAction,
        result: Result<TrayActionSuccess, String>,
    ) {
        if action.is_async_guarded() {
            self.action_in_flight = false;
            self.set_async_action_items_enabled(true);
        }

        match result {
            Ok(success) => self.apply_action_success(action, success),
            Err(error) => self.apply_action_failure(action, &error),
        }
    }

    fn apply_action_success(&mut self, action: TrayAction, success: TrayActionSuccess) {
        if action.is_bundle_export() {
            self.latest_bundle_root = success.latest_bundle_root.clone();
            self.latest_bundle_archive = if action.produces_bundle_archive() {
                success.latest_bundle_archive.clone()
            } else {
                None
            };
            if let Some(item) = &self.open_latest_bundle_item {
                item.set_enabled(self.latest_bundle_root.is_some());
            }
            if let Some(item) = &self.open_latest_bundle_zip_item {
                item.set_enabled(self.latest_bundle_archive.is_some());
            }
        }
        if action == TrayAction::DiagnoseProfile {
            self.latest_profile_output = success.latest_profile_output.clone();
            if let Some(item) = &self.open_last_profile_item {
                item.set_enabled(self.latest_profile_output.is_some());
            }
        }

        if let Some(report) = success.report {
            let summary = localized_dominant_summary(&report);
            self.set_status_text(&status_label(report.status, &summary));
            self.set_tooltip(&tooltip_label(report.status, &summary));
            let _ = self.set_icon(report.status);
            self.set_last_action_text(&action_label(action, success.exit_code, &success.detail));
            self.exit_code = 0;
            return;
        }

        self.set_last_action_text(&action_label(action, success.exit_code, &success.detail));
    }

    fn apply_action_failure(&mut self, action: TrayAction, error: &str) {
        self.set_status_text("状态：失败 - 托盘动作执行失败");
        self.set_last_action_text(&format!(
            "最近动作：{}失败 - {}",
            action.label(),
            truncate_text(error, 72)
        ));
        self.set_tooltip(&format!(
            "{GUARDIAN_PRODUCT_NAME_ZH} · 失败 · 托盘动作异常：{}",
            truncate_text(error, 96)
        ));
        let _ = self.set_icon(StatusLevel::Fail);
        self.exit_code = 1;
    }

    fn set_async_action_items_enabled(&mut self, enabled: bool) {
        if let Some(item) = &self.refresh_item {
            item.set_enabled(enabled);
        }
        if let Some(item) = &self.repair_codex_item {
            item.set_enabled(enabled);
        }
        if let Some(item) = &self.repair_docker_item {
            item.set_enabled(enabled);
        }
        if let Some(item) = &self.export_bundle_item {
            item.set_enabled(enabled);
        }
        if let Some(item) = &self.diagnose_profile_item {
            item.set_enabled(enabled);
        }
        if let Some(item) = &self.export_bundle_zip_item {
            item.set_enabled(enabled);
        }
        if let Some(item) = &self.export_bundle_zip_retain_item {
            item.set_enabled(enabled);
        }
        if let Some(item) = &self.open_latest_bundle_item {
            item.set_enabled(enabled && self.latest_bundle_root.is_some());
        }
        if let Some(item) = &self.open_latest_bundle_zip_item {
            item.set_enabled(enabled && self.latest_bundle_archive.is_some());
        }
        if let Some(item) = &self.open_last_profile_item {
            item.set_enabled(enabled && self.latest_profile_output.is_some());
        }
        self.write_test_state_snapshot();
    }

    fn set_status_text(&mut self, value: &str) {
        self.status_text = value.to_string();
        if let Some(item) = &self.status_item {
            item.set_text(value);
        }
        self.write_test_state_snapshot();
    }

    fn set_last_action_text(&mut self, value: &str) {
        self.last_action_text = value.to_string();
        if let Some(item) = &self.last_action_item {
            item.set_text(value);
        }
        self.write_test_state_snapshot();
    }

    fn set_tooltip(&mut self, value: &str) {
        self.tooltip_text = value.to_string();
        if let Some(icon) = &self.tray_icon {
            let _ = icon.set_tooltip(Some(value.to_string()));
        }
        self.write_test_state_snapshot();
    }

    fn set_icon(&self, status: StatusLevel) -> Result<(), GuardianError> {
        if let Some(icon) = &self.tray_icon {
            icon.set_icon(Some(build_status_icon(status)?))
                .map_err(|error| {
                    GuardianError::invalid_state(format!("更新托盘图标失败：{error}"))
                })?;
        }
        Ok(())
    }

    fn write_test_state_snapshot(&self) {
        let Some(path) = &self.test_state_path else {
            return;
        };

        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let tray_rect = self.tray_icon.as_ref().and_then(|icon| icon.rect());
        let payload = serde_json::json!({
            "pid": std::process::id(),
            "tray_hwnd": self
                .tray_icon
                .as_ref()
                .map(|icon| format!("{:X}", icon.window_handle() as usize)),
            "status_text": self.status_text,
            "last_action_text": self.last_action_text,
            "tooltip_text": self.tooltip_text,
            "action_in_flight": self.action_in_flight,
            "latest_bundle_root": self
                .latest_bundle_root
                .as_ref()
                .map(|path| path.display().to_string()),
            "latest_bundle_archive": self
                .latest_bundle_archive
                .as_ref()
                .map(|path| path.display().to_string()),
            "latest_profile_output": self
                .latest_profile_output
                .as_ref()
                .map(|path| path.display().to_string()),
            "open_latest_bundle_enabled": !self.action_in_flight && self.latest_bundle_root.is_some(),
            "open_latest_bundle_zip_enabled": !self.action_in_flight && self.latest_bundle_archive.is_some(),
            "open_last_profile_enabled": !self.action_in_flight && self.latest_profile_output.is_some(),
            "tray_rect": tray_rect.map(|rect| serde_json::json!({
                "x": rect.position.x,
                "y": rect.position.y,
                "width": rect.size.width,
                "height": rect.size.height,
            })),
        });

        if let Ok(serialized) = serde_json::to_vec_pretty(&payload) {
            let _ = fs::write(path, serialized);
        }
    }
}

impl ApplicationHandler<TrayUserEvent> for GuardianTrayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.tray_icon.is_some() || self.startup_error.is_some() {
            return;
        }

        if let Err(error) = self.initialize_tray(event_loop) {
            self.startup_error = Some(error);
            self.exit_code = 1;
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: TrayUserEvent) {
        match event {
            TrayUserEvent::Menu(event) if event.id.0 == MENU_ID_EXIT => {
                self.set_last_action_text("最近动作：托盘已请求退出");
                event_loop.exit();
            }
            TrayUserEvent::Menu(event) => {
                if let Some(action) = TrayAction::from_menu_id(&event.id) {
                    self.dispatch_action(action);
                }
            }
            TrayUserEvent::TestCommand(TrayTestCommand::Action(action)) => {
                self.dispatch_action(action);
            }
            TrayUserEvent::TestCommand(TrayTestCommand::Exit) => {
                self.set_last_action_text("最近动作：托盘已请求退出");
                event_loop.exit();
            }
            TrayUserEvent::ActionFinished(result) => {
                self.apply_action_result(result.action, result.result);
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        _event: WindowEvent,
    ) {
    }
}

fn spawn_tray_action(
    action: TrayAction,
    current_exe: PathBuf,
    event_proxy: EventLoopProxy<TrayUserEvent>,
    latest_bundle_root: Option<PathBuf>,
    latest_bundle_archive: Option<PathBuf>,
    latest_profile_output: Option<PathBuf>,
) {
    thread::spawn(move || {
        let result = execute_tray_action(
            action,
            &current_exe,
            latest_bundle_root,
            latest_bundle_archive,
            latest_profile_output,
        )
        .map_err(|error| error.to_string());
        let _ = event_proxy.send_event(TrayUserEvent::ActionFinished(Box::new(TrayActionResult {
            action,
            result,
        })));
    });
}

fn spawn_tray_test_command_watcher(
    command_path: PathBuf,
    event_proxy: EventLoopProxy<TrayUserEvent>,
) {
    thread::spawn(move || {
        loop {
            if let Ok(contents) = fs::read_to_string(&command_path) {
                let _ = fs::remove_file(&command_path);
                if let Some(command) = TrayTestCommand::parse(&contents) {
                    let _ = event_proxy.send_event(TrayUserEvent::TestCommand(command));
                }
            }

            thread::sleep(std::time::Duration::from_millis(200));
        }
    });
}

fn execute_tray_action(
    action: TrayAction,
    current_exe: &Path,
    latest_bundle_root: Option<PathBuf>,
    latest_bundle_archive: Option<PathBuf>,
    latest_profile_output: Option<PathBuf>,
) -> Result<TrayActionSuccess, GuardianError> {
    if action == TrayAction::DiagnoseProfile {
        let output = build_profile_diagnosis_output_path()?;
        let args = vec![
            OsString::from("diagnose"),
            OsString::from("profile"),
            OsString::from("--json"),
            OsString::from("--output"),
            output.into_os_string(),
        ];
        return execute_cli_action(action, current_exe, &args);
    }

    if let Some(args) = action.args() {
        let args = args.iter().map(OsString::from).collect::<Vec<_>>();
        return execute_cli_action(action, current_exe, &args);
    }

    let target_path = open_target_for_action(
        action,
        latest_bundle_root,
        latest_bundle_archive,
        latest_profile_output,
    )?;
    prepare_open_target(action, &target_path)?;
    open_path(&target_path)?;

    Ok(TrayActionSuccess {
        exit_code: 0,
        report: None,
        detail: localized_open_detail(&target_path),
        latest_bundle_root: None,
        latest_bundle_archive: None,
        latest_profile_output: None,
    })
}

fn execute_cli_action(
    action: TrayAction,
    current_exe: &Path,
    args: &[OsString],
) -> Result<TrayActionSuccess, GuardianError> {
    let output = run_health_report_command(current_exe, args, action.label())?;
    let exit_code = output.exit_code;
    let report = output.report;

    let detail = if action == TrayAction::ExportBundle {
        bundle_root_from_notes(&report.notes)
            .map(|path| format!("bundle saved to {}", path.display()))
            .unwrap_or_else(|| localized_dominant_summary(&report))
    } else if action.produces_bundle_archive() {
        bundle_archive_from_notes(&report.notes)
            .map(|path| format!("bundle zip saved to {}", path.display()))
            .or_else(|| {
                bundle_root_from_notes(&report.notes)
                    .map(|path| format!("bundle saved to {}", path.display()))
            })
            .unwrap_or_else(|| localized_dominant_summary(&report))
    } else if action == TrayAction::DiagnoseProfile {
        profile_diagnosis_from_notes(&report.notes)
            .map(|path| format!("profile diagnosis saved to {}", path.display()))
            .unwrap_or_else(|| localized_dominant_summary(&report))
    } else {
        localized_dominant_summary(&report)
    };

    Ok(TrayActionSuccess {
        exit_code,
        latest_bundle_root: if action.is_bundle_export() {
            bundle_root_from_notes(&report.notes)
        } else {
            None
        },
        latest_bundle_archive: if action.produces_bundle_archive() {
            bundle_archive_from_notes(&report.notes)
        } else {
            None
        },
        latest_profile_output: if action == TrayAction::DiagnoseProfile {
            profile_diagnosis_from_notes(&report.notes)
        } else {
            None
        },
        report: Some(report),
        detail,
    })
}

fn open_target_for_action(
    action: TrayAction,
    latest_bundle_root: Option<PathBuf>,
    latest_bundle_archive: Option<PathBuf>,
    latest_profile_output: Option<PathBuf>,
) -> Result<PathBuf, GuardianError> {
    match action {
        TrayAction::OpenLatestBundle => latest_bundle_root.ok_or_else(|| {
            GuardianError::invalid_state("尚未生成诊断包；请先执行“导出诊断包”。".to_string())
        }),
        TrayAction::OpenLatestBundleZip => latest_bundle_archive.ok_or_else(|| {
            GuardianError::invalid_state(
                "尚未生成压缩诊断包；请先执行“导出诊断包并压缩”。".to_string(),
            )
        }),
        TrayAction::OpenLastProfileDiagnosis => latest_profile_output.ok_or_else(|| {
            GuardianError::invalid_state(
                "尚未生成最新 Profile 诊断；请先执行“只读诊断 Profile”。".to_string(),
            )
        }),
        TrayAction::OpenBundlesRoot => guardian_bundle_dir().map_err(GuardianError::Io),
        TrayAction::OpenAuditsRoot => guardian_audit_dir().map_err(GuardianError::Io),
        TrayAction::OpenGuardianDataRoot => guardian_data_dir().map_err(GuardianError::Io),
        _ => Err(GuardianError::invalid_state(format!(
            "当前动作 `{}` 不支持解析打开目标",
            action.label()
        ))),
    }
}

fn prepare_open_target(action: TrayAction, path: &Path) -> Result<(), GuardianError> {
    if action.creates_root_directory_if_missing() {
        fs::create_dir_all(path).map_err(GuardianError::Io)?;
        return Ok(());
    }

    if path.exists() {
        return Ok(());
    }

    Err(GuardianError::invalid_state(format!(
        "`{}` 对应的最新路径已不存在：{}",
        action.label(),
        path.display()
    )))
}

fn status_label(status: StatusLevel, summary: &str) -> String {
    format!(
        "状态：{} - {}",
        status_name_zh(status),
        truncate_text(summary, 72)
    )
}

fn tooltip_label(status: StatusLevel, summary: &str) -> String {
    format!(
        "{GUARDIAN_PRODUCT_NAME_ZH} · {} · {}",
        status_name_zh(status),
        truncate_text(summary, 96)
    )
}

fn action_label(action: TrayAction, exit_code: i32, detail: &str) -> String {
    format!(
        "最近动作：{} -> EXIT={}（{}）",
        action.label(),
        exit_code,
        truncate_text(&localized_detail_text(detail), 64)
    )
}

fn confirm_tray_action(action: TrayAction) -> bool {
    let message = match action {
        TrayAction::RepairCodexConfirm => concat!(
            "即将执行真实的 Codex 修复链。\n",
            "- 会触发 `guardian repair codex --confirm --json`\n",
            "- 可能修改 state 数据库、为目标项目补齐 `%USERPROFILE%\\.codex\\config.toml` 中的 trusted 条目，或为 `/resume` slow-path 布置受控 launcher hotfix\n",
            "- 会写入审计文件，并在写后做验证\n",
            "- 建议先阅读主窗口中的状态摘要与风险说明\n\n",
            "确认继续执行吗？"
        ),
        TrayAction::RepairDockerConfirm => concat!(
            "即将执行真实的 Docker / WSL 修复链。\n",
            "- 会触发 `guardian repair docker --confirm --json`\n",
            "- 可能修改 `.wslconfig` 并触发受保护恢复\n",
            "- 请仅在确认当前环境可安全操作时继续\n\n",
            "确认继续执行吗？"
        ),
        _ => return true,
    };

    let title = to_wide(action.label());
    let message = to_wide(message);
    unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MESSAGEBOX_STYLE(MB_YESNO.0 | MB_ICONWARNING.0),
        ) == IDYES
    }
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }

    let keep = max_chars.saturating_sub(3);
    let truncated = value.chars().take(keep).collect::<String>();
    format!("{truncated}...")
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn build_status_icon(status: StatusLevel) -> Result<Icon, GuardianError> {
    let (r, g, b) = match status {
        StatusLevel::Ok => (52, 199, 89),
        StatusLevel::Warn => (255, 159, 10),
        StatusLevel::Fail => (255, 59, 48),
    };

    let width = 32;
    let height = 32;
    let radius = 12.0f32;
    let center = 15.5f32;
    let mut rgba = vec![0u8; width * height * 4];

    for y in 0..height {
        for x in 0..width {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let distance = (dx * dx + dy * dy).sqrt();
            let offset = (y * width + x) * 4;
            let pixel = &mut rgba[offset..offset + 4];

            if distance <= radius {
                pixel[0] = r;
                pixel[1] = g;
                pixel[2] = b;
                pixel[3] = 255;
            } else if distance <= radius + 1.5 {
                pixel[0] = 33;
                pixel[1] = 33;
                pixel[2] = 33;
                pixel[3] = 255;
            }
        }
    }

    Icon::from_rgba(rgba, width as u32, height as u32)
        .map_err(|error| GuardianError::invalid_state(format!("生成托盘图标像素失败：{error}")))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use guardian_core::types::{DomainReport, DomainReports, HealthReport, StatusLevel};

    use super::{
        TrayAction, action_label, bundle_archive_from_notes, bundle_root_from_notes,
        open_target_for_action, status_label, truncate_text,
    };

    #[test]
    fn tray_actions_map_to_cli_arguments() {
        assert_eq!(
            TrayAction::RefreshStatus.args(),
            Some(&["check", "--json"][..])
        );
        assert_eq!(TrayAction::DiagnoseProfile.args(), None);
        assert_eq!(
            TrayAction::RepairCodexConfirm.args(),
            Some(&["repair", "codex", "--confirm", "--json"][..])
        );
        assert_eq!(
            TrayAction::RepairDockerConfirm.args(),
            Some(&["repair", "docker", "--confirm", "--json"][..])
        );
        assert_eq!(
            TrayAction::ExportBundle.args(),
            Some(&["export", "bundle", "--json"][..])
        );
        assert_eq!(
            TrayAction::ExportBundleZip.args(),
            Some(&["export", "bundle", "--json", "--zip"][..])
        );
        assert_eq!(
            TrayAction::ExportBundleZipRetain.args(),
            Some(&["export", "bundle", "--json", "--zip", "--retain", "5"][..])
        );
        assert_eq!(TrayAction::OpenLatestBundle.args(), None);
        assert_eq!(TrayAction::OpenLatestBundleZip.args(), None);
        assert_eq!(TrayAction::OpenLastProfileDiagnosis.args(), None);
    }

    #[test]
    fn extracts_bundle_root_from_export_note() {
        let bundle_root = bundle_root_from_notes(&[String::from(
            "Bundle export wrote diagnostic files to C:\\bundle-root",
        )])
        .expect("bundle root should be parsed");

        assert_eq!(bundle_root.display().to_string(), "C:\\bundle-root");
    }

    #[test]
    fn extracts_bundle_archive_from_export_note() {
        let bundle_archive = bundle_archive_from_notes(&[String::from(
            "Bundle export also wrote zip archive to C:\\bundle-root.zip",
        )])
        .expect("bundle archive should be parsed");

        assert_eq!(bundle_archive.display().to_string(), "C:\\bundle-root.zip");
    }

    #[test]
    fn resolves_latest_bundle_path_for_open_action() {
        let expected = PathBuf::from("C:\\bundle-root");
        let resolved = open_target_for_action(
            TrayAction::OpenLatestBundle,
            Some(expected.clone()),
            None,
            None,
        )
        .expect("latest bundle path should resolve");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolves_latest_bundle_zip_path_for_open_action() {
        let expected = PathBuf::from("C:\\bundle-root.zip");
        let resolved = open_target_for_action(
            TrayAction::OpenLatestBundleZip,
            None,
            Some(expected.clone()),
            None,
        )
        .expect("latest bundle zip path should resolve");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolves_last_profile_path_for_open_action() {
        let expected = PathBuf::from("C:\\profile-diagnosis.json");
        let resolved = open_target_for_action(
            TrayAction::OpenLastProfileDiagnosis,
            None,
            None,
            Some(expected.clone()),
        )
        .expect("latest profile path should resolve");

        assert_eq!(resolved, expected);
    }

    #[test]
    fn latest_bundle_open_requires_previous_export() {
        let error = open_target_for_action(TrayAction::OpenLatestBundle, None, None, None)
            .expect_err("latest bundle open should fail without previous export");

        assert!(error.to_string().contains("导出诊断包"));
    }

    #[test]
    fn latest_bundle_zip_open_requires_previous_zip_export() {
        let error = open_target_for_action(TrayAction::OpenLatestBundleZip, None, None, None)
            .expect_err("latest bundle zip open should fail without previous export");

        assert!(error.to_string().contains("导出诊断包并压缩"));
    }

    #[test]
    fn latest_profile_open_requires_previous_profile_diagnosis() {
        let error = open_target_for_action(TrayAction::OpenLastProfileDiagnosis, None, None, None)
            .expect_err("latest profile open should fail without previous diagnosis");

        assert!(error.to_string().contains("只读诊断 Profile"));
    }

    #[test]
    fn action_label_uses_detail_text() {
        let label = action_label(
            TrayAction::ExportBundle,
            2,
            "bundle saved to %LOCALAPPDATA%\\guardian\\bundles\\bundle-20260416-233130",
        );
        assert!(label.contains("EXIT=2"));
        assert!(label.contains("bundle-20260416-233130"));
    }

    #[test]
    fn status_label_truncates_long_summary() {
        let label = status_label(
            StatusLevel::Warn,
            "这是一段非常长的中文摘要，用来验证托盘状态标签在 Windows shell 中不会因为内容过长而变得难以阅读，需要被安全截断，并且继续补充更多说明来确保超过限制长度。",
        );
        assert!(label.starts_with("状态：警告 - "));
        assert!(label.ends_with("..."));
    }

    #[test]
    fn truncate_text_keeps_short_values_unchanged() {
        assert_eq!(truncate_text("short", 32), "short");
    }

    #[allow(dead_code)]
    fn sample_report(status: StatusLevel, summary: &str, notes: Vec<String>) -> HealthReport {
        HealthReport::new(
            "2026-04-16T00:00:00+08:00".to_string(),
            DomainReports {
                codex: DomainReport::new(StatusLevel::Ok, "codex ok", Vec::new(), Vec::new()),
                docker_wsl: DomainReport::new(StatusLevel::Ok, "docker ok", Vec::new(), Vec::new()),
                profile: DomainReport::new(status, summary, Vec::new(), Vec::new()),
            },
            Vec::new(),
            notes,
        )
    }
}
