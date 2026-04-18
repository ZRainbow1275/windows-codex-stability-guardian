#![allow(dead_code)]

use guardian_core::types::StatusLevel;
use windows::{
    Win32::{
        Foundation::COLORREF,
        Graphics::Gdi::{CreateFontW, CreateSolidBrush, DeleteObject, HBRUSH, HFONT},
    },
    core::PCWSTR,
};

pub(crate) const WINDOW_WIDTH: i32 = 1280;
pub(crate) const WINDOW_HEIGHT: i32 = 860;
pub(crate) const WINDOW_MIN_WIDTH: i32 = 800;
pub(crate) const WINDOW_MIN_HEIGHT: i32 = 560;
pub(crate) const LAYOUT_BASE_WIDTH: i32 = 1120;
pub(crate) const LAYOUT_BASE_HEIGHT: i32 = 760;

pub(crate) const BG_BASE: COLORREF = rgb(0x1E, 0x1E, 0x1E);
pub(crate) const BG_SURFACE: COLORREF = rgb(0x25, 0x25, 0x26);
pub(crate) const BG_SURFACE_ALT: COLORREF = rgb(0x2D, 0x2D, 0x30);
pub(crate) const BG_INPUT: COLORREF = rgb(0x3C, 0x3C, 0x3C);
pub(crate) const BORDER_SUBTLE: COLORREF = rgb(0x3F, 0x3F, 0x46);
pub(crate) const BORDER_STRONG: COLORREF = rgb(0x55, 0x55, 0x55);
pub(crate) const TEXT_PRIMARY: COLORREF = rgb(0xE8, 0xE8, 0xE8);
pub(crate) const TEXT_SECONDARY: COLORREF = rgb(0xA0, 0xA0, 0xA0);
pub(crate) const TEXT_MUTED: COLORREF = rgb(0x70, 0x70, 0x70);
pub(crate) const TEXT_LINK: COLORREF = rgb(0x4E, 0xC9, 0xB0);
pub(crate) const STATUS_OK: COLORREF = rgb(0x4E, 0xC9, 0xB0);
pub(crate) const STATUS_WARN: COLORREF = rgb(0xE5, 0xC0, 0x7B);
pub(crate) const STATUS_ERROR: COLORREF = rgb(0xE0, 0x6C, 0x75);
pub(crate) const STATUS_INFO: COLORREF = rgb(0x56, 0x9C, 0xD6);
pub(crate) const ACCENT_PRIMARY: COLORREF = rgb(0x0E, 0x63, 0x9C);
pub(crate) const ACCENT_PRIMARY_HOVER: COLORREF = rgb(0x11, 0x77, 0xBB);
pub(crate) const ACCENT_DANGER: COLORREF = rgb(0xA1, 0x26, 0x0D);
pub(crate) const ACCENT_DANGER_HOVER: COLORREF = rgb(0xC4, 0x2B, 0x1C);

pub(crate) const FONT_DISPLAY_PT: i32 = 20;
pub(crate) const FONT_H1_PT: i32 = 16;
pub(crate) const FONT_H2_PT: i32 = 14;
pub(crate) const FONT_BODY_PT: i32 = 12;
pub(crate) const FONT_CAPTION_PT: i32 = 11;
pub(crate) const FONT_MICRO_PT: i32 = 10;
pub(crate) const FONT_MONO_PT: i32 = 11;

pub(crate) const FONT_WEIGHT_REGULAR: i32 = 400;
pub(crate) const FONT_WEIGHT_SEMIBOLD: i32 = 600;

pub(crate) const SPACING_XS: i32 = 4;
pub(crate) const SPACING_S: i32 = 8;
pub(crate) const SPACING_M: i32 = 12;
pub(crate) const SPACING_L: i32 = 16;
pub(crate) const SPACING_XL: i32 = 24;
pub(crate) const SPACING_XXL: i32 = 32;

pub(crate) const RADIUS_NONE: i32 = 0;
pub(crate) const RADIUS_SMALL: i32 = 2;
pub(crate) const RADIUS_CARD: i32 = 4;

pub(crate) const BANNER_HEIGHT: i32 = 72;
pub(crate) const STEP_NAV_HEIGHT: i32 = 44;
pub(crate) const STEP_CARD_HEIGHT: i32 = 112;
pub(crate) const DOMAIN_CARD_HEIGHT: i32 = 120;
pub(crate) const SECTION_LABEL_HEIGHT: i32 = 24;
pub(crate) const SECTION_HINT_HEIGHT: i32 = 20;
pub(crate) const PRIMARY_BUTTON_HEIGHT: i32 = 44;
pub(crate) const SECONDARY_BUTTON_HEIGHT: i32 = 36;
pub(crate) const STEP_NAV_BUTTON_HEIGHT: i32 = 32;
pub(crate) const COLLAPSIBLE_HEIGHT: i32 = 40;
pub(crate) const DETAIL_TAB_HEIGHT: i32 = 32;
pub(crate) const DETAIL_BODY_HEIGHT: i32 = 240;
pub(crate) const CONTENT_WIDTH_MIN: i32 = 760;
pub(crate) const CONTENT_WIDTH_MAX: i32 = 2400;
pub(crate) const REVIEW_STACK_BREAKPOINT: i32 = 680;
pub(crate) const STEP_NAV_MIN_WIDTH: i32 = 132;
pub(crate) const ACTION_BUTTON_MIN_WIDTH: i32 = 220;
pub(crate) const EDIT_PADDING_X: i32 = 12;
pub(crate) const EDIT_PADDING_Y: i32 = 10;

pub(crate) struct GuiTheme {
    scale: f32,
    pub display_font: HFONT,
    pub h1_font: HFONT,
    pub h2_font: HFONT,
    pub body_font: HFONT,
    pub caption_font: HFONT,
    pub micro_font: HFONT,
    pub mono_font: HFONT,
    pub base_brush: HBRUSH,
    pub surface_brush: HBRUSH,
    pub surface_alt_brush: HBRUSH,
    pub input_brush: HBRUSH,
    pub border_subtle_brush: HBRUSH,
    pub border_strong_brush: HBRUSH,
    pub ok_brush: HBRUSH,
    pub warn_brush: HBRUSH,
    pub error_brush: HBRUSH,
    pub info_brush: HBRUSH,
}

impl GuiTheme {
    pub(crate) fn new() -> Self {
        Self {
            scale: 1.0,
            display_font: create_theme_font(
                FONT_DISPLAY_PT,
                FONT_WEIGHT_SEMIBOLD,
                1.0,
                "Microsoft YaHei UI",
            ),
            h1_font: create_theme_font(FONT_H1_PT, FONT_WEIGHT_SEMIBOLD, 1.0, "Microsoft YaHei UI"),
            h2_font: create_theme_font(FONT_H2_PT, FONT_WEIGHT_SEMIBOLD, 1.0, "Microsoft YaHei UI"),
            body_font: create_theme_font(
                FONT_BODY_PT,
                FONT_WEIGHT_REGULAR,
                1.0,
                "Microsoft YaHei UI",
            ),
            caption_font: create_theme_font(
                FONT_CAPTION_PT,
                FONT_WEIGHT_REGULAR,
                1.0,
                "Microsoft YaHei UI",
            ),
            micro_font: create_theme_font(
                FONT_MICRO_PT,
                FONT_WEIGHT_REGULAR,
                1.0,
                "Microsoft YaHei UI",
            ),
            mono_font: create_theme_font(FONT_MONO_PT, FONT_WEIGHT_REGULAR, 1.0, "Cascadia Code"),
            base_brush: create_solid_brush(BG_BASE),
            surface_brush: create_solid_brush(BG_SURFACE),
            surface_alt_brush: create_solid_brush(BG_SURFACE_ALT),
            input_brush: create_solid_brush(BG_INPUT),
            border_subtle_brush: create_solid_brush(BORDER_SUBTLE),
            border_strong_brush: create_solid_brush(BORDER_STRONG),
            ok_brush: create_solid_brush(STATUS_OK),
            warn_brush: create_solid_brush(STATUS_WARN),
            error_brush: create_solid_brush(STATUS_ERROR),
            info_brush: create_solid_brush(STATUS_INFO),
        }
    }

    pub(crate) fn scale(&self) -> f32 {
        self.scale
    }

    pub(crate) fn update_scale(&mut self, scale: f32) -> bool {
        let next = scale.clamp(1.0, 1.5);
        if (self.scale - next).abs() < 0.04 {
            return false;
        }

        self.delete_fonts();
        self.scale = next;
        self.display_font = create_theme_font(
            FONT_DISPLAY_PT,
            FONT_WEIGHT_SEMIBOLD,
            next,
            "Microsoft YaHei UI",
        );
        self.h1_font =
            create_theme_font(FONT_H1_PT, FONT_WEIGHT_SEMIBOLD, next, "Microsoft YaHei UI");
        self.h2_font =
            create_theme_font(FONT_H2_PT, FONT_WEIGHT_SEMIBOLD, next, "Microsoft YaHei UI");
        self.body_font = create_theme_font(
            FONT_BODY_PT,
            FONT_WEIGHT_REGULAR,
            next,
            "Microsoft YaHei UI",
        );
        self.caption_font = create_theme_font(
            FONT_CAPTION_PT,
            FONT_WEIGHT_REGULAR,
            next,
            "Microsoft YaHei UI",
        );
        self.micro_font = create_theme_font(
            FONT_MICRO_PT,
            FONT_WEIGHT_REGULAR,
            next,
            "Microsoft YaHei UI",
        );
        self.mono_font =
            create_theme_font(FONT_MONO_PT, FONT_WEIGHT_REGULAR, next, "Cascadia Code");
        true
    }

    fn delete_fonts(&mut self) {
        unsafe {
            let _ = DeleteObject(self.display_font);
            let _ = DeleteObject(self.h1_font);
            let _ = DeleteObject(self.h2_font);
            let _ = DeleteObject(self.body_font);
            let _ = DeleteObject(self.caption_font);
            let _ = DeleteObject(self.micro_font);
            let _ = DeleteObject(self.mono_font);
        }
    }

    fn delete_brushes(&mut self) {
        unsafe {
            let _ = DeleteObject(self.base_brush);
            let _ = DeleteObject(self.surface_brush);
            let _ = DeleteObject(self.surface_alt_brush);
            let _ = DeleteObject(self.input_brush);
            let _ = DeleteObject(self.border_subtle_brush);
            let _ = DeleteObject(self.border_strong_brush);
            let _ = DeleteObject(self.ok_brush);
            let _ = DeleteObject(self.warn_brush);
            let _ = DeleteObject(self.error_brush);
            let _ = DeleteObject(self.info_brush);
        }
    }
}

impl Drop for GuiTheme {
    fn drop(&mut self) {
        self.delete_fonts();
        self.delete_brushes();
    }
}

pub(crate) fn status_line_palette(
    theme: &GuiTheme,
    status: Option<StatusLevel>,
) -> (COLORREF, HBRUSH) {
    match status {
        Some(StatusLevel::Ok) => (STATUS_OK, theme.ok_brush),
        Some(StatusLevel::Warn) => (STATUS_WARN, theme.warn_brush),
        Some(StatusLevel::Fail) => (STATUS_ERROR, theme.error_brush),
        None => (BORDER_STRONG, theme.border_strong_brush),
    }
}

pub(crate) const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF((red as u32) | ((green as u32) << 8) | ((blue as u32) << 16))
}

pub(crate) const fn font_height(points: i32) -> i32 {
    -((points * 96 + 36) / 72)
}

pub(crate) fn scaled(value: i32, scale: f32) -> i32 {
    ((value as f32 * scale).round() as i32).max(1)
}

pub(crate) fn create_solid_brush(color: COLORREF) -> HBRUSH {
    unsafe { CreateSolidBrush(color) }
}

fn create_theme_font(points: i32, weight: i32, scale: f32, face_name: &str) -> HFONT {
    let scaled_points = ((points as f32) * scale).round() as i32;
    create_font(font_height(scaled_points.max(1)), weight, face_name)
}

pub(crate) fn create_font(height: i32, weight: i32, face_name: &str) -> HFONT {
    let wide_face = to_wide(face_name);
    unsafe {
        CreateFontW(
            height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            PCWSTR(wide_face.as_ptr()),
        )
    }
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
