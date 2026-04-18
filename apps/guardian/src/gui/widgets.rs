use guardian_core::types::StatusLevel;
use windows::Win32::{
    Foundation::{COLORREF, RECT},
    Graphics::Gdi::{
        DC_BRUSH, DC_PEN, DRAW_TEXT_FORMAT, DT_CENTER, DT_EDITCONTROL, DT_LEFT, DT_NOPREFIX,
        DT_RIGHT, DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, DrawTextW, GetStockObject, HDC,
        RoundRect, SelectObject, SetBkMode, SetDCBrushColor, SetDCPenColor, SetTextColor,
        TRANSPARENT,
    },
};

use super::theme::{
    ACCENT_DANGER, ACCENT_DANGER_HOVER, ACCENT_PRIMARY, ACCENT_PRIMARY_HOVER, BG_BASE, BG_INPUT,
    BG_SURFACE, BG_SURFACE_ALT, BORDER_STRONG, BORDER_SUBTLE, GuiTheme, RADIUS_CARD, RADIUS_NONE,
    RADIUS_SMALL, SPACING_L, SPACING_M, SPACING_S, SPACING_XS, STATUS_ERROR, STATUS_INFO,
    STATUS_OK, STATUS_WARN, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY, scaled, status_line_palette,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonKind {
    Primary,
    Secondary,
    Danger,
    Ghost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonVisualState {
    Normal,
    Pressed,
    Disabled,
    Active,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepVisualState {
    Current,
    Ready,
    Locked,
}

pub(crate) fn paint_banner(
    hdc: HDC,
    rect: RECT,
    theme: &GuiTheme,
    status: Option<StatusLevel>,
    title: &str,
    subtitle: &str,
    timestamp: &str,
) {
    let scale = theme.scale();
    let spacing_xs = scaled(SPACING_XS, scale);
    let spacing_s = scaled(SPACING_S, scale);
    let spacing_m = scaled(SPACING_M, scale);
    let spacing_l = scaled(SPACING_L, scale);
    let (status_color, _) = status_line_palette(theme, status);
    fill_round_rect(hdc, rect, BG_SURFACE, BORDER_SUBTLE, RADIUS_NONE);
    let banner_width = (rect.right - rect.left).max(0);
    let timestamp_width = (banner_width / 4).clamp(scaled(132, scale), scaled(220, scale));
    let content_left = rect.left + spacing_l + spacing_m;
    let content_top = rect.top + spacing_s;

    let stripe = RECT {
        left: rect.left,
        top: rect.top,
        right: rect.left + spacing_xs,
        bottom: rect.bottom,
    };
    fill_round_rect(hdc, stripe, status_color, status_color, RADIUS_NONE);

    let mut title_rect = RECT {
        left: content_left,
        top: content_top,
        right: rect.right - timestamp_width - spacing_l - spacing_s,
        bottom: content_top + scaled(38, scale),
    };
    let mut subtitle_rect = RECT {
        left: title_rect.left,
        top: title_rect.bottom - spacing_xs,
        right: rect.right - spacing_l,
        bottom: rect.bottom - spacing_s,
    };
    let mut timestamp_rect = RECT {
        left: rect.right - timestamp_width - spacing_l,
        top: content_top + spacing_xs,
        right: rect.right - spacing_l,
        bottom: content_top + scaled(28, scale),
    };

    draw_text(
        hdc,
        theme.display_font,
        TEXT_PRIMARY,
        title,
        &mut title_rect,
        DT_LEFT,
    );
    draw_text(
        hdc,
        theme.caption_font,
        TEXT_SECONDARY,
        subtitle,
        &mut subtitle_rect,
        DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL,
    );
    draw_text(
        hdc,
        theme.caption_font,
        TEXT_MUTED,
        timestamp,
        &mut timestamp_rect,
        DT_RIGHT | DT_SINGLELINE | DT_VCENTER,
    );
}

pub(crate) fn paint_step_nav(
    hdc: HDC,
    rect: RECT,
    theme: &GuiTheme,
    label: &str,
    state: StepVisualState,
    pressed: bool,
) {
    let scale = theme.scale();
    let spacing_xs = scaled(SPACING_XS, scale);
    let spacing_s = scaled(SPACING_S, scale);
    let spacing_m = scaled(SPACING_M, scale);
    let (fill, text, border) = match state {
        StepVisualState::Current => (BG_SURFACE_ALT, TEXT_PRIMARY, ACCENT_PRIMARY),
        StepVisualState::Ready => (BG_BASE, TEXT_SECONDARY, BORDER_SUBTLE),
        StepVisualState::Locked => (BG_BASE, TEXT_MUTED, BORDER_SUBTLE),
    };
    let fill = if pressed { BG_SURFACE_ALT } else { fill };

    fill_round_rect(hdc, rect, fill, border, RADIUS_NONE);

    if state == StepVisualState::Current {
        let underline = RECT {
            left: rect.left,
            top: rect.bottom - spacing_xs,
            right: rect.right,
            bottom: rect.bottom,
        };
        fill_round_rect(hdc, underline, ACCENT_PRIMARY, ACCENT_PRIMARY, RADIUS_NONE);
    }

    let mut text_rect = inset_rect(rect, spacing_m, spacing_s);
    draw_text(
        hdc,
        theme.caption_font,
        text,
        label,
        &mut text_rect,
        DT_CENTER | DT_VCENTER | DT_WORDBREAK | DT_EDITCONTROL,
    );
}

pub(crate) fn paint_card(
    hdc: HDC,
    rect: RECT,
    theme: &GuiTheme,
    header: &str,
    summary: &str,
    status: Option<StatusLevel>,
    active: bool,
) {
    let scale = theme.scale();
    let spacing_s = scaled(SPACING_S, scale);
    let spacing_m = scaled(SPACING_M, scale);
    let fill = if active { BG_SURFACE_ALT } else { BG_SURFACE };
    let border = if active { BORDER_STRONG } else { BORDER_SUBTLE };
    fill_round_rect(hdc, rect, fill, border, RADIUS_CARD);

    let badge_rect = RECT {
        left: rect.right - scaled(112, scale),
        top: rect.top + spacing_m,
        right: rect.right - spacing_m,
        bottom: rect.top + spacing_m + scaled(28, scale),
    };
    paint_badge(hdc, badge_rect, theme, status_label(status), status);

    let mut header_rect = RECT {
        left: rect.left + spacing_m,
        top: rect.top + spacing_m,
        right: badge_rect.left - spacing_s,
        bottom: rect.top + spacing_m + scaled(34, scale),
    };
    let mut body_rect = RECT {
        left: rect.left + spacing_m,
        top: header_rect.bottom + spacing_s,
        right: rect.right - spacing_m,
        bottom: rect.bottom - spacing_m,
    };

    draw_text(
        hdc,
        theme.h2_font,
        TEXT_PRIMARY,
        header,
        &mut header_rect,
        DT_LEFT,
    );
    draw_text(
        hdc,
        theme.body_font,
        TEXT_SECONDARY,
        summary,
        &mut body_rect,
        DT_LEFT | DT_WORDBREAK | DT_EDITCONTROL,
    );
}

pub(crate) fn paint_button(
    hdc: HDC,
    rect: RECT,
    theme: &GuiTheme,
    label: &str,
    kind: ButtonKind,
    state: ButtonVisualState,
) {
    let (fill, border, text) = button_palette(kind, state);
    fill_round_rect(hdc, rect, fill, border, RADIUS_SMALL);

    let scale = theme.scale();
    let mut text_rect = inset_rect(rect, scaled(SPACING_M, scale), scaled(SPACING_XS, scale));
    draw_text(
        hdc,
        theme.body_font,
        text,
        label,
        &mut text_rect,
        DT_CENTER | DT_VCENTER | DT_WORDBREAK | DT_EDITCONTROL | DT_NOPREFIX,
    );
}

pub(crate) fn paint_badge(
    hdc: HDC,
    rect: RECT,
    theme: &GuiTheme,
    label: &str,
    status: Option<StatusLevel>,
) {
    let scale = theme.scale();
    let badge_fill = match status {
        Some(StatusLevel::Ok) => STATUS_OK,
        Some(StatusLevel::Warn) => STATUS_WARN,
        Some(StatusLevel::Fail) => STATUS_ERROR,
        None => STATUS_INFO,
    };
    fill_round_rect(hdc, rect, BG_BASE, badge_fill, RADIUS_SMALL);

    let mut text_rect = inset_rect(rect, scaled(SPACING_S, scale), scaled(SPACING_XS, scale));
    let label = format!("● {label}");
    draw_text(
        hdc,
        theme.micro_font,
        badge_fill,
        &label,
        &mut text_rect,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
    );
}

pub(crate) fn paint_collapsible(
    hdc: HDC,
    rect: RECT,
    theme: &GuiTheme,
    title: &str,
    expanded: bool,
    pressed: bool,
) {
    let scale = theme.scale();
    let spacing_m = scaled(SPACING_M, scale);
    let fill = if pressed { BG_SURFACE_ALT } else { BG_SURFACE };
    fill_round_rect(hdc, rect, fill, BORDER_SUBTLE, RADIUS_CARD);

    let arrow = if expanded { "▾" } else { "▸" };
    let mut title_rect = RECT {
        left: rect.left + spacing_m,
        top: rect.top,
        right: rect.right - spacing_m,
        bottom: rect.bottom,
    };
    let label = format!("{arrow} {title}");
    draw_text(
        hdc,
        theme.h2_font,
        TEXT_PRIMARY,
        &label,
        &mut title_rect,
        DT_LEFT | DT_VCENTER | DT_SINGLELINE,
    );
}

fn button_palette(kind: ButtonKind, state: ButtonVisualState) -> (COLORREF, COLORREF, COLORREF) {
    match state {
        ButtonVisualState::Disabled => (BG_INPUT, BORDER_SUBTLE, TEXT_MUTED),
        ButtonVisualState::Pressed => match kind {
            ButtonKind::Primary => (ACCENT_PRIMARY_HOVER, ACCENT_PRIMARY_HOVER, TEXT_PRIMARY),
            ButtonKind::Danger => (ACCENT_DANGER_HOVER, ACCENT_DANGER_HOVER, TEXT_PRIMARY),
            ButtonKind::Ghost => (BG_SURFACE_ALT, BORDER_STRONG, TEXT_PRIMARY),
            ButtonKind::Secondary => (BG_SURFACE_ALT, BORDER_STRONG, TEXT_PRIMARY),
        },
        ButtonVisualState::Active => match kind {
            ButtonKind::Primary => (ACCENT_PRIMARY_HOVER, ACCENT_PRIMARY_HOVER, TEXT_PRIMARY),
            ButtonKind::Danger => (ACCENT_DANGER_HOVER, ACCENT_DANGER_HOVER, TEXT_PRIMARY),
            ButtonKind::Ghost => (BG_SURFACE_ALT, BORDER_STRONG, TEXT_PRIMARY),
            ButtonKind::Secondary => (BG_SURFACE_ALT, BORDER_STRONG, TEXT_PRIMARY),
        },
        ButtonVisualState::Normal => match kind {
            ButtonKind::Primary => (ACCENT_PRIMARY, ACCENT_PRIMARY, TEXT_PRIMARY),
            ButtonKind::Danger => (ACCENT_DANGER, ACCENT_DANGER, TEXT_PRIMARY),
            ButtonKind::Ghost => (BG_BASE, BORDER_SUBTLE, TEXT_SECONDARY),
            ButtonKind::Secondary => (BG_SURFACE, BORDER_SUBTLE, TEXT_PRIMARY),
        },
    }
}

fn status_label(status: Option<StatusLevel>) -> &'static str {
    match status {
        Some(StatusLevel::Ok) => "正常",
        Some(StatusLevel::Warn) => "警告",
        Some(StatusLevel::Fail) => "失败",
        None => "待处理",
    }
}

fn fill_round_rect(hdc: HDC, rect: RECT, fill: COLORREF, border: COLORREF, radius: i32) {
    unsafe {
        let old_brush = SelectObject(hdc, GetStockObject(DC_BRUSH));
        let old_pen = SelectObject(hdc, GetStockObject(DC_PEN));
        let _ = SetDCBrushColor(hdc, fill);
        let _ = SetDCPenColor(hdc, border);
        let _ = RoundRect(
            hdc,
            rect.left,
            rect.top,
            rect.right,
            rect.bottom,
            radius.max(1),
            radius.max(1),
        );
        let _ = SelectObject(hdc, old_brush);
        let _ = SelectObject(hdc, old_pen);
    }
}

fn draw_text(
    hdc: HDC,
    font: windows::Win32::Graphics::Gdi::HFONT,
    color: COLORREF,
    text: &str,
    rect: &mut RECT,
    flags: DRAW_TEXT_FORMAT,
) {
    let mut wide = text.encode_utf16().collect::<Vec<_>>();
    unsafe {
        let old_font = SelectObject(hdc, font);
        let _ = SetBkMode(hdc, TRANSPARENT);
        let _ = SetTextColor(hdc, color);
        let _ = DrawTextW(hdc, &mut wide, rect, flags);
        let _ = SelectObject(hdc, old_font);
    }
}

fn inset_rect(rect: RECT, horizontal: i32, vertical: i32) -> RECT {
    RECT {
        left: rect.left + horizontal,
        top: rect.top + vertical,
        right: rect.right - horizontal,
        bottom: rect.bottom - vertical,
    }
}
