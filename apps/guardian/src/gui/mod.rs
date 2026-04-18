mod theme;
mod widgets;

use std::{
    ffi::{OsString, c_void},
    fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use chrono::Local;
use guardian_core::{
    GuardianError,
    types::{DomainReport, HealthReport, StatusLevel},
};
use guardian_windows::paths::{guardian_audit_dir, guardian_bundle_dir, guardian_data_dir};
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            HDC, HFONT, InvalidateRect, OPAQUE, SetBkColor, SetBkMode, SetTextColor, TRANSPARENT,
            UpdateWindow,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{
                DRAWITEMSTRUCT, EM_SETRECTNP, ODS_DISABLED, ODS_SELECTED, ODT_BUTTON, ODT_STATIC,
                SetScrollInfo, ShowScrollBar, WC_BUTTON, WC_EDIT, WC_STATIC,
            },
            Input::KeyboardAndMouse::EnableWindow,
            WindowsAndMessaging::{
                BS_MULTILINE, BS_OWNERDRAW, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT,
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, ES_AUTOVSCROLL,
                ES_LEFT, ES_MULTILINE, ES_READONLY, ES_WANTRETURN, GWLP_USERDATA, GetClientRect,
                GetMessageW, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HMENU,
                IDC_ARROW, IDYES, IsWindowVisible, KillTimer, LoadCursorW, MB_ICONERROR,
                MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_YESNO, MESSAGEBOX_STYLE, MINMAXINFO,
                MSG, MessageBoxW, MoveWindow, PostQuitMessage, RegisterClassW, SB_BOTTOM,
                SB_LINEDOWN, SB_LINEUP, SB_PAGEDOWN, SB_PAGEUP, SB_THUMBPOSITION, SB_THUMBTRACK,
                SB_TOP, SB_VERT, SCROLLINFO, SIF_PAGE, SIF_POS, SIF_RANGE, SW_HIDE, SW_SHOW,
                SendMessageW, SetTimer, SetWindowLongPtrW, SetWindowTextW, ShowWindow,
                TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_CREATE,
                WM_CTLCOLORBTN, WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DRAWITEM,
                WM_GETMINMAXINFO, WM_MOUSEWHEEL, WM_NCDESTROY, WM_SETFONT, WM_SIZE, WM_TIMER,
                WM_VSCROLL, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
                WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
            },
        },
    },
    core::{PCWSTR, w},
};

use self::{
    theme::{
        ACTION_BUTTON_MIN_WIDTH, BANNER_HEIGHT, BG_BASE, BG_INPUT, BG_SURFACE_ALT,
        COLLAPSIBLE_HEIGHT, CONTENT_WIDTH_MAX, CONTENT_WIDTH_MIN, DETAIL_BODY_HEIGHT,
        DETAIL_TAB_HEIGHT, DOMAIN_CARD_HEIGHT, EDIT_PADDING_X, EDIT_PADDING_Y, GuiTheme,
        LAYOUT_BASE_HEIGHT, LAYOUT_BASE_WIDTH, PRIMARY_BUTTON_HEIGHT, REVIEW_STACK_BREAKPOINT,
        SECONDARY_BUTTON_HEIGHT, SECTION_HINT_HEIGHT, SECTION_LABEL_HEIGHT, SPACING_L, SPACING_M,
        SPACING_S, SPACING_XL, SPACING_XS, STEP_CARD_HEIGHT, STEP_NAV_HEIGHT, STEP_NAV_MIN_WIDTH,
        TEXT_PRIMARY, TEXT_SECONDARY, WINDOW_HEIGHT, WINDOW_MIN_HEIGHT, WINDOW_MIN_WIDTH,
        WINDOW_WIDTH, scaled,
    },
    widgets::{
        ButtonKind, ButtonVisualState, StepVisualState, paint_banner, paint_button, paint_card,
        paint_collapsible, paint_step_nav,
    },
};

use crate::shell::{
    GUARDIAN_PRODUCT_NAME_ZH, JsonCommandSuccess, build_profile_diagnosis_output_path,
    bundle_archive_from_notes, bundle_root_from_notes, domain_title, localized_action_description,
    localized_detail_text, localized_domain_summary, localized_dominant_summary,
    localized_open_detail, localized_report_note, open_path, profile_diagnosis_from_notes,
    run_health_report_command, status_name_zh,
};

const WINDOW_CLASS_NAME: PCWSTR = w!("GuardianGuiWindow");
const WINDOW_TITLE: PCWSTR = w!("Guardian 稳定性控制台");
const GUI_TIMER_ID: usize = 1;

const ID_RUN_CHECK: i32 = 1001;
const ID_REPAIR_CODEX: i32 = 1002;
const ID_REPAIR_DOCKER: i32 = 1003;
const ID_DIAGNOSE_PROFILE: i32 = 1004;
const ID_EXPORT_BUNDLE: i32 = 1005;
const ID_EXPORT_BUNDLE_ZIP: i32 = 1006;
const ID_EXPORT_BUNDLE_ZIP_RETAIN: i32 = 1007;
const ID_OPEN_LATEST_BUNDLE: i32 = 1008;
const ID_OPEN_LATEST_BUNDLE_ZIP: i32 = 1009;
const ID_OPEN_LAST_PROFILE: i32 = 1010;
const ID_OPEN_AUDITS: i32 = 1011;
const ID_OPEN_BUNDLES: i32 = 1012;
const ID_OPEN_DATA: i32 = 1013;

const ID_HERO_BANNER: i32 = 1503;
const ID_STEP_REVIEW: i32 = 1504;
const ID_STEP_DIAGNOSE: i32 = 1505;
const ID_STEP_REPAIR: i32 = 1506;
const ID_STEP_EXPORT: i32 = 1507;
const ID_EXPERT_TOGGLE: i32 = 1508;
const ID_DETAIL_TAB_SUMMARY: i32 = 1601;
const ID_DETAIL_TAB_ACTIVITY: i32 = 1602;
const ID_DETAIL_TAB_JSON: i32 = 1603;

const ID_OVERVIEW_EDIT: i32 = 2001;
const ID_CODEX_EDIT: i32 = 2002;
const ID_DOCKER_EDIT: i32 = 2003;
const ID_PROFILE_EDIT: i32 = 2004;
const ID_DETAILS_EDIT: i32 = 2005;
const ID_RAW_JSON_EDIT: i32 = 2006;
const ID_ACTIVITY_EDIT: i32 = 2007;

const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
const STATIC_OWNERDRAW_STYLE: u32 = 0x0000_000D;

unsafe extern "system" {
    fn SetProcessDpiAwarenessContext(value: isize) -> i32;
}

pub fn run_gui() -> Result<i32, GuardianError> {
    let current_exe = std::env::current_exe().map_err(GuardianError::Io)?;
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let module = unsafe {
        GetModuleHandleW(None).map_err(|error| {
            GuardianError::invalid_state(format!("GetModuleHandleW failed: {error}"))
        })?
    };
    let instance = HINSTANCE(module.0);

    register_window_class(instance)?;

    let state = Box::new(GuiWindowState::new(current_exe, instance));
    let state_ptr = Box::into_raw(state);

    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            WINDOW_CLASS_NAME,
            WINDOW_TITLE,
            WS_OVERLAPPEDWINDOW | WS_VISIBLE | WS_VSCROLL | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WINDOW_WIDTH,
            WINDOW_HEIGHT,
            None,
            None,
            instance,
            Some(state_ptr.cast::<c_void>()),
        )
        .map_err(|error| {
            drop(Box::from_raw(state_ptr));
            GuardianError::invalid_state(format!("CreateWindowExW failed: {error}"))
        })?
    };

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
    }

    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, HWND(std::ptr::null_mut()), 0, 0).0 };
        if status == -1 {
            return Err(GuardianError::invalid_state(
                "运行 Guardian 稳定性控制台时，GetMessageW 返回 -1",
            ));
        }
        if status == 0 {
            break;
        }

        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    Ok(message.wParam.0 as i32)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuiAction {
    RunCheck,
    RepairCodexConfirm,
    RepairDockerConfirm,
    DiagnoseProfile,
    ExportBundle,
    ExportBundleZip,
    ExportBundleZipRetain5,
    OpenLatestBundle,
    OpenLatestBundleZip,
    OpenLastProfileDiagnosis,
    OpenAuditsFolder,
    OpenBundlesFolder,
    OpenGuardianDataFolder,
}

impl GuiAction {
    fn label(self) -> &'static str {
        match self {
            Self::RunCheck => "刷新整机检查",
            Self::RepairCodexConfirm => "确认修复 Codex",
            Self::RepairDockerConfirm => "确认修复 Docker / WSL",
            Self::DiagnoseProfile => "只读诊断 Profile",
            Self::ExportBundle => "导出诊断包",
            Self::ExportBundleZip => "导出并压缩",
            Self::ExportBundleZipRetain5 => "导出并保留 5 份",
            Self::OpenLatestBundle => "打开最新诊断包",
            Self::OpenLatestBundleZip => "打开最新压缩包",
            Self::OpenLastProfileDiagnosis => "打开最新 Profile 诊断",
            Self::OpenAuditsFolder => "打开审计目录",
            Self::OpenBundlesFolder => "打开包目录",
            Self::OpenGuardianDataFolder => "打开数据目录",
        }
    }

    fn requires_confirmation(self) -> bool {
        matches!(self, Self::RepairCodexConfirm | Self::RepairDockerConfirm)
    }

    fn is_bundle_export(self) -> bool {
        matches!(
            self,
            Self::ExportBundle | Self::ExportBundleZip | Self::ExportBundleZipRetain5
        )
    }

    fn produces_bundle_archive(self) -> bool {
        matches!(self, Self::ExportBundleZip | Self::ExportBundleZipRetain5)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowStep {
    Review,
    Diagnose,
    Repair,
    Export,
}

impl WorkflowStep {
    fn label(self) -> &'static str {
        match self {
            Self::Review => "1. 查看状态",
            Self::Diagnose => "2. 开始诊断",
            Self::Repair => "3. 确认修复",
            Self::Export => "4. 导出证据",
        }
    }

    fn from_control_id(control_id: i32) -> Option<Self> {
        match control_id {
            ID_STEP_REVIEW => Some(Self::Review),
            ID_STEP_DIAGNOSE => Some(Self::Diagnose),
            ID_STEP_REPAIR => Some(Self::Repair),
            ID_STEP_EXPORT => Some(Self::Export),
            _ => None,
        }
    }
}

struct GuiCommandSpec {
    args: Vec<OsString>,
    profile_output: Option<PathBuf>,
}

impl GuiCommandSpec {
    fn build(action: GuiAction) -> Result<Self, GuardianError> {
        let spec = match action {
            GuiAction::RunCheck => Self {
                args: vec![OsString::from("check"), OsString::from("--json")],
                profile_output: None,
            },
            GuiAction::RepairCodexConfirm => Self {
                args: vec![
                    OsString::from("repair"),
                    OsString::from("codex"),
                    OsString::from("--confirm"),
                    OsString::from("--json"),
                ],
                profile_output: None,
            },
            GuiAction::RepairDockerConfirm => Self {
                args: vec![
                    OsString::from("repair"),
                    OsString::from("docker"),
                    OsString::from("--confirm"),
                    OsString::from("--json"),
                ],
                profile_output: None,
            },
            GuiAction::DiagnoseProfile => {
                let output = build_profile_diagnosis_output_path()?;
                Self {
                    args: vec![
                        OsString::from("diagnose"),
                        OsString::from("profile"),
                        OsString::from("--json"),
                        OsString::from("--output"),
                        output.as_os_str().to_os_string(),
                    ],
                    profile_output: Some(output),
                }
            }
            GuiAction::ExportBundle => Self {
                args: vec![
                    OsString::from("export"),
                    OsString::from("bundle"),
                    OsString::from("--json"),
                ],
                profile_output: None,
            },
            GuiAction::ExportBundleZip => Self {
                args: vec![
                    OsString::from("export"),
                    OsString::from("bundle"),
                    OsString::from("--json"),
                    OsString::from("--zip"),
                ],
                profile_output: None,
            },
            GuiAction::ExportBundleZipRetain5 => Self {
                args: vec![
                    OsString::from("export"),
                    OsString::from("bundle"),
                    OsString::from("--json"),
                    OsString::from("--zip"),
                    OsString::from("--retain"),
                    OsString::from("5"),
                ],
                profile_output: None,
            },
            action => {
                return Err(GuardianError::invalid_state(format!(
                    "GUI 操作 `{}` 没有映射到 JSON 命令",
                    action.label()
                )));
            }
        };

        Ok(spec)
    }
}

#[derive(Debug)]
struct GuiActionSuccess {
    exit_code: i32,
    report: HealthReport,
    raw_json: String,
    stderr: String,
    latest_bundle_root: Option<PathBuf>,
    latest_bundle_archive: Option<PathBuf>,
    latest_profile_output: Option<PathBuf>,
}

#[derive(Debug)]
enum WorkerMessage {
    ActionFinished(GuiAction, Result<GuiActionSuccess, String>),
}

struct GuiControls {
    hero_banner: HWND,
    step_review_button: HWND,
    step_diagnose_button: HWND,
    step_repair_button: HWND,
    step_export_button: HWND,
    overview_group: HWND,
    overview: HWND,
    actions_group: HWND,
    actions_hint: HWND,
    repair_group: HWND,
    repair_hint: HWND,
    artifacts_group: HWND,
    artifacts_hint: HWND,
    codex: HWND,
    docker_wsl: HWND,
    profile: HWND,
    details_group: HWND,
    details: HWND,
    detail_tab_summary_button: HWND,
    detail_tab_activity_button: HWND,
    detail_tab_json_button: HWND,
    raw_json: HWND,
    activity: HWND,
    run_check_button: HWND,
    repair_codex_button: HWND,
    repair_docker_button: HWND,
    diagnose_profile_button: HWND,
    export_bundle_button: HWND,
    export_bundle_zip_button: HWND,
    export_bundle_zip_retain_button: HWND,
    open_latest_bundle_button: HWND,
    open_latest_bundle_zip_button: HWND,
    open_last_profile_button: HWND,
    open_audits_button: HWND,
    open_bundles_button: HWND,
    open_data_button: HWND,
}

impl Default for GuiControls {
    fn default() -> Self {
        let null = HWND(std::ptr::null_mut());
        Self {
            hero_banner: null,
            step_review_button: null,
            step_diagnose_button: null,
            step_repair_button: null,
            step_export_button: null,
            overview_group: null,
            overview: null,
            actions_group: null,
            actions_hint: null,
            repair_group: null,
            repair_hint: null,
            artifacts_group: null,
            artifacts_hint: null,
            codex: null,
            docker_wsl: null,
            profile: null,
            details_group: null,
            details: null,
            detail_tab_summary_button: null,
            detail_tab_activity_button: null,
            detail_tab_json_button: null,
            raw_json: null,
            activity: null,
            run_check_button: null,
            repair_codex_button: null,
            repair_docker_button: null,
            diagnose_profile_button: null,
            export_bundle_button: null,
            export_bundle_zip_button: null,
            export_bundle_zip_retain_button: null,
            open_latest_bundle_button: null,
            open_latest_bundle_zip_button: null,
            open_last_profile_button: null,
            open_audits_button: null,
            open_bundles_button: null,
            open_data_button: null,
        }
    }
}

impl GuiControls {
    fn all_handles(&self) -> [HWND; 36] {
        [
            self.hero_banner,
            self.step_review_button,
            self.step_diagnose_button,
            self.step_repair_button,
            self.step_export_button,
            self.overview_group,
            self.overview,
            self.actions_group,
            self.actions_hint,
            self.repair_group,
            self.repair_hint,
            self.artifacts_group,
            self.artifacts_hint,
            self.codex,
            self.docker_wsl,
            self.profile,
            self.details_group,
            self.details,
            self.detail_tab_summary_button,
            self.detail_tab_activity_button,
            self.detail_tab_json_button,
            self.raw_json,
            self.activity,
            self.run_check_button,
            self.repair_codex_button,
            self.repair_docker_button,
            self.diagnose_profile_button,
            self.export_bundle_button,
            self.export_bundle_zip_button,
            self.export_bundle_zip_retain_button,
            self.open_latest_bundle_button,
            self.open_latest_bundle_zip_button,
            self.open_last_profile_button,
            self.open_audits_button,
            self.open_bundles_button,
            self.open_data_button,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GuiButtonStates {
    async_actions_enabled: bool,
    open_latest_bundle_enabled: bool,
    open_latest_bundle_zip_enabled: bool,
    open_last_profile_enabled: bool,
}

impl GuiButtonStates {
    fn from_runtime(
        action_in_flight: Option<GuiAction>,
        latest_bundle_root: Option<&PathBuf>,
        latest_bundle_archive: Option<&PathBuf>,
        latest_profile_output: Option<&PathBuf>,
    ) -> Self {
        let async_actions_enabled = action_in_flight.is_none();
        Self {
            async_actions_enabled,
            open_latest_bundle_enabled: async_actions_enabled && latest_bundle_root.is_some(),
            open_latest_bundle_zip_enabled: async_actions_enabled
                && latest_bundle_archive.is_some(),
            open_last_profile_enabled: async_actions_enabled && latest_profile_output.is_some(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuiDetailTab {
    Summary,
    Activity,
    RawJson,
}

impl GuiDetailTab {
    fn label(self) -> &'static str {
        match self {
            Self::Summary => "建议与风险",
            Self::Activity => "执行轨迹",
            Self::RawJson => "原始 JSON",
        }
    }

    fn control_id(self) -> i32 {
        match self {
            Self::Summary => ID_DETAIL_TAB_SUMMARY,
            Self::Activity => ID_DETAIL_TAB_ACTIVITY,
            Self::RawJson => ID_DETAIL_TAB_JSON,
        }
    }
}

struct GuiWindowState {
    main_hwnd: HWND,
    current_exe: PathBuf,
    instance: HINSTANCE,
    tx: Sender<WorkerMessage>,
    rx: Receiver<WorkerMessage>,
    theme: GuiTheme,
    controls: GuiControls,
    latest_report: Option<HealthReport>,
    latest_raw_json: String,
    latest_stderr: String,
    latest_exit_code: Option<i32>,
    last_action_text: String,
    last_error: Option<String>,
    latest_bundle_root: Option<PathBuf>,
    latest_bundle_archive: Option<PathBuf>,
    latest_profile_output: Option<PathBuf>,
    action_in_flight: Option<GuiAction>,
    current_step: WorkflowStep,
    active_detail_tab: GuiDetailTab,
    expert_details_expanded: bool,
    diagnosis_completed: bool,
    startup_refresh_pending: bool,
    scroll_offset: i32,
    content_height: i32,
    activity_log: Vec<String>,
}

impl GuiWindowState {
    fn new(current_exe: PathBuf, instance: HINSTANCE) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut state = Self {
            main_hwnd: HWND(std::ptr::null_mut()),
            current_exe,
            instance,
            tx,
            rx,
            theme: GuiTheme::new(),
            controls: GuiControls::default(),
            latest_report: None,
            latest_raw_json: String::new(),
            latest_stderr: String::new(),
            latest_exit_code: None,
            last_action_text: "控制台已启动".to_string(),
            last_error: None,
            latest_bundle_root: None,
            latest_bundle_archive: None,
            latest_profile_output: None,
            action_in_flight: None,
            current_step: WorkflowStep::Review,
            active_detail_tab: GuiDetailTab::Summary,
            expert_details_expanded: false,
            diagnosis_completed: false,
            startup_refresh_pending: true,
            scroll_offset: 0,
            content_height: 0,
            activity_log: Vec::new(),
        };
        state.log(format!("{GUARDIAN_PRODUCT_NAME_ZH}已启动。"));
        state
    }

    fn initialize_window(&mut self, hwnd: HWND) -> Result<(), GuardianError> {
        self.main_hwnd = hwnd;
        self.controls.hero_banner = create_owner_draw_static(hwnd, self.instance, ID_HERO_BANNER)?;
        self.controls.step_review_button = create_button(
            hwnd,
            self.instance,
            ID_STEP_REVIEW,
            WorkflowStep::Review.label(),
        )?;
        self.controls.step_diagnose_button = create_button(
            hwnd,
            self.instance,
            ID_STEP_DIAGNOSE,
            WorkflowStep::Diagnose.label(),
        )?;
        self.controls.step_repair_button = create_button(
            hwnd,
            self.instance,
            ID_STEP_REPAIR,
            WorkflowStep::Repair.label(),
        )?;
        self.controls.step_export_button = create_button(
            hwnd,
            self.instance,
            ID_STEP_EXPORT,
            WorkflowStep::Export.label(),
        )?;
        self.controls.actions_group = create_group_box(hwnd, self.instance, "只读诊断")?;
        self.controls.actions_hint = create_static_text(hwnd, self.instance, 0, "")?;
        self.controls.repair_group = create_group_box(hwnd, self.instance, "确认修复")?;
        self.controls.repair_hint = create_static_text(hwnd, self.instance, 0, "")?;
        self.controls.artifacts_group = create_group_box(hwnd, self.instance, "导出与结果")?;
        self.controls.artifacts_hint = create_static_text(hwnd, self.instance, 0, "")?;
        self.controls.details_group =
            create_button(hwnd, self.instance, ID_EXPERT_TOGGLE, "展开专家详情")?;
        self.controls.detail_tab_summary_button = create_button(
            hwnd,
            self.instance,
            ID_DETAIL_TAB_SUMMARY,
            GuiDetailTab::Summary.label(),
        )?;
        self.controls.detail_tab_activity_button = create_button(
            hwnd,
            self.instance,
            ID_DETAIL_TAB_ACTIVITY,
            GuiDetailTab::Activity.label(),
        )?;
        self.controls.detail_tab_json_button = create_button(
            hwnd,
            self.instance,
            ID_DETAIL_TAB_JSON,
            GuiDetailTab::RawJson.label(),
        )?;
        self.controls.overview = create_owner_draw_static(hwnd, self.instance, ID_OVERVIEW_EDIT)?;
        self.controls.codex = create_owner_draw_static(hwnd, self.instance, ID_CODEX_EDIT)?;
        self.controls.docker_wsl = create_owner_draw_static(hwnd, self.instance, ID_DOCKER_EDIT)?;
        self.controls.profile = create_owner_draw_static(hwnd, self.instance, ID_PROFILE_EDIT)?;
        self.controls.details = create_readonly_edit(hwnd, self.instance, ID_DETAILS_EDIT)?;
        self.controls.raw_json = create_readonly_edit(hwnd, self.instance, ID_RAW_JSON_EDIT)?;
        self.controls.activity = create_readonly_edit(hwnd, self.instance, ID_ACTIVITY_EDIT)?;

        self.controls.run_check_button = create_button(
            hwnd,
            self.instance,
            ID_RUN_CHECK,
            GuiAction::RunCheck.label(),
        )?;
        self.controls.repair_codex_button = create_button(
            hwnd,
            self.instance,
            ID_REPAIR_CODEX,
            GuiAction::RepairCodexConfirm.label(),
        )?;
        self.controls.repair_docker_button = create_button(
            hwnd,
            self.instance,
            ID_REPAIR_DOCKER,
            GuiAction::RepairDockerConfirm.label(),
        )?;
        self.controls.diagnose_profile_button = create_button(
            hwnd,
            self.instance,
            ID_DIAGNOSE_PROFILE,
            GuiAction::DiagnoseProfile.label(),
        )?;
        self.controls.export_bundle_button = create_button(
            hwnd,
            self.instance,
            ID_EXPORT_BUNDLE,
            GuiAction::ExportBundle.label(),
        )?;
        self.controls.export_bundle_zip_button = create_button(
            hwnd,
            self.instance,
            ID_EXPORT_BUNDLE_ZIP,
            GuiAction::ExportBundleZip.label(),
        )?;
        self.controls.export_bundle_zip_retain_button = create_button(
            hwnd,
            self.instance,
            ID_EXPORT_BUNDLE_ZIP_RETAIN,
            GuiAction::ExportBundleZipRetain5.label(),
        )?;
        self.controls.open_latest_bundle_button = create_button(
            hwnd,
            self.instance,
            ID_OPEN_LATEST_BUNDLE,
            GuiAction::OpenLatestBundle.label(),
        )?;
        self.controls.open_latest_bundle_zip_button = create_button(
            hwnd,
            self.instance,
            ID_OPEN_LATEST_BUNDLE_ZIP,
            GuiAction::OpenLatestBundleZip.label(),
        )?;
        self.controls.open_last_profile_button = create_button(
            hwnd,
            self.instance,
            ID_OPEN_LAST_PROFILE,
            GuiAction::OpenLastProfileDiagnosis.label(),
        )?;
        self.controls.open_audits_button = create_button(
            hwnd,
            self.instance,
            ID_OPEN_AUDITS,
            GuiAction::OpenAuditsFolder.label(),
        )?;
        self.controls.open_bundles_button = create_button(
            hwnd,
            self.instance,
            ID_OPEN_BUNDLES,
            GuiAction::OpenBundlesFolder.label(),
        )?;
        self.controls.open_data_button = create_button(
            hwnd,
            self.instance,
            ID_OPEN_DATA,
            GuiAction::OpenGuardianDataFolder.label(),
        )?;

        self.apply_theme();
        set_text(
            self.controls.actions_hint,
            "这些动作只读取状态，不会修改系统；建议先执行整机检查或导出 Profile 诊断。",
        );
        set_text(
            self.controls.repair_hint,
            "只在确认当前风险边界后再执行确认修复；Profile 保持引导式恢复，不提供自动修复。",
        );
        set_text(
            self.controls.artifacts_hint,
            "导出结果用于留痕与回传；路径按钮已下沉到默认折叠的专家详情层。",
        );
        self.layout_controls(hwnd)?;
        self.refresh_controls();
        self.spawn_async_action(GuiAction::RunCheck)?;
        self.refresh_controls();
        unsafe {
            SetTimer(hwnd, GUI_TIMER_ID, 100, None);
        }
        Ok(())
    }

    fn apply_theme(&self) {
        apply_font(self.controls.hero_banner, self.theme.body_font);
        for control in [
            self.controls.step_review_button,
            self.controls.step_diagnose_button,
            self.controls.step_repair_button,
            self.controls.step_export_button,
            self.controls.details_group,
        ] {
            apply_font(control, self.theme.caption_font);
        }
        for control in [
            self.controls.overview_group,
            self.controls.actions_group,
            self.controls.repair_group,
            self.controls.artifacts_group,
        ] {
            apply_font(control, self.theme.h2_font);
        }
        for control in [
            self.controls.actions_hint,
            self.controls.repair_hint,
            self.controls.artifacts_hint,
        ] {
            apply_font(control, self.theme.caption_font);
        }

        for control in [
            self.controls.hero_banner,
            self.controls.overview,
            self.controls.codex,
            self.controls.docker_wsl,
            self.controls.profile,
            self.controls.details,
            self.controls.activity,
        ] {
            apply_font(control, self.theme.body_font);
        }
        apply_font(self.controls.raw_json, self.theme.mono_font);
        for control in [
            self.controls.run_check_button,
            self.controls.repair_codex_button,
            self.controls.repair_docker_button,
            self.controls.diagnose_profile_button,
            self.controls.export_bundle_button,
            self.controls.export_bundle_zip_button,
            self.controls.export_bundle_zip_retain_button,
            self.controls.detail_tab_summary_button,
            self.controls.detail_tab_activity_button,
            self.controls.detail_tab_json_button,
            self.controls.open_latest_bundle_button,
            self.controls.open_latest_bundle_zip_button,
            self.controls.open_last_profile_button,
            self.controls.open_audits_button,
            self.controls.open_bundles_button,
            self.controls.open_data_button,
        ] {
            apply_font(control, self.theme.body_font);
        }
    }

    fn responsive_scales(client_width: i32, client_height: i32) -> (f32, f32, f32) {
        let width_scale = (client_width as f32 / LAYOUT_BASE_WIDTH as f32).clamp(1.0, 1.8);
        let height_scale = (client_height as f32 / LAYOUT_BASE_HEIGHT as f32).clamp(1.0, 1.8);
        let font_scale = ((width_scale + height_scale) / 2.0).clamp(1.0, 1.5);
        (width_scale, height_scale, font_scale)
    }

    fn sync_responsive_theme(&mut self, client_width: i32, client_height: i32) {
        let (_, _, font_scale) = Self::responsive_scales(client_width, client_height);
        if self.theme.update_scale(font_scale) {
            self.apply_theme();
        }
    }

    fn handle_command(&mut self, control_id: i32) {
        let result = match control_id {
            ID_EXPERT_TOGGLE => {
                self.expert_details_expanded = !self.expert_details_expanded;
                self.layout_controls(self.main_hwnd).map(|_| {
                    self.invalidate_main_window();
                })
            }
            ID_RUN_CHECK => self.spawn_async_action(GuiAction::RunCheck),
            ID_REPAIR_CODEX => self.start_confirmed_action(GuiAction::RepairCodexConfirm),
            ID_REPAIR_DOCKER => self.start_confirmed_action(GuiAction::RepairDockerConfirm),
            ID_DIAGNOSE_PROFILE => self.spawn_async_action(GuiAction::DiagnoseProfile),
            ID_EXPORT_BUNDLE => self.spawn_async_action(GuiAction::ExportBundle),
            ID_EXPORT_BUNDLE_ZIP => self.spawn_async_action(GuiAction::ExportBundleZip),
            ID_EXPORT_BUNDLE_ZIP_RETAIN => {
                self.spawn_async_action(GuiAction::ExportBundleZipRetain5)
            }
            ID_DETAIL_TAB_SUMMARY => {
                self.set_active_detail_tab(GuiDetailTab::Summary);
                Ok(())
            }
            ID_DETAIL_TAB_ACTIVITY => {
                self.set_active_detail_tab(GuiDetailTab::Activity);
                Ok(())
            }
            ID_DETAIL_TAB_JSON => {
                self.set_active_detail_tab(GuiDetailTab::RawJson);
                Ok(())
            }
            ID_OPEN_LATEST_BUNDLE => {
                self.handle_open_action(GuiAction::OpenLatestBundle);
                Ok(())
            }
            ID_OPEN_LATEST_BUNDLE_ZIP => {
                self.handle_open_action(GuiAction::OpenLatestBundleZip);
                Ok(())
            }
            ID_OPEN_LAST_PROFILE => {
                self.handle_open_action(GuiAction::OpenLastProfileDiagnosis);
                Ok(())
            }
            ID_OPEN_AUDITS => {
                self.handle_open_action(GuiAction::OpenAuditsFolder);
                Ok(())
            }
            ID_OPEN_BUNDLES => {
                self.handle_open_action(GuiAction::OpenBundlesFolder);
                Ok(())
            }
            ID_OPEN_DATA => {
                self.handle_open_action(GuiAction::OpenGuardianDataFolder);
                Ok(())
            }
            _ if WorkflowStep::from_control_id(control_id).is_some() => {
                self.activate_step(WorkflowStep::from_control_id(control_id).expect("step id"));
                Ok(())
            }
            _ => Ok(()),
        };

        if let Err(error) = result {
            self.last_error = Some(error.to_string());
            self.last_action_text = "操作启动失败".to_string();
            self.log(format!("无法启动操作：{error}。"));
            self.show_error_dialog("操作启动失败", &error.to_string());
        }
    }

    fn start_confirmed_action(&mut self, action: GuiAction) -> Result<(), GuardianError> {
        if action.requires_confirmation() && !self.confirm_action(action) {
            self.last_error = None;
            self.last_action_text = format!("已取消：{}", action.label());
            self.log(format!("用户取消了 `{}`。", action.label()));
            return Ok(());
        }

        self.spawn_async_action(action)
    }

    fn confirm_action(&self, action: GuiAction) -> bool {
        if self.main_hwnd.0.is_null() {
            return true;
        }

        let message = match action {
            GuiAction::RepairCodexConfirm => concat!(
                "即将执行真实的 Codex 修复链。\n",
                "- 会触发 `guardian repair codex --confirm --json`\n",
                "- 可能修改 state 数据库并写入审计文件\n",
                "- 建议先阅读上方摘要与风险说明\n\n",
                "确认继续执行吗？"
            ),
            GuiAction::RepairDockerConfirm => concat!(
                "即将执行真实的 Docker / WSL 修复链。\n",
                "- 会触发 `guardian repair docker --confirm --json`\n",
                "- 可能修改 `.wslconfig` 并触发受保护恢复\n",
                "- 仅在确认当前环境可安全操作时继续\n\n",
                "确认继续执行吗？"
            ),
            _ => return true,
        };

        let wide_title = to_wide(action.label());
        let wide_message = to_wide(message);
        unsafe {
            MessageBoxW(
                self.main_hwnd,
                PCWSTR(wide_message.as_ptr()),
                PCWSTR(wide_title.as_ptr()),
                MESSAGEBOX_STYLE(MB_YESNO.0 | MB_ICONWARNING.0),
            ) == IDYES
        }
    }

    fn spawn_async_action(&mut self, action: GuiAction) -> Result<(), GuardianError> {
        if let Some(in_flight) = self.action_in_flight {
            self.log(format!(
                "忽略 `{}`，因为 `{}` 仍在执行中。",
                action.label(),
                in_flight.label()
            ));
            return Ok(());
        }

        let spec = GuiCommandSpec::build(action)?;
        let tx = self.tx.clone();
        let current_exe = self.current_exe.clone();
        self.action_in_flight = Some(action);
        self.last_error = None;
        self.last_action_text = format!("{}：执行中", action.label());
        self.log(format!("已开始 `{}`。", action.label()));

        thread::spawn(move || {
            let result =
                execute_gui_action(action, &current_exe, spec).map_err(|error| error.to_string());
            let _ = tx.send(WorkerMessage::ActionFinished(action, result));
        });

        Ok(())
    }

    fn handle_open_action(&mut self, action: GuiAction) {
        let result = self
            .path_for_open_action(action)
            .and_then(|path| {
                open_path(&path)?;
                Ok(path)
            })
            .map(|path| localized_open_detail(&path));

        match result {
            Ok(detail) => {
                self.last_error = None;
                self.last_action_text = format!("{} -> {}", action.label(), detail);
                self.log(format!("已完成 `{}`：{detail}。", action.label()));
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                self.last_action_text = format!("{}：执行失败", action.label());
                self.log(format!("`{}` 执行失败：{error}。", action.label()));
                self.show_error_dialog(&format!("{}失败", action.label()), &error.to_string());
            }
        }
    }

    fn activate_step(&mut self, step: WorkflowStep) {
        if !self.can_activate_step(step) {
            self.show_info_dialog("请先完成上一步", &self.step_block_message(step));
            return;
        }

        if self.current_step == step {
            return;
        }

        self.current_step = step;
        let _ = self.layout_controls(self.main_hwnd);
        self.invalidate_main_window();
    }

    fn set_active_detail_tab(&mut self, tab: GuiDetailTab) {
        if self.active_detail_tab == tab {
            return;
        }

        self.active_detail_tab = tab;
        let _ = self.layout_controls(self.main_hwnd);
        self.invalidate_main_window();
    }

    fn refresh_detail_tab_visibility(&self) {
        let details_visible = self.expert_details_expanded;
        set_visible(self.controls.detail_tab_summary_button, details_visible);
        set_visible(self.controls.detail_tab_activity_button, details_visible);
        set_visible(self.controls.detail_tab_json_button, details_visible);
        set_visible(
            self.controls.details,
            details_visible && self.active_detail_tab == GuiDetailTab::Summary,
        );
        set_visible(
            self.controls.activity,
            details_visible && self.active_detail_tab == GuiDetailTab::Activity,
        );
        set_visible(
            self.controls.raw_json,
            details_visible && self.active_detail_tab == GuiDetailTab::RawJson,
        );
    }

    fn is_current_detail_tab_button(&self, control_id: i32) -> bool {
        self.active_detail_tab.control_id() == control_id
    }

    fn can_activate_step(&self, step: WorkflowStep) -> bool {
        match step {
            WorkflowStep::Review => true,
            WorkflowStep::Diagnose => self.latest_report.is_some(),
            WorkflowStep::Repair | WorkflowStep::Export => self.diagnosis_completed,
        }
    }

    fn step_block_message(&self, step: WorkflowStep) -> String {
        match step {
            WorkflowStep::Review => "当前步骤始终可用。".to_string(),
            WorkflowStep::Diagnose => {
                "请先等待首份整机状态采集完成，再进入“开始诊断”。".to_string()
            }
            WorkflowStep::Repair => "请先完成“开始诊断”，再进入“确认修复”。".to_string(),
            WorkflowStep::Export => "请先完成“开始诊断”，再进入“导出证据”。".to_string(),
        }
    }

    fn step_visual_state(&self, step: WorkflowStep) -> StepVisualState {
        if self.current_step == step {
            StepVisualState::Current
        } else if self.can_activate_step(step) {
            StepVisualState::Ready
        } else {
            StepVisualState::Locked
        }
    }

    fn scroll_viewport_height(&self, client_height: i32) -> i32 {
        (client_height - (SPACING_L + BANNER_HEIGHT + SPACING_S + STEP_NAV_HEIGHT + SPACING_L))
            .max(0)
    }

    fn max_scroll_offset(&self, client_height: i32) -> i32 {
        (self.content_height - self.scroll_viewport_height(client_height)).max(0)
    }

    fn sync_window_scrollbar(&self, hwnd: HWND, client_height: i32) {
        let viewport_height = self.scroll_viewport_height(client_height);
        let max_scroll = self.max_scroll_offset(client_height);
        let info = SCROLLINFO {
            cbSize: std::mem::size_of::<SCROLLINFO>() as u32,
            fMask: SIF_RANGE | SIF_PAGE | SIF_POS,
            nMin: 0,
            nMax: self.content_height.max(viewport_height).saturating_sub(1),
            nPage: viewport_height.max(0) as u32,
            nPos: self.scroll_offset.clamp(0, max_scroll),
            ..Default::default()
        };

        unsafe {
            let _ = SetScrollInfo(hwnd, SB_VERT, &info, true);
            let _ = ShowScrollBar(hwnd, SB_VERT, max_scroll > 0);
        }
    }

    fn set_scroll_offset(&mut self, hwnd: HWND, next_offset: i32) {
        let mut rect = RECT::default();
        let client_height = unsafe {
            if GetClientRect(hwnd, &mut rect).is_err() {
                0
            } else {
                rect.bottom - rect.top
            }
        };
        let clamped = next_offset.clamp(0, self.max_scroll_offset(client_height));
        if clamped == self.scroll_offset {
            self.sync_window_scrollbar(hwnd, client_height);
            return;
        }

        self.scroll_offset = clamped;
        let _ = self.layout_controls(hwnd);
    }

    fn handle_vertical_scroll(&mut self, hwnd: HWND, wparam: WPARAM) {
        let request = (wparam.0 & 0xffff) as u16 as i32;
        let thumb = (((wparam.0 >> 16) & 0xffff) as u16) as i16 as i32;
        let next_offset = match request {
            code if code == SB_LINEUP.0 => self.scroll_offset - 48,
            code if code == SB_LINEDOWN.0 => self.scroll_offset + 48,
            code if code == SB_PAGEUP.0 => self.scroll_offset - 180,
            code if code == SB_PAGEDOWN.0 => self.scroll_offset + 180,
            code if code == SB_TOP.0 => 0,
            code if code == SB_BOTTOM.0 => i32::MAX,
            code if code == SB_THUMBTRACK.0 || code == SB_THUMBPOSITION.0 => thumb,
            _ => self.scroll_offset,
        };

        self.set_scroll_offset(hwnd, next_offset);
    }

    fn handle_mouse_wheel(&mut self, hwnd: HWND, wparam: WPARAM) {
        let delta = (((wparam.0 >> 16) & 0xffff) as u16) as i16 as i32;
        if delta == 0 {
            return;
        }

        let step = ((delta / 120).abs().max(1)) * 60;
        self.set_scroll_offset(hwnd, self.scroll_offset - step * delta.signum());
    }

    fn path_for_open_action(&self, action: GuiAction) -> Result<PathBuf, GuardianError> {
        match action {
            GuiAction::OpenLatestBundle => self.latest_bundle_root.clone().ok_or_else(|| {
                GuardianError::invalid_state("尚未生成最新诊断包；请先执行“导出诊断包”。")
            }),
            GuiAction::OpenLatestBundleZip => self.latest_bundle_archive.clone().ok_or_else(|| {
                GuardianError::invalid_state("尚未生成最新压缩诊断包；请先执行“导出诊断包并压缩”。")
            }),
            GuiAction::OpenLastProfileDiagnosis => {
                self.latest_profile_output.clone().ok_or_else(|| {
                    GuardianError::invalid_state(
                        "尚未生成 Profile 诊断文件；请先执行“Profile 诊断”。",
                    )
                })
            }
            GuiAction::OpenAuditsFolder => ensure_directory(guardian_audit_dir()?),
            GuiAction::OpenBundlesFolder => ensure_directory(guardian_bundle_dir()?),
            GuiAction::OpenGuardianDataFolder => ensure_directory(guardian_data_dir()?),
            other => Err(GuardianError::invalid_state(format!(
                "GUI 操作 `{}` 不是打开路径类动作",
                other.label()
            ))),
        }
    }

    fn poll_worker_messages(&mut self) -> bool {
        let mut changed = false;
        while let Ok(message) = self.rx.try_recv() {
            changed = true;
            let WorkerMessage::ActionFinished(action, result) = message;
            if self.action_in_flight == Some(action) {
                self.action_in_flight = None;
            }

            match result {
                Ok(success) => self.apply_action_success(action, success),
                Err(error) => self.apply_action_failure(action, &error),
            }
        }
        changed
    }

    fn apply_action_success(&mut self, action: GuiAction, success: GuiActionSuccess) {
        let detail = localized_success_detail(action, &success);
        self.latest_exit_code = Some(success.exit_code);
        self.latest_raw_json = success.raw_json;
        self.latest_stderr = success.stderr;
        self.latest_bundle_root = success
            .latest_bundle_root
            .or_else(|| self.latest_bundle_root.clone());
        self.latest_bundle_archive = success
            .latest_bundle_archive
            .or_else(|| self.latest_bundle_archive.clone());
        self.latest_profile_output = success
            .latest_profile_output
            .or_else(|| self.latest_profile_output.clone());
        self.last_error = None;
        self.last_action_text = format!(
            "{} -> EXIT={}（{}）",
            action.label(),
            success.exit_code,
            detail
        );
        self.log(format!(
            "已完成 `{}`，退出码 {}。",
            action.label(),
            success.exit_code
        ));
        self.latest_report = Some(success.report);
        match action {
            GuiAction::RunCheck => {
                if self.startup_refresh_pending {
                    self.startup_refresh_pending = false;
                } else {
                    self.diagnosis_completed = true;
                    self.current_step = WorkflowStep::Diagnose;
                }
            }
            GuiAction::DiagnoseProfile => {
                self.diagnosis_completed = true;
                self.current_step = WorkflowStep::Diagnose;
            }
            GuiAction::RepairCodexConfirm | GuiAction::RepairDockerConfirm => {
                self.current_step = WorkflowStep::Repair;
            }
            GuiAction::ExportBundle
            | GuiAction::ExportBundleZip
            | GuiAction::ExportBundleZipRetain5 => {
                self.current_step = WorkflowStep::Export;
            }
            _ => {}
        }

        if action.requires_confirmation() {
            self.show_info_dialog(
                &format!("{}已完成", action.label()),
                &format!(
                    "{}\n退出码：{}\n结果：{}",
                    action.label(),
                    success.exit_code,
                    detail
                ),
            );
        }

        let _ = self.layout_controls(self.main_hwnd);
        self.invalidate_main_window();
    }

    fn apply_action_failure(&mut self, action: GuiAction, error: &str) {
        self.last_error = Some(error.to_string());
        self.last_action_text = format!("{}：执行失败", action.label());
        self.log(format!("`{}` 执行失败：{error}。", action.label()));
        self.show_error_dialog(&format!("{}失败", action.label()), error);
    }

    fn log(&mut self, message: impl Into<String>) {
        self.activity_log.push(format!(
            "{}  {}",
            Local::now().format("%H:%M:%S"),
            message.into()
        ));
        if self.activity_log.len() > 120 {
            let drop_count = self.activity_log.len().saturating_sub(120);
            self.activity_log.drain(0..drop_count);
        }
    }

    fn layout_controls(&mut self, hwnd: HWND) -> Result<(), GuardianError> {
        let mut rect = RECT::default();
        unsafe {
            GetClientRect(hwnd, &mut rect).map_err(|error| {
                GuardianError::invalid_state(format!("GetClientRect failed: {error}"))
            })?;
        }

        let client_width = rect.right - rect.left;
        let client_height = rect.bottom - rect.top;
        self.sync_responsive_theme(client_width, client_height);

        let (width_scale, height_scale, font_scale) =
            Self::responsive_scales(client_width, client_height);
        let spacing_xs = scaled(SPACING_XS, font_scale);
        let spacing_s = scaled(SPACING_S, font_scale);
        let spacing_m = scaled(SPACING_M, font_scale);
        let spacing_l = scaled(SPACING_L, font_scale);
        let spacing_xl = scaled(SPACING_XL, font_scale);
        let banner_height = scaled(BANNER_HEIGHT, height_scale);
        let step_nav_height = scaled(STEP_NAV_HEIGHT, font_scale);
        let step_card_height = scaled(STEP_CARD_HEIGHT, height_scale);
        let domain_card_height = scaled(DOMAIN_CARD_HEIGHT, height_scale);
        let section_label_height = scaled(SECTION_LABEL_HEIGHT, font_scale);
        let section_hint_height = scaled(SECTION_HINT_HEIGHT, font_scale);
        let primary_button_height = scaled(PRIMARY_BUTTON_HEIGHT, font_scale);
        let secondary_button_height = scaled(SECONDARY_BUTTON_HEIGHT, font_scale);
        let collapsible_height = scaled(COLLAPSIBLE_HEIGHT, font_scale);
        let detail_tab_height = scaled(DETAIL_TAB_HEIGHT, font_scale);
        let detail_body_height = scaled(DETAIL_BODY_HEIGHT, height_scale);
        let step_nav_min_width = scaled(STEP_NAV_MIN_WIDTH, font_scale);
        let action_button_min_width = scaled(ACTION_BUTTON_MIN_WIDTH, font_scale);
        let review_stack_breakpoint = scaled(REVIEW_STACK_BREAKPOINT, width_scale);

        let padded_width = (client_width - spacing_xl * 2).max(0);
        let content_width = if padded_width >= CONTENT_WIDTH_MIN {
            padded_width.min(CONTENT_WIDTH_MAX)
        } else {
            padded_width
        };
        let x = ((client_width - content_width).max(0)) / 2;
        let banner_y = spacing_l;
        move_window(
            self.controls.hero_banner,
            x,
            banner_y,
            content_width,
            banner_height,
        )?;

        let step_y = banner_y + banner_height + spacing_s;
        let step_width = if content_width >= step_nav_min_width * 4 + spacing_s * 3 {
            ((content_width - spacing_s * 3) / 4).max(step_nav_min_width)
        } else {
            ((content_width - spacing_s * 3) / 4).max(0)
        };
        move_button_row(
            hwnd,
            &[
                (ID_STEP_REVIEW, step_width),
                (ID_STEP_DIAGNOSE, step_width),
                (ID_STEP_REPAIR, step_width),
                (ID_STEP_EXPORT, step_width),
            ],
            x,
            step_y,
            step_nav_height,
        )?;

        let scroll_origin_y = step_y + step_nav_height + spacing_l;
        let view_y = |content_y: i32| scroll_origin_y + content_y - self.scroll_offset;
        let mut content_y = 0;

        move_window(
            self.controls.overview,
            x,
            view_y(content_y),
            content_width,
            step_card_height,
        )?;
        content_y += step_card_height + spacing_l;

        match self.current_step {
            WorkflowStep::Review => {
                if content_width < review_stack_breakpoint {
                    for control in [
                        self.controls.codex,
                        self.controls.docker_wsl,
                        self.controls.profile,
                    ] {
                        move_window(
                            control,
                            x,
                            view_y(content_y),
                            content_width,
                            domain_card_height,
                        )?;
                        content_y += domain_card_height + spacing_s;
                    }
                    content_y += spacing_l - spacing_s;
                } else {
                    let column_width = ((content_width - spacing_s * 2) / 3).max(0);
                    move_window(
                        self.controls.codex,
                        x,
                        view_y(content_y),
                        column_width,
                        domain_card_height,
                    )?;
                    move_window(
                        self.controls.docker_wsl,
                        x + column_width + spacing_s,
                        view_y(content_y),
                        column_width,
                        domain_card_height,
                    )?;
                    move_window(
                        self.controls.profile,
                        x + (column_width + spacing_s) * 2,
                        view_y(content_y),
                        column_width,
                        domain_card_height,
                    )?;
                    content_y += domain_card_height + spacing_l;
                }
            }
            WorkflowStep::Diagnose => {
                let dual_button_width = if content_width
                    >= action_button_min_width * 2 + spacing_l * 2 + spacing_s
                {
                    ((content_width - spacing_l * 2 - spacing_s) / 2).max(action_button_min_width)
                } else {
                    ((content_width - spacing_l * 2 - spacing_s) / 2).max(0)
                };
                move_window(
                    self.controls.actions_group,
                    x,
                    view_y(content_y),
                    content_width,
                    section_label_height,
                )?;
                move_window(
                    self.controls.actions_hint,
                    x,
                    view_y(content_y + section_label_height + spacing_xs),
                    content_width,
                    section_hint_height,
                )?;
                let buttons_y = content_y + section_label_height + section_hint_height + spacing_m;
                move_button_row(
                    hwnd,
                    &[
                        (ID_RUN_CHECK, dual_button_width),
                        (ID_DIAGNOSE_PROFILE, dual_button_width),
                    ],
                    x + spacing_l,
                    view_y(buttons_y),
                    primary_button_height,
                )?;
                content_y = buttons_y + primary_button_height + spacing_l;
            }
            WorkflowStep::Repair => {
                let dual_button_width = if content_width
                    >= action_button_min_width * 2 + spacing_l * 2 + spacing_s
                {
                    ((content_width - spacing_l * 2 - spacing_s) / 2).max(action_button_min_width)
                } else {
                    ((content_width - spacing_l * 2 - spacing_s) / 2).max(0)
                };
                move_window(
                    self.controls.repair_group,
                    x,
                    view_y(content_y),
                    content_width,
                    section_label_height,
                )?;
                move_window(
                    self.controls.repair_hint,
                    x,
                    view_y(content_y + section_label_height + spacing_xs),
                    content_width,
                    section_hint_height,
                )?;
                let buttons_y = content_y + section_label_height + section_hint_height + spacing_m;
                move_button_row(
                    hwnd,
                    &[
                        (ID_REPAIR_CODEX, dual_button_width),
                        (ID_REPAIR_DOCKER, dual_button_width),
                    ],
                    x + spacing_l,
                    view_y(buttons_y),
                    primary_button_height,
                )?;
                content_y = buttons_y + primary_button_height + spacing_l;
            }
            WorkflowStep::Export => {
                let triple_button_width =
                    ((content_width - spacing_l * 2 - spacing_s * 2) / 3).max(0);
                move_window(
                    self.controls.artifacts_group,
                    x,
                    view_y(content_y),
                    content_width,
                    section_label_height,
                )?;
                move_window(
                    self.controls.artifacts_hint,
                    x,
                    view_y(content_y + section_label_height + spacing_xs),
                    content_width,
                    section_hint_height,
                )?;
                let buttons_y = content_y + section_label_height + section_hint_height + spacing_m;
                move_button_row(
                    hwnd,
                    &[
                        (ID_EXPORT_BUNDLE, triple_button_width),
                        (ID_EXPORT_BUNDLE_ZIP, triple_button_width),
                        (ID_EXPORT_BUNDLE_ZIP_RETAIN, triple_button_width),
                    ],
                    x + spacing_l,
                    view_y(buttons_y),
                    secondary_button_height,
                )?;
                content_y = buttons_y + secondary_button_height + spacing_l;
            }
        }

        move_window(
            self.controls.details_group,
            x,
            view_y(content_y),
            content_width,
            collapsible_height,
        )?;
        content_y += collapsible_height;

        if self.expert_details_expanded {
            content_y += spacing_m;
            let detail_tab_width = ((content_width - spacing_l * 2 - spacing_s * 2) / 3).max(0);
            move_button_row(
                hwnd,
                &[
                    (ID_DETAIL_TAB_SUMMARY, detail_tab_width),
                    (ID_DETAIL_TAB_ACTIVITY, detail_tab_width),
                    (ID_DETAIL_TAB_JSON, detail_tab_width),
                ],
                x + spacing_l,
                view_y(content_y),
                detail_tab_height,
            )?;
            content_y += detail_tab_height + spacing_s;

            move_window(
                self.controls.details,
                x,
                view_y(content_y),
                content_width,
                detail_body_height,
            )?;
            move_window(
                self.controls.activity,
                x,
                view_y(content_y),
                content_width,
                detail_body_height,
            )?;
            move_window(
                self.controls.raw_json,
                x,
                view_y(content_y),
                content_width,
                detail_body_height,
            )?;
            content_y += detail_body_height + spacing_m;

            let triple_button_width =
                ((content_width - spacing_l * 2 - spacing_s * 2) / 3).max(scaled(160, font_scale));
            move_button_row(
                hwnd,
                &[
                    (ID_OPEN_LATEST_BUNDLE, triple_button_width),
                    (ID_OPEN_LATEST_BUNDLE_ZIP, triple_button_width),
                    (ID_OPEN_LAST_PROFILE, triple_button_width),
                ],
                x,
                view_y(content_y),
                secondary_button_height,
            )?;
            content_y += secondary_button_height + spacing_s;
            move_button_row(
                hwnd,
                &[
                    (ID_OPEN_AUDITS, triple_button_width),
                    (ID_OPEN_BUNDLES, triple_button_width),
                    (ID_OPEN_DATA, triple_button_width),
                ],
                x,
                view_y(content_y),
                secondary_button_height,
            )?;
            content_y += secondary_button_height;
        }

        self.content_height = content_y + spacing_l;
        let max_scroll = self.max_scroll_offset(client_height);
        let clamped_offset = self.scroll_offset.clamp(0, max_scroll);
        if clamped_offset != self.scroll_offset {
            self.scroll_offset = clamped_offset;
            return self.layout_controls(hwnd);
        }
        self.sync_window_scrollbar(hwnd, client_height);
        self.redraw_window_tree(hwnd);

        Ok(())
    }

    fn refresh_controls(&self) {
        self.refresh_window_title();
        set_text(
            self.controls.hero_banner,
            &format!(
                "{}\n{}",
                self.banner_title_text(),
                self.banner_subtitle_text()
            ),
        );
        set_text(
            self.controls.step_review_button,
            WorkflowStep::Review.label(),
        );
        set_text(
            self.controls.step_diagnose_button,
            WorkflowStep::Diagnose.label(),
        );
        set_text(
            self.controls.step_repair_button,
            WorkflowStep::Repair.label(),
        );
        set_text(
            self.controls.step_export_button,
            WorkflowStep::Export.label(),
        );
        set_text(
            self.controls.details_group,
            if self.expert_details_expanded {
                "收起专家详情"
            } else {
                "展开专家详情"
            },
        );
        set_text(self.controls.overview, &self.overview_text());
        set_text(
            self.controls.codex,
            &self.domain_text(
                "codex",
                self.latest_report.as_ref().map(|r| &r.domains.codex),
            ),
        );
        set_text(
            self.controls.docker_wsl,
            &self.domain_text(
                "docker_wsl",
                self.latest_report.as_ref().map(|r| &r.domains.docker_wsl),
            ),
        );
        set_text(
            self.controls.profile,
            &self.domain_text(
                "profile",
                self.latest_report.as_ref().map(|r| &r.domains.profile),
            ),
        );
        set_text(self.controls.details, &self.details_text());
        set_text(self.controls.raw_json, &self.raw_json_text());
        set_text(self.controls.activity, &self.activity_text());
        set_text(self.controls.actions_group, "开始诊断");
        set_text(self.controls.repair_group, "确认修复");
        set_text(self.controls.artifacts_group, "导出证据");
        set_text(
            self.controls.detail_tab_summary_button,
            GuiDetailTab::Summary.label(),
        );
        set_text(
            self.controls.detail_tab_activity_button,
            GuiDetailTab::Activity.label(),
        );
        set_text(
            self.controls.detail_tab_json_button,
            GuiDetailTab::RawJson.label(),
        );
        self.apply_text_padding();
        self.refresh_section_visibility();
        self.refresh_detail_tab_visibility();
        self.refresh_button_states();
    }

    fn apply_text_padding(&self) {
        let padding_x = scaled(EDIT_PADDING_X, self.theme.scale());
        let padding_y = scaled(EDIT_PADDING_Y, self.theme.scale());
        for control in [
            self.controls.details,
            self.controls.raw_json,
            self.controls.activity,
        ] {
            set_edit_padding(control, padding_x, padding_y, padding_x, padding_y);
        }
    }

    fn refresh_section_visibility(&self) {
        let on_review = self.current_step == WorkflowStep::Review;
        let on_diagnose = self.current_step == WorkflowStep::Diagnose;
        let on_repair = self.current_step == WorkflowStep::Repair;
        let on_export = self.current_step == WorkflowStep::Export;

        set_visible(self.controls.overview, true);
        set_visible(self.controls.codex, on_review);
        set_visible(self.controls.docker_wsl, on_review);
        set_visible(self.controls.profile, on_review);

        set_visible(self.controls.actions_group, on_diagnose);
        set_visible(self.controls.actions_hint, on_diagnose);
        set_visible(self.controls.run_check_button, on_diagnose);
        set_visible(self.controls.diagnose_profile_button, on_diagnose);

        set_visible(self.controls.repair_group, on_repair);
        set_visible(self.controls.repair_hint, on_repair);
        set_visible(self.controls.repair_codex_button, on_repair);
        set_visible(self.controls.repair_docker_button, on_repair);

        set_visible(self.controls.artifacts_group, on_export);
        set_visible(self.controls.artifacts_hint, on_export);
        set_visible(self.controls.export_bundle_button, on_export);
        set_visible(self.controls.export_bundle_zip_button, on_export);
        set_visible(self.controls.export_bundle_zip_retain_button, on_export);

        let expert_visible = self.expert_details_expanded;
        for control in [
            self.controls.open_latest_bundle_button,
            self.controls.open_latest_bundle_zip_button,
            self.controls.open_last_profile_button,
            self.controls.open_audits_button,
            self.controls.open_bundles_button,
            self.controls.open_data_button,
        ] {
            set_visible(control, expert_visible);
        }
    }

    fn stage_summary_text(&self) -> String {
        if let Some(action) = self.action_in_flight {
            format!("步骤执行中｜{}", action.label())
        } else {
            match self.current_step {
                WorkflowStep::Review => "步骤 1｜查看状态".to_string(),
                WorkflowStep::Diagnose => "步骤 2｜开始诊断".to_string(),
                WorkflowStep::Repair => "步骤 3｜确认修复".to_string(),
                WorkflowStep::Export => "步骤 4｜导出证据".to_string(),
            }
        }
    }

    fn recommended_action_summary(&self) -> String {
        if let Some(action) = self.action_in_flight {
            format!("正在执行 {}，请等待这轮结果返回。", action.label())
        } else if let Some(report) = &self.latest_report {
            if let Some(action) = report.actions.first() {
                let mode = if action.requires_confirmation {
                    "确认修复"
                } else {
                    "只读诊断"
                };
                format!(
                    "[{mode}] {}",
                    localized_action_description(&action.description)
                )
            } else {
                "当前没有额外动作要求，可直接查看三域摘要或导出结果。".to_string()
            }
        } else {
            "先执行“刷新整机检查”，生成首份真实健康报告。".to_string()
        }
    }

    fn banner_title_text(&self) -> String {
        if let Some(action) = self.action_in_flight {
            return format!("{} · 正在执行", action.label());
        }

        let status_text = self
            .latest_report
            .as_ref()
            .map(|report| status_name_zh(report.status))
            .unwrap_or("待采集");
        format!("{GUARDIAN_PRODUCT_NAME_ZH} · {status_text}")
    }

    fn banner_subtitle_text(&self) -> String {
        if self.latest_report.is_none() {
            return "正在采集首份真实健康报告，请先查看当前机器状态。".to_string();
        }

        format!(
            "{} ｜ {}",
            self.stage_summary_text(),
            self.recommended_action_summary()
        )
    }

    fn banner_timestamp_text(&self) -> String {
        self.latest_report
            .as_ref()
            .map(|report| format!("最近检查：{}", compact_timestamp(&report.timestamp)))
            .unwrap_or_else(|| "最近检查：等待首份报告".to_string())
    }

    fn current_step_card_header(&self) -> &'static str {
        match self.current_step {
            WorkflowStep::Review => "当前机器状态",
            WorkflowStep::Diagnose => "诊断准备",
            WorkflowStep::Repair => "修复风险说明",
            WorkflowStep::Export => "证据导出",
        }
    }

    fn current_step_card_status(&self) -> Option<StatusLevel> {
        self.latest_report.as_ref().map(|report| report.status)
    }

    fn current_step_card_text(&self) -> String {
        match self.current_step {
            WorkflowStep::Review => self.redesigned_overview_text(),
            WorkflowStep::Diagnose => {
                let report_summary = self
                    .latest_report
                    .as_ref()
                    .map(localized_dominant_summary)
                    .unwrap_or_else(|| "等待首份整机状态。".to_string());
                [
                    "开始诊断会触发真实命令执行，不会使用 mock 或占位数据。".to_string(),
                    format!("- 当前整机结论：{report_summary}"),
                    "- “刷新整机检查”会重新执行 `guardian check --json`。".to_string(),
                    "- “只读诊断 Profile”仅导出诊断 JSON，不会修改注册表或结束安全软件。"
                        .to_string(),
                ]
                .join("\n")
            }
            WorkflowStep::Repair => {
                let recommendation = self.recommended_action_summary();
                let profile_mode =
                    self.latest_report
                        .as_ref()
                        .and_then(|report| {
                            report.domains.profile.evidence.iter().find(|item| {
                                item.key == "guided_recovery_mode" && item.value == "true"
                            })
                        })
                        .map(|_| "Profile 仅允许引导式恢复，GUI 不会自动改注册表。".to_string())
                        .unwrap_or_else(|| {
                            "仅在上方推荐动作明确提示时再执行确认修复。".to_string()
                        });
                [
                    "确认修复会执行真实后端修复链，请先核对风险边界。".to_string(),
                    format!("- 当前推荐：{recommendation}"),
                    format!("- 风险边界：{profile_mode}"),
                    "- 若只需要留痕或回传，请改走“导出证据”，避免把只读诊断误用成自动修复。"
                        .to_string(),
                ]
                .join("\n")
            }
            WorkflowStep::Export => [
                "导出证据会保留本轮真实诊断结果，适合回传与审计。".to_string(),
                format!(
                    "- 最新诊断目录：{}",
                    truncate_text(&display_path(self.latest_bundle_root.as_ref()), 72)
                ),
                format!(
                    "- 最新压缩包：{}",
                    truncate_text(&display_path(self.latest_bundle_archive.as_ref()), 72)
                ),
                format!(
                    "- 最新 Profile 诊断：{}",
                    truncate_text(&display_path(self.latest_profile_output.as_ref()), 72)
                ),
            ]
            .join("\n"),
        }
    }

    #[allow(dead_code)]
    fn redesigned_hero_banner_text(&self) -> String {
        let status_text = self
            .latest_report
            .as_ref()
            .map(|report| status_name_zh(report.status))
            .unwrap_or("待采集");
        let summary_text = self
            .latest_report
            .as_ref()
            .map(localized_dominant_summary)
            .unwrap_or_else(|| "等待首份整机健康报告。".to_string());

        [
            format!("当前状态：{status_text}｜{summary_text}"),
            format!("当前推荐：{}", self.recommended_action_summary()),
        ]
        .join("\n")
    }

    #[allow(dead_code)]
    fn redesigned_hero_subtitle_text(&self) -> String {
        if let Some(action) = self.action_in_flight {
            return format!("正在调用真实 Guardian 后端链路：{}。", action.label());
        }

        "单页任务流：先看推荐动作，再核对三域状态，继续向下滚动到步骤区与证据工作台。".to_string()
    }

    fn redesigned_overview_text(&self) -> String {
        let timestamp_text = self
            .latest_report
            .as_ref()
            .map(|report| compact_timestamp(&report.timestamp))
            .unwrap_or_else(|| "等待首次整机检查".to_string());
        let conclusion = self
            .latest_report
            .as_ref()
            .map(localized_dominant_summary)
            .unwrap_or_else(|| "Guardian 仍在等待首份健康报告。".to_string());

        [
            format!("当前阶段：{}", self.stage_summary_text()),
            format!("报告结论：{}（{}）", conclusion, timestamp_text),
            "阅读顺序：1 先看上方推荐动作 → 2 核对三域状态 → 3 向下按步骤区操作".to_string(),
            "滚动提示：继续向下可看到“确认修复 / 导出结果 / 证据工作台”，高风险动作不会与只读动作混排。"
                .to_string(),
        ]
        .join("\n")
    }

    fn redesigned_domain_text(&self, name: &str, report: Option<&DomainReport>) -> String {
        let Some(report) = report else {
            return [
                "健康状态：待采集".to_string(),
                "当前摘要：等待首份健康检查结果。".to_string(),
                "关注点：采集后会显示失败分类或恢复模式。".to_string(),
            ]
            .join("\n");
        };

        let focus_line = if name == "profile"
            && report
                .evidence
                .iter()
                .any(|item| item.key == "guided_recovery_mode" && item.value == "true")
        {
            "关注点：仅支持引导式恢复（保持只读）".to_string()
        } else if let Some(failure_classes) = report
            .evidence
            .iter()
            .find(|item| item.key == "failure_classes")
            .map(|item| localized_evidence_value(&item.value))
        {
            format!("关注点：{}", truncate_text(&failure_classes, 26))
        } else if report.status == StatusLevel::Ok {
            "关注点：当前没有额外风险。".to_string()
        } else {
            "关注点：完整证据请在下方证据工作台查看。".to_string()
        };

        [
            format!("健康状态：{}", status_name_zh(report.status)),
            format!(
                "当前摘要：{}",
                truncate_text(&localized_domain_summary(name, &report.summary), 42)
            ),
            focus_line,
        ]
        .join("\n")
    }

    fn redesigned_details_text(&self) -> String {
        let focus = self
            .latest_report
            .as_ref()
            .and_then(|report| report.notes.first().map(|note| localized_report_note(note)))
            .or_else(|| {
                self.latest_report.as_ref().and_then(|report| {
                    report
                        .actions
                        .first()
                        .map(|action| localized_detail_text(&action.description))
                })
            })
            .unwrap_or_else(|| "当前没有额外的总体风险说明。".to_string());

        let mut lines = vec![
            "当前推荐".to_string(),
            format!("- {}", self.recommended_action_summary()),
            String::new(),
            "本轮风险焦点".to_string(),
            format!("- {}", truncate_text(&focus, 120)),
            String::new(),
            "如何继续".to_string(),
            "- 先在上方步骤区完成当前动作，再回到这里查看结果。".to_string(),
            "- 切到“执行轨迹”查看真实命令、最近日志与执行节奏。".to_string(),
            "- 切到“原始 JSON”查看结构化原始证据与字段。".to_string(),
            String::new(),
            "最新结果出口".to_string(),
            format!(
                "- 诊断目录：{}",
                truncate_text(&display_path(self.latest_bundle_root.as_ref()), 72)
            ),
            format!(
                "- Profile 诊断：{}",
                truncate_text(&display_path(self.latest_profile_output.as_ref()), 72)
            ),
        ];

        if self.latest_bundle_archive.is_some() {
            lines.push(format!(
                "- 压缩包：{}",
                truncate_text(&display_path(self.latest_bundle_archive.as_ref()), 72)
            ));
        }

        if let Some(error) = &self.last_error {
            lines.push(String::new());
            lines.push("最近异常".to_string());
            lines.push(format!("- {}", truncate_text(error, 120)));
        } else if let Some(stderr_line) = self
            .latest_stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
        {
            lines.push(String::new());
            lines.push("最近异常".to_string());
            lines.push(format!("- {}", truncate_text(stderr_line, 120)));
        }

        lines.join("\n")
    }

    #[allow(dead_code)]
    fn hero_banner_text(&self) -> String {
        if std::env::var_os("GUARDIAN_GUI_FORCE_LEGACY_COPY").is_none() {
            return self.redesigned_hero_banner_text();
        }

        let next_move = if let Some(action) = self.action_in_flight {
            format!(
                "当前正在执行：{}。为保证证据一致性，界面已暂时锁定其他异步动作。",
                action.label()
            )
        } else if let Some(report) = &self.latest_report {
            if let Some(action) = report.actions.first() {
                let mode = if action.requires_confirmation {
                    "需要确认的修复"
                } else {
                    "只读诊断 / 预览"
                };
                format!("建议下一步：{}（{}）", action.command, mode)
            } else {
                "当前没有额外动作要求，可直接查看三域状态卡、建议与最新产物。".to_string()
            }
        } else {
            "先执行“刷新整机检查”，生成首份真实健康报告。".to_string()
        };

        let status_text = self
            .latest_report
            .as_ref()
            .map(|report| status_name_zh(report.status))
            .unwrap_or("待采集");
        let summary_text = self
            .latest_report
            .as_ref()
            .map(localized_dominant_summary)
            .unwrap_or_else(|| "等待首份整机健康报告。".to_string());

        [
            format!("当前状态：{status_text} ｜ {summary_text}"),
            next_move,
        ]
        .join("\n")
    }

    #[allow(dead_code)]
    fn hero_subtitle_text(&self) -> String {
        if std::env::var_os("GUARDIAN_GUI_FORCE_LEGACY_COPY").is_none() {
            return self.redesigned_hero_subtitle_text();
        }

        if let Some(action) = self.action_in_flight {
            return format!("正在调用真实 Guardian 后端链路：{}。", action.label());
        }

        "把读屏摘要、风险动作和证据出口收在同一页，先判断，再动手。".to_string()
    }

    fn handle_control_colors(&self, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let hdc = HDC(wparam.0 as *mut c_void);
        let control = HWND(lparam.0 as *mut c_void);
        if hdc.0.is_null() || control.0.is_null() {
            return LRESULT(0);
        }

        let (brush, background, text_color, transparent) = if [
            self.controls.actions_hint,
            self.controls.repair_hint,
            self.controls.artifacts_hint,
        ]
        .contains(&control)
        {
            (self.theme.base_brush, BG_BASE, TEXT_SECONDARY, false)
        } else if [
            self.controls.actions_group,
            self.controls.repair_group,
            self.controls.artifacts_group,
        ]
        .contains(&control)
        {
            (self.theme.base_brush, BG_BASE, TEXT_PRIMARY, false)
        } else if control == self.controls.details {
            (self.theme.input_brush, BG_INPUT, TEXT_PRIMARY, false)
        } else if control == self.controls.raw_json || control == self.controls.activity {
            (
                self.theme.surface_alt_brush,
                BG_SURFACE_ALT,
                TEXT_SECONDARY,
                false,
            )
        } else if msg == WM_CTLCOLORBTN {
            (self.theme.base_brush, BG_BASE, TEXT_SECONDARY, false)
        } else {
            (self.theme.base_brush, BG_BASE, TEXT_PRIMARY, true)
        };

        unsafe {
            let _ = SetTextColor(hdc, text_color);
            if transparent {
                let _ = SetBkMode(hdc, TRANSPARENT);
            } else {
                let _ = SetBkMode(hdc, OPAQUE);
                let _ = SetBkColor(hdc, background);
            }
        }

        LRESULT(brush.0 as isize)
    }

    fn invalidate_main_window(&self) {
        if self.main_hwnd.0.is_null() {
            return;
        }

        unsafe {
            let _ = InvalidateRect(self.main_hwnd, None, true);
            let _ = UpdateWindow(self.main_hwnd);
        }
    }

    fn redraw_window_tree(&self, hwnd: HWND) {
        if hwnd.0.is_null() {
            return;
        }

        unsafe {
            let _ = InvalidateRect(hwnd, None, true);
            let _ = UpdateWindow(hwnd);
        }

        for control in self.controls.all_handles() {
            if control.0.is_null() || !unsafe { IsWindowVisible(control).as_bool() } {
                continue;
            }

            unsafe {
                let _ = InvalidateRect(control, None, true);
                let _ = UpdateWindow(control);
            }
        }
    }

    fn draw_button(&self, draw_item: &DRAWITEMSTRUCT) -> LRESULT {
        if draw_item.CtlType != ODT_BUTTON {
            return LRESULT(0);
        }

        let control_id = draw_item.CtlID as i32;
        let is_disabled = (draw_item.itemState.0 & ODS_DISABLED.0) != 0;
        let is_selected = (draw_item.itemState.0 & ODS_SELECTED.0) != 0;
        let is_active = control_action(control_id)
            .map(|action| self.action_in_flight == Some(action))
            .unwrap_or(false);
        if let Some(step) = WorkflowStep::from_control_id(control_id) {
            paint_step_nav(
                draw_item.hDC,
                draw_item.rcItem,
                &self.theme,
                step.label(),
                self.step_visual_state(step),
                is_selected,
            );
            return LRESULT(1);
        }
        if control_id == ID_EXPERT_TOGGLE {
            paint_collapsible(
                draw_item.hDC,
                draw_item.rcItem,
                &self.theme,
                "专家详情",
                self.expert_details_expanded,
                is_selected,
            );
            return LRESULT(1);
        }

        let visual_state = if is_disabled {
            ButtonVisualState::Disabled
        } else if is_selected {
            ButtonVisualState::Pressed
        } else if is_active || self.is_current_detail_tab_button(control_id) {
            ButtonVisualState::Active
        } else {
            ButtonVisualState::Normal
        };
        paint_button(
            draw_item.hDC,
            draw_item.rcItem,
            &self.theme,
            &window_text_string(draw_item.hwndItem),
            button_kind(control_id),
            visual_state,
        );

        LRESULT(1)
    }

    fn draw_static_control(&self, draw_item: &DRAWITEMSTRUCT) -> LRESULT {
        if draw_item.CtlType != ODT_STATIC {
            return LRESULT(0);
        }

        match draw_item.CtlID as i32 {
            ID_HERO_BANNER => {
                paint_banner(
                    draw_item.hDC,
                    draw_item.rcItem,
                    &self.theme,
                    self.latest_report.as_ref().map(|report| report.status),
                    &self.banner_title_text(),
                    &self.banner_subtitle_text(),
                    &self.banner_timestamp_text(),
                );
                LRESULT(1)
            }
            ID_OVERVIEW_EDIT => {
                paint_card(
                    draw_item.hDC,
                    draw_item.rcItem,
                    &self.theme,
                    self.current_step_card_header(),
                    &window_text_string(draw_item.hwndItem),
                    self.current_step_card_status(),
                    true,
                );
                LRESULT(1)
            }
            ID_CODEX_EDIT => {
                paint_card(
                    draw_item.hDC,
                    draw_item.rcItem,
                    &self.theme,
                    domain_title("codex"),
                    &window_text_string(draw_item.hwndItem),
                    self.latest_report
                        .as_ref()
                        .map(|report| report.domains.codex.status),
                    false,
                );
                LRESULT(1)
            }
            ID_DOCKER_EDIT => {
                paint_card(
                    draw_item.hDC,
                    draw_item.rcItem,
                    &self.theme,
                    domain_title("docker_wsl"),
                    &window_text_string(draw_item.hwndItem),
                    self.latest_report
                        .as_ref()
                        .map(|report| report.domains.docker_wsl.status),
                    false,
                );
                LRESULT(1)
            }
            ID_PROFILE_EDIT => {
                paint_card(
                    draw_item.hDC,
                    draw_item.rcItem,
                    &self.theme,
                    domain_title("profile"),
                    &window_text_string(draw_item.hwndItem),
                    self.latest_report
                        .as_ref()
                        .map(|report| report.domains.profile.status),
                    false,
                );
                LRESULT(1)
            }
            _ => LRESULT(0),
        }
    }

    fn refresh_button_states(&self) {
        let button_states = GuiButtonStates::from_runtime(
            self.action_in_flight,
            self.latest_bundle_root.as_ref(),
            self.latest_bundle_archive.as_ref(),
            self.latest_profile_output.as_ref(),
        );

        set_text(
            self.controls.run_check_button,
            &action_button_text(GuiAction::RunCheck, self.action_in_flight),
        );
        set_text(
            self.controls.repair_codex_button,
            &action_button_text(GuiAction::RepairCodexConfirm, self.action_in_flight),
        );
        set_text(
            self.controls.repair_docker_button,
            &action_button_text(GuiAction::RepairDockerConfirm, self.action_in_flight),
        );
        set_text(
            self.controls.diagnose_profile_button,
            &action_button_text(GuiAction::DiagnoseProfile, self.action_in_flight),
        );
        set_text(
            self.controls.export_bundle_button,
            &action_button_text(GuiAction::ExportBundle, self.action_in_flight),
        );
        set_text(
            self.controls.export_bundle_zip_button,
            &action_button_text(GuiAction::ExportBundleZip, self.action_in_flight),
        );
        set_text(
            self.controls.export_bundle_zip_retain_button,
            &action_button_text(GuiAction::ExportBundleZipRetain5, self.action_in_flight),
        );

        set_enabled(
            self.controls.run_check_button,
            button_states.async_actions_enabled,
        );
        set_enabled(
            self.controls.repair_codex_button,
            button_states.async_actions_enabled,
        );
        set_enabled(
            self.controls.repair_docker_button,
            button_states.async_actions_enabled,
        );
        set_enabled(
            self.controls.diagnose_profile_button,
            button_states.async_actions_enabled,
        );
        set_enabled(
            self.controls.export_bundle_button,
            button_states.async_actions_enabled,
        );
        set_enabled(
            self.controls.export_bundle_zip_button,
            button_states.async_actions_enabled,
        );
        set_enabled(
            self.controls.export_bundle_zip_retain_button,
            button_states.async_actions_enabled,
        );
        set_enabled(
            self.controls.open_latest_bundle_button,
            button_states.open_latest_bundle_enabled,
        );
        set_enabled(
            self.controls.open_latest_bundle_zip_button,
            button_states.open_latest_bundle_zip_enabled,
        );
        set_enabled(
            self.controls.open_last_profile_button,
            button_states.open_last_profile_enabled,
        );
        set_enabled(self.controls.open_audits_button, true);
        set_enabled(self.controls.open_bundles_button, true);
        set_enabled(self.controls.open_data_button, true);
    }

    fn refresh_window_title(&self) {
        if self.main_hwnd.0.is_null() {
            return;
        }

        let title = if std::env::var_os("GUARDIAN_GUI_FORCE_LEGACY_COPY").is_none() {
            redesigned_window_title_text(
                self.latest_report.as_ref().map(|report| report.status),
                self.action_in_flight,
            )
        } else {
            window_title_text(
                self.latest_report.as_ref().map(|report| report.status),
                self.action_in_flight,
                &self.last_action_text,
            )
        };

        set_text(self.main_hwnd, &title);
    }

    fn overview_text(&self) -> String {
        if std::env::var_os("GUARDIAN_GUI_FORCE_LEGACY_COPY").is_none() {
            return self.current_step_card_text();
        }

        let mut lines = Vec::new();
        if let Some(report) = &self.latest_report {
            lines.push(format!("总体状态：{}", status_name_zh(report.status)));
            lines.push(format!(
                "报告时间：{}",
                compact_timestamp(&report.timestamp)
            ));
            lines.push(format!("当前摘要：{}", localized_dominant_summary(report)));
        } else {
            lines.push("总体状态：待采集".to_string());
            lines.push("报告时间：等待首次整机检查".to_string());
            lines.push("当前摘要：Guardian 仍在等待首份健康报告。".to_string());
        }

        lines.push(String::new());
        lines.push(format!("最近动作：{}", self.last_action_text));
        if let Some(exit_code) = self.latest_exit_code {
            lines.push(format!("最近退出码：{exit_code}"));
        }
        if let Some(action) = self.action_in_flight {
            lines.push(format!("当前状态：正在执行 {}", action.label()));
        } else {
            lines.push("当前状态：空闲".to_string());
        }
        if let Some(error) = &self.last_error {
            lines.push(format!("最近错误：{error}"));
        }

        lines.join("\n")
    }

    fn domain_text(&self, name: &str, report: Option<&DomainReport>) -> String {
        if std::env::var_os("GUARDIAN_GUI_FORCE_LEGACY_COPY").is_none() {
            return self.redesigned_domain_text(name, report);
        }

        let Some(report) = report else {
            return [
                "健康状态：待采集".to_string(),
                "摘要：等待首次健康检查结果。".to_string(),
                String::new(),
                "说明：该区域会展示失败分类、风险摘要和关键证据。".to_string(),
            ]
            .join("\n");
        };

        let mut lines = vec![
            format!("健康状态：{}", status_name_zh(report.status)),
            format!("摘要：{}", localized_domain_summary(name, &report.summary)),
        ];

        if let Some(failure_classes) = report
            .evidence
            .iter()
            .find(|item| item.key == "failure_classes")
            .map(|item| localized_evidence_value(&item.value))
        {
            lines.push(format!("失败分类：{}", failure_classes));
        }
        if name == "profile"
            && report
                .evidence
                .iter()
                .any(|item| item.key == "guided_recovery_mode" && item.value == "true")
        {
            lines.push("恢复模式：引导式恢复（保持只读）".to_string());
        }

        if !report.notes.is_empty() {
            lines.push(String::new());
            lines.push("风险焦点：".to_string());
            for note in report.notes.iter().take(1) {
                lines.push(format!("- {}", localized_report_note(note)));
            }
            if report.notes.len() > 1 {
                lines.push(format!(
                    "- 其余 {} 条已收起，详见“建议与风险”区域。",
                    report.notes.len() - 1
                ));
            }
        }

        if !report.evidence.is_empty() {
            lines.push(String::new());
            lines.push("关键快照：".to_string());
            for item in report.evidence.iter().take(1) {
                lines.push(format!(
                    "- {}：{}",
                    compact_evidence_label(name, &item.key),
                    compact_evidence_value(name, &item.key, &item.value)
                ));
            }
            if report.evidence.len() > 1 {
                lines.push(format!(
                    "- 其余 {} 条证据已收起，完整内容见原始 JSON。",
                    report.evidence.len() - 1
                ));
            }
        }

        lines.join("\n")
    }

    fn details_text(&self) -> String {
        if std::env::var_os("GUARDIAN_GUI_FORCE_LEGACY_COPY").is_none() {
            return self.redesigned_details_text();
        }

        let mut lines = Vec::new();

        if let Some(report) = &self.latest_report {
            lines.push("当前结论".to_string());
            lines.push(format!("- {}", localized_dominant_summary(report)));

            lines.push(String::new());
            lines.push("优先关注".to_string());
            if let Some(note) = report.notes.first() {
                lines.push(format!("- {}", localized_report_note(note)));
                if report.notes.len() > 1 {
                    lines.push(format!(
                        "- 另外 {} 条风险已收起，完整证据见右侧原始 JSON。",
                        report.notes.len() - 1
                    ));
                }
            } else {
                lines.push("- 当前没有额外的总体风险说明。".to_string());
            }

            lines.push(String::new());
            lines.push("建议先做".to_string());
            if let Some(action) = report.actions.first() {
                let confirm_suffix = if action.requires_confirmation {
                    "需要确认"
                } else {
                    "只读/预览"
                };
                lines.push(format!(
                    "- [{}] {}",
                    confirm_suffix,
                    localized_action_description(&action.description)
                ));
                lines.push(format!("  命令：{}", action.command));
                if report.actions.len() > 1 {
                    lines.push(format!(
                        "- 另外 {} 个候选动作已收起。",
                        report.actions.len() - 1
                    ));
                }
            } else {
                lines.push("- 当前没有额外动作要求，可继续查看三域卡片与导出区。".to_string());
            }
        } else {
            lines.push("当前结论".to_string());
            lines.push("- 尚未采集到健康报告。".to_string());

            lines.push(String::new());
            lines.push("建议先做".to_string());
            lines.push("- 先执行“刷新整机检查”，生成首份真实健康报告。".to_string());
        }

        lines.push(String::new());
        lines.push("结果出口".to_string());
        lines.push(format!(
            "- 最新诊断包：{}",
            truncate_text(&display_path(self.latest_bundle_root.as_ref()), 52)
        ));
        lines.push(format!(
            "- 最新 Profile 诊断：{}",
            truncate_text(&display_path(self.latest_profile_output.as_ref()), 52)
        ));
        if self.latest_bundle_archive.is_some() {
            lines.push(format!(
                "- 最新压缩包：{}",
                truncate_text(&display_path(self.latest_bundle_archive.as_ref()), 52)
            ));
        }

        if let Some(error) = &self.last_error {
            lines.push(String::new());
            lines.push("最近异常".to_string());
            lines.push(format!("- {}", truncate_text(error, 88)));
        } else if let Some(stderr_line) = self
            .latest_stderr
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
        {
            lines.push(String::new());
            lines.push("最近异常".to_string());
            lines.push(format!("- {}", truncate_text(stderr_line, 88)));
        }

        lines.join("\n")
    }

    fn raw_json_text(&self) -> String {
        if self.latest_raw_json.is_empty() {
            return "尚未捕获任何 JSON 输出。".to_string();
        }

        self.latest_raw_json.clone()
    }

    fn activity_text(&self) -> String {
        if self.activity_log.is_empty() {
            return "当前还没有活动记录。".to_string();
        }

        self.activity_log.join("\n")
    }

    fn show_error_dialog(&self, title: &str, message: &str) {
        self.show_message_dialog(title, message, MESSAGEBOX_STYLE(MB_OK.0 | MB_ICONERROR.0));
    }

    fn show_info_dialog(&self, title: &str, message: &str) {
        self.show_message_dialog(
            title,
            message,
            MESSAGEBOX_STYLE(MB_OK.0 | MB_ICONINFORMATION.0),
        );
    }

    fn show_message_dialog(&self, title: &str, message: &str, style: MESSAGEBOX_STYLE) {
        if self.main_hwnd.0.is_null() {
            return;
        }

        let wide_title = to_wide(title);
        let wide_message = to_wide(message);
        unsafe {
            let _ = MessageBoxW(
                self.main_hwnd,
                PCWSTR(wide_message.as_ptr()),
                PCWSTR(wide_title.as_ptr()),
                style,
            );
        }
    }
}

fn register_window_class(instance: HINSTANCE) -> Result<(), GuardianError> {
    let cursor = unsafe {
        LoadCursorW(None, IDC_ARROW)
            .map_err(|error| GuardianError::invalid_state(format!("LoadCursorW failed: {error}")))?
    };
    let class = WNDCLASSW {
        hCursor: cursor,
        hInstance: instance,
        lpszClassName: WINDOW_CLASS_NAME,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(gui_window_proc),
        hbrBackground: theme::create_solid_brush(BG_BASE),
        ..Default::default()
    };

    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        return Err(GuardianError::invalid_state(
            "Guardian 稳定性控制台的 RegisterClassW 返回了 0",
        ));
    }

    Ok(())
}

unsafe extern "system" fn gui_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CREATE => {
            let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            let state_ptr = create.lpCreateParams as *mut GuiWindowState;
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state_ptr as isize);
            }
            if let Some(state) = unsafe { state_mut(hwnd) }
                && let Err(error) = state.initialize_window(hwnd)
            {
                state.last_error = Some(error.to_string());
                state.log(format!("{GUARDIAN_PRODUCT_NAME_ZH}初始化失败：{error}。"));
                state.refresh_controls();
            }
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize.x = WINDOW_MIN_WIDTH;
            info.ptMinTrackSize.y = WINDOW_MIN_HEIGHT;
            LRESULT(0)
        }
        WM_SIZE => {
            if let Some(state) = unsafe { state_mut(hwnd) } {
                let _ = state.layout_controls(hwnd);
            }
            LRESULT(0)
        }
        WM_VSCROLL => {
            if let Some(state) = unsafe { state_mut(hwnd) } {
                state.handle_vertical_scroll(hwnd, wparam);
            }
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            if let Some(state) = unsafe { state_mut(hwnd) } {
                state.handle_mouse_wheel(hwnd, wparam);
            }
            LRESULT(0)
        }
        WM_TIMER => {
            if wparam.0 == GUI_TIMER_ID
                && let Some(state) = unsafe { state_mut(hwnd) }
                && state.poll_worker_messages()
            {
                state.refresh_controls();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            if let Some(state) = unsafe { state_mut(hwnd) } {
                let control_id = (wparam.0 & 0xffff) as i32;
                state.handle_command(control_id);
                state.refresh_controls();
            }
            LRESULT(0)
        }
        WM_DRAWITEM => {
            if let Some(state) = unsafe { state_mut(hwnd) } {
                let draw_item = unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) };
                if draw_item.CtlType == ODT_STATIC {
                    return state.draw_static_control(draw_item);
                }
                return state.draw_button(draw_item);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORBTN => {
            if let Some(state) = unsafe { state_mut(hwnd) } {
                return state.handle_control_colors(msg, wparam, lparam);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = unsafe { KillTimer(hwnd, GUI_TIMER_ID) };
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_NCDESTROY => {
            let state_ptr = unsafe { take_state_ptr(hwnd) };
            if !state_ptr.is_null() {
                drop(unsafe { Box::from_raw(state_ptr) });
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn state_mut(hwnd: HWND) -> Option<&'static mut GuiWindowState> {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiWindowState };
    unsafe { ptr.as_mut() }
}

unsafe fn take_state_ptr(hwnd: HWND) -> *mut GuiWindowState {
    let ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut GuiWindowState };
    let _ = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
    ptr
}

fn execute_gui_action(
    action: GuiAction,
    current_exe: &Path,
    spec: GuiCommandSpec,
) -> Result<GuiActionSuccess, GuardianError> {
    let JsonCommandSuccess {
        exit_code,
        report,
        stdout,
        stderr,
    } = run_health_report_command(current_exe, &spec.args, action.label())?;

    let latest_bundle_root = if action.is_bundle_export() {
        bundle_root_from_notes(&report.notes)
    } else {
        None
    };
    let latest_bundle_archive = if action.produces_bundle_archive() {
        bundle_archive_from_notes(&report.notes)
    } else {
        None
    };
    let latest_profile_output = if action == GuiAction::DiagnoseProfile {
        profile_diagnosis_from_notes(&report.notes).or(spec.profile_output)
    } else {
        None
    };

    Ok(GuiActionSuccess {
        exit_code,
        report,
        raw_json: stdout,
        stderr,
        latest_bundle_root,
        latest_bundle_archive,
        latest_profile_output,
    })
}

fn localized_success_detail(action: GuiAction, success: &GuiActionSuccess) -> String {
    match action {
        GuiAction::ExportBundle => success
            .latest_bundle_root
            .as_ref()
            .map(|path| localized_detail_text(&format!("bundle saved to {}", path.display())))
            .unwrap_or_else(|| localized_dominant_summary(&success.report)),
        GuiAction::ExportBundleZip | GuiAction::ExportBundleZipRetain5 => success
            .latest_bundle_archive
            .as_ref()
            .map(|path| localized_detail_text(&format!("bundle zip saved to {}", path.display())))
            .or_else(|| {
                success.latest_bundle_root.as_ref().map(|path| {
                    localized_detail_text(&format!("bundle saved to {}", path.display()))
                })
            })
            .unwrap_or_else(|| localized_dominant_summary(&success.report)),
        GuiAction::DiagnoseProfile => success
            .latest_profile_output
            .as_ref()
            .map(|path| {
                localized_detail_text(&format!("profile diagnosis saved to {}", path.display()))
            })
            .unwrap_or_else(|| localized_dominant_summary(&success.report)),
        _ => localized_dominant_summary(&success.report),
    }
}

fn localized_evidence_label(domain: &str, key: &str) -> String {
    let label = match (domain, key) {
        ("codex", "codex_home") => Some("Codex 主目录"),
        ("codex", "codex_version") => Some("CLI 版本"),
        ("codex", "history_lines") => Some("history.jsonl 行数"),
        ("codex", "session_files") => Some("会话文件数"),
        ("codex", "repair_script_present") => Some("修复脚本已就绪"),
        ("codex", "state_files") => Some("state 数据库文件数"),
        ("codex", "latest_state_file") => Some("最新 state 数据库"),
        ("codex", "threads_total") => Some("threads 总数"),
        ("codex", "stale_rows") => Some("stale rows"),
        ("codex", "codex_tui_log_path") => Some("TUI 日志路径"),
        ("codex", "codex_tui_signal_count") => Some("TUI 风险信号数"),
        ("codex", "codex_tui_matches") => Some("TUI 命中片段"),
        ("docker_wsl", "docker_version") => Some("Docker 版本"),
        ("docker_wsl", "docker_info") => Some("Docker 运行信息"),
        ("docker_wsl", "wsl_list") => Some("WSL 列表"),
        ("docker_wsl", "wslconfig_path") => Some(".wslconfig 路径"),
        ("docker_wsl", "wslconfig_exists") => Some(".wslconfig 已存在"),
        ("docker_wsl", "wslconfig_wsl2_has_memory") => Some("wsl2.memory 已配置"),
        ("docker_wsl", "wslconfig_wsl2_has_processors") => Some("wsl2.processors 已配置"),
        ("docker_wsl", "wslconfig_wsl2_has_swap") => Some("wsl2.swap 已配置"),
        ("docker_wsl", "wslconfig_experimental_has_auto_memory_reclaim") => {
            Some("experimental.autoMemoryReclaim 已配置")
        }
        ("profile", "collector_mode") => Some("采集模式"),
        ("profile", "current_behavior") => Some("当前行为"),
        ("profile", "application_event_count") => Some("Application 事件数"),
        ("profile", "operational_event_count") => Some("Operational 事件数"),
        ("profile", "latest_event_id") => Some("最新事件 ID"),
        ("profile", "latest_event_date") => Some("最新事件时间"),
        ("profile", "latest_event_log") => Some("最新事件日志"),
        ("profile", "latest_event_description") => Some("最新事件说明"),
        ("profile", "locking_process_name") => Some("锁定进程"),
        ("profile", "locking_process_pid") => Some("锁定进程 PID"),
        (_, "failure_classes") => Some("失败分类"),
        (_, "guided_recovery_mode") => Some("引导恢复模式"),
        (_, "guided_recovery_failure_classes") => Some("引导恢复分类"),
        _ => None,
    };

    label
        .map(|value| format!("{value}（{key}）"))
        .unwrap_or_else(|| key.to_string())
}

fn compact_evidence_label(domain: &str, key: &str) -> String {
    localized_evidence_label(domain, key)
        .split('（')
        .next()
        .unwrap_or(key)
        .to_string()
}

fn localized_evidence_value(value: &str) -> String {
    match value {
        "true" => "是".to_string(),
        "false" => "否".to_string(),
        "none" => "无".to_string(),
        "read_only" => "只读".to_string(),
        "eventlog_read_only" => "事件日志只读采集".to_string(),
        other => other.to_string(),
    }
}

fn compact_evidence_value(domain: &str, key: &str, value: &str) -> String {
    let localized = localized_evidence_value(value);
    match (domain, key) {
        ("docker_wsl", "docker_version") => "已采集 Docker 版本详情".to_string(),
        ("docker_wsl", "docker_info") => "已采集 Docker 运行信息".to_string(),
        ("docker_wsl", "wsl_list") => "已采集 WSL 发行版列表".to_string(),
        ("codex", "codex_tui_matches") => "已命中 TUI 风险片段".to_string(),
        ("profile", "latest_event_description") => truncate_text(&localized, 38),
        _ if localized.starts_with('{') || localized.starts_with('[') => {
            "已采集结构化输出，详见原始 JSON".to_string()
        }
        _ if localized.len() > 56 => truncate_text(&localized, 56),
        _ => localized,
    }
}

fn compact_timestamp(value: &str) -> String {
    value.split('.').next().unwrap_or(value).replace('T', " ")
}

fn truncate_text(value: &str, max_chars: usize) -> String {
    let mut result = String::new();
    for (count, ch) in value.chars().enumerate() {
        if count >= max_chars {
            result.push('…');
            return result;
        }
        result.push(ch);
    }
    result
}

fn create_static_text(
    parent: HWND,
    instance: HINSTANCE,
    control_id: i32,
    text: &str,
) -> Result<HWND, GuardianError> {
    create_control_window(
        parent,
        instance,
        WC_STATIC,
        text,
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
        control_id,
    )
}

fn create_owner_draw_static(
    parent: HWND,
    instance: HINSTANCE,
    control_id: i32,
) -> Result<HWND, GuardianError> {
    create_control_window(
        parent,
        instance,
        WC_STATIC,
        "",
        WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | STATIC_OWNERDRAW_STYLE),
        control_id,
    )
}

fn create_button(
    parent: HWND,
    instance: HINSTANCE,
    control_id: i32,
    text: &str,
) -> Result<HWND, GuardianError> {
    create_control_window(
        parent,
        instance,
        WC_BUTTON,
        text,
        WINDOW_STYLE(
            WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_OWNERDRAW as u32 | BS_MULTILINE as u32,
        ),
        control_id,
    )
}

fn create_group_box(parent: HWND, instance: HINSTANCE, text: &str) -> Result<HWND, GuardianError> {
    create_static_text(parent, instance, 0, text)
}

fn create_readonly_edit(
    parent: HWND,
    instance: HINSTANCE,
    control_id: i32,
) -> Result<HWND, GuardianError> {
    create_control_window(
        parent,
        instance,
        WC_EDIT,
        "",
        WINDOW_STYLE(
            WS_CHILD.0
                | WS_VISIBLE.0
                | WS_VSCROLL.0
                | ES_LEFT as u32
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32
                | ES_WANTRETURN as u32
                | ES_READONLY as u32,
        ),
        control_id,
    )
}

fn create_control_window(
    parent: HWND,
    instance: HINSTANCE,
    class_name: PCWSTR,
    text: &str,
    style: WINDOW_STYLE,
    control_id: i32,
) -> Result<HWND, GuardianError> {
    let wide_text = to_wide(text);
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR(wide_text.as_ptr()),
            WINDOW_STYLE(style.0 | WS_CLIPSIBLINGS.0),
            0,
            0,
            0,
            0,
            parent,
            HMENU(control_id as isize as *mut c_void),
            instance,
            None,
        )
        .map_err(|error| {
            GuardianError::invalid_state(format!(
                "CreateWindowExW failed for control `{text}`: {error}"
            ))
        })
    }
}

fn apply_font(hwnd: HWND, font: HFONT) {
    if hwnd.0.is_null() || font.0.is_null() {
        return;
    }

    unsafe {
        let _ = SendMessageW(hwnd, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    }
}

fn move_button_row(
    parent: HWND,
    buttons: &[(i32, i32)],
    start_x: i32,
    y: i32,
    height: i32,
) -> Result<(), GuardianError> {
    let mut x = start_x;
    for (control_id, width) in buttons {
        let hwnd = control_handle(parent, *control_id)?;
        move_window(hwnd, x, y, *width, height)?;
        x += *width + SPACING_S;
    }
    Ok(())
}

fn control_handle(parent: HWND, control_id: i32) -> Result<HWND, GuardianError> {
    let ptr = control_id as isize as *mut c_void;
    let hwnd = unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetDlgItem(parent, control_id)
            .map_err(|error| GuardianError::invalid_state(format!("GetDlgItem failed: {error}")))?
    };
    if hwnd.0.is_null() && !ptr.is_null() {
        return Err(GuardianError::invalid_state(format!(
            "control `{control_id}` returned a null HWND"
        )));
    }
    Ok(hwnd)
}

fn move_window(hwnd: HWND, x: i32, y: i32, width: i32, height: i32) -> Result<(), GuardianError> {
    unsafe {
        MoveWindow(hwnd, x, y, width, height, false)
            .map_err(|error| GuardianError::invalid_state(format!("MoveWindow failed: {error}")))
    }
}

fn ensure_directory(path: PathBuf) -> Result<PathBuf, GuardianError> {
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn set_text(hwnd: HWND, value: &str) {
    if hwnd.0.is_null() {
        return;
    }

    let normalized = value.replace('\n', "\r\n");
    if window_text_string(hwnd) == normalized {
        return;
    }

    let wide = to_wide(&normalized);
    unsafe {
        let _ = SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
        if !IsWindowVisible(hwnd).as_bool() {
            return;
        }
        // Owner-draw and colored static controls do not always repaint on the
        // first content update, especially on the Review step. Force a redraw
        // so the first frame reflects the latest localized summaries.
        let _ = InvalidateRect(hwnd, None, true);
        let _ = UpdateWindow(hwnd);
    }
}

fn set_enabled(hwnd: HWND, enabled: bool) {
    if hwnd.0.is_null() {
        return;
    }

    unsafe {
        let _ = EnableWindow(hwnd, enabled);
    }
}

fn set_visible(hwnd: HWND, visible: bool) {
    if hwnd.0.is_null() {
        return;
    }

    unsafe {
        if IsWindowVisible(hwnd).as_bool() == visible {
            return;
        }
        let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
    }
}

fn set_edit_padding(hwnd: HWND, left: i32, top: i32, right: i32, bottom: i32) {
    if hwnd.0.is_null() {
        return;
    }

    let mut rect = RECT::default();
    unsafe {
        if GetClientRect(hwnd, &mut rect).is_err() {
            return;
        }
    }

    let inset_rect = RECT {
        left,
        top,
        right: (rect.right - right).max(left + 1),
        bottom: (rect.bottom - bottom).max(top + 1),
    };

    unsafe {
        let _ = SendMessageW(
            hwnd,
            EM_SETRECTNP,
            WPARAM(0),
            LPARAM((&inset_rect as *const RECT) as isize),
        );
    }
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn display_path(path: Option<&PathBuf>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "<尚未生成>".to_string())
}

fn action_button_text(action: GuiAction, action_in_flight: Option<GuiAction>) -> String {
    if action_in_flight == Some(action) {
        format!("{}（执行中）", action.label())
    } else {
        match action {
            GuiAction::RunCheck => "刷新整机检查".to_string(),
            GuiAction::DiagnoseProfile => "只读诊断 Profile".to_string(),
            GuiAction::RepairCodexConfirm => "确认修复 Codex".to_string(),
            GuiAction::RepairDockerConfirm => "确认修复 Docker / WSL".to_string(),
            GuiAction::ExportBundle => "导出诊断目录".to_string(),
            GuiAction::ExportBundleZip => "导出并压缩".to_string(),
            GuiAction::ExportBundleZipRetain5 => "导出并保留 5 份".to_string(),
            _ => action.label().to_string(),
        }
    }
}

fn control_action(control_id: i32) -> Option<GuiAction> {
    match control_id {
        ID_RUN_CHECK => Some(GuiAction::RunCheck),
        ID_REPAIR_CODEX => Some(GuiAction::RepairCodexConfirm),
        ID_REPAIR_DOCKER => Some(GuiAction::RepairDockerConfirm),
        ID_DIAGNOSE_PROFILE => Some(GuiAction::DiagnoseProfile),
        ID_EXPORT_BUNDLE => Some(GuiAction::ExportBundle),
        ID_EXPORT_BUNDLE_ZIP => Some(GuiAction::ExportBundleZip),
        ID_EXPORT_BUNDLE_ZIP_RETAIN => Some(GuiAction::ExportBundleZipRetain5),
        ID_OPEN_LATEST_BUNDLE => Some(GuiAction::OpenLatestBundle),
        ID_OPEN_LATEST_BUNDLE_ZIP => Some(GuiAction::OpenLatestBundleZip),
        ID_OPEN_LAST_PROFILE => Some(GuiAction::OpenLastProfileDiagnosis),
        ID_OPEN_AUDITS => Some(GuiAction::OpenAuditsFolder),
        ID_OPEN_BUNDLES => Some(GuiAction::OpenBundlesFolder),
        ID_OPEN_DATA => Some(GuiAction::OpenGuardianDataFolder),
        _ => None,
    }
}

fn button_kind(control_id: i32) -> ButtonKind {
    match control_id {
        ID_RUN_CHECK | ID_EXPORT_BUNDLE => ButtonKind::Primary,
        ID_REPAIR_CODEX | ID_REPAIR_DOCKER => ButtonKind::Danger,
        ID_OPEN_LATEST_BUNDLE
        | ID_OPEN_LATEST_BUNDLE_ZIP
        | ID_OPEN_LAST_PROFILE
        | ID_OPEN_AUDITS
        | ID_OPEN_BUNDLES
        | ID_OPEN_DATA
        | ID_DETAIL_TAB_SUMMARY
        | ID_DETAIL_TAB_ACTIVITY
        | ID_DETAIL_TAB_JSON => ButtonKind::Ghost,
        _ => ButtonKind::Secondary,
    }
}

fn window_text(hwnd: HWND) -> Vec<u16> {
    let length = unsafe { GetWindowTextLengthW(hwnd) };
    if length <= 0 {
        return Vec::new();
    }

    let mut buffer = vec![0u16; length as usize + 1];
    let written = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    buffer.truncate(written as usize);
    buffer
}

fn window_text_string(hwnd: HWND) -> String {
    String::from_utf16_lossy(&window_text(hwnd))
}

fn redesigned_window_title_text(
    overall_status: Option<StatusLevel>,
    action_in_flight: Option<GuiAction>,
) -> String {
    let status = overall_status.map(status_name_zh).unwrap_or("待采集");
    if let Some(action) = action_in_flight {
        return format!(
            "{GUARDIAN_PRODUCT_NAME_ZH} - {status} - 正在执行：{}",
            action.label()
        );
    }

    format!("{GUARDIAN_PRODUCT_NAME_ZH} - {status}")
}

fn window_title_text(
    overall_status: Option<StatusLevel>,
    action_in_flight: Option<GuiAction>,
    last_action_text: &str,
) -> String {
    let status = overall_status.map(status_name_zh).unwrap_or("待采集");
    if let Some(action) = action_in_flight {
        return format!(
            "{GUARDIAN_PRODUCT_NAME_ZH} - {status} - 正在执行：{}",
            action.label()
        );
    }

    format!("{GUARDIAN_PRODUCT_NAME_ZH} - {status} - {last_action_text}")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        GuiAction, GuiButtonStates, GuiCommandSpec, action_button_text, window_title_text,
    };

    #[test]
    fn export_zip_retain_command_uses_expected_arguments() {
        let spec = GuiCommandSpec::build(GuiAction::ExportBundleZipRetain5)
            .expect("export command should build");
        let args = spec
            .args
            .iter()
            .map(|value| value.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            args,
            vec!["export", "bundle", "--json", "--zip", "--retain", "5"]
        );
    }

    #[test]
    fn diagnose_profile_command_builds_a_json_output_path() {
        let spec = GuiCommandSpec::build(GuiAction::DiagnoseProfile)
            .expect("diagnose command should build");
        let output = spec
            .profile_output
            .as_deref()
            .expect("profile output path should exist");

        assert_eq!(spec.args[0].to_string_lossy().as_ref(), "diagnose");
        assert_eq!(spec.args[1].to_string_lossy().as_ref(), "profile");
        assert_eq!(spec.args[2].to_string_lossy().as_ref(), "--json");
        assert_eq!(spec.args[3].to_string_lossy().as_ref(), "--output");
        assert_eq!(Path::new(&spec.args[4]), output);
    }

    #[test]
    fn button_states_disable_async_actions_while_busy() {
        let states = GuiButtonStates::from_runtime(
            Some(GuiAction::RunCheck),
            Some(&"C:\\bundle".into()),
            Some(&"C:\\bundle.zip".into()),
            Some(&"C:\\profile.json".into()),
        );

        assert!(!states.async_actions_enabled);
        assert!(!states.open_latest_bundle_enabled);
        assert!(!states.open_latest_bundle_zip_enabled);
        assert!(!states.open_last_profile_enabled);
    }

    #[test]
    fn button_states_enable_latest_paths_only_when_available() {
        let states = GuiButtonStates::from_runtime(
            None,
            Some(&"C:\\bundle".into()),
            None,
            Some(&"C:\\profile.json".into()),
        );

        assert!(states.async_actions_enabled);
        assert!(states.open_latest_bundle_enabled);
        assert!(!states.open_latest_bundle_zip_enabled);
        assert!(states.open_last_profile_enabled);
    }

    #[test]
    fn running_action_text_marks_only_current_action() {
        assert_eq!(
            action_button_text(GuiAction::ExportBundleZip, Some(GuiAction::ExportBundleZip)),
            "导出并压缩（执行中）"
        );
        assert_eq!(
            action_button_text(GuiAction::RunCheck, Some(GuiAction::ExportBundleZip)),
            "刷新整机检查"
        );
    }

    #[test]
    fn window_title_prefers_busy_state_when_action_running() {
        assert_eq!(
            window_title_text(
                Some(guardian_core::types::StatusLevel::Warn),
                Some(GuiAction::DiagnoseProfile),
                "ignored"
            ),
            "Guardian 稳定性控制台 - 警告 - 正在执行：只读诊断 Profile"
        );
        assert_eq!(
            window_title_text(
                Some(guardian_core::types::StatusLevel::Ok),
                None,
                "刷新整机检查 -> EXIT=0（正常）"
            ),
            "Guardian 稳定性控制台 - 正常 - 刷新整机检查 -> EXIT=0（正常）"
        );
    }
}
