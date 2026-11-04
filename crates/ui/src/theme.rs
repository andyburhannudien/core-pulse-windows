//! Visual language for core-pulse.
//!
//! The target is a dense, technical instrument panel: near-black surfaces,
//! hairline borders, one accent colour, and tabular numbers in a monospace
//! face. Everything here is presentation only — no data decisions.

use eframe::egui;
use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, TextStyle,
};
use std::collections::BTreeMap;

/// Shared palette. Keeping colours in one place prevents the slow drift into
/// ten slightly different greys that makes utility UIs feel amateurish.
pub mod palette {
    use eframe::egui::Color32;

    pub const BG: Color32 = Color32::from_rgb(0x11, 0x13, 0x17);
    pub const PANEL: Color32 = Color32::from_rgb(0x17, 0x19, 0x1F);
    pub const ELEVATED: Color32 = Color32::from_rgb(0x1D, 0x20, 0x27);
    pub const HOVER: Color32 = Color32::from_rgb(0x25, 0x29, 0x32);
    pub const BORDER: Color32 = Color32::from_rgb(0x2B, 0x30, 0x39);
    pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3A, 0x41, 0x4D);

    pub const TEXT: Color32 = Color32::from_rgb(0xE4, 0xE8, 0xEF);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(0x9A, 0xA3, 0xB2);
    pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x6C, 0x75, 0x85);

    pub const ACCENT: Color32 = Color32::from_rgb(0x4C, 0x9E, 0xFF);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(0x2B, 0x4C, 0x74);

    pub const GREEN: Color32 = Color32::from_rgb(0x4F, 0xC3, 0x86);
    pub const AMBER: Color32 = Color32::from_rgb(0xE3, 0xA8, 0x3E);
    pub const RED: Color32 = Color32::from_rgb(0xE5, 0x60, 0x5A);
    pub const PURPLE: Color32 = Color32::from_rgb(0xA9, 0x84, 0xE8);
    pub const CYAN: Color32 = Color32::from_rgb(0x4F, 0xC5, 0xD8);
}

/// Install fonts and global style. Called once from `App::new`.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());

    ctx.all_styles_mut(|style| {
        style.text_styles = text_styles();
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(8.0, 3.0);
        style.spacing.window_margin = egui::Margin::same(10);
        style.spacing.menu_margin = egui::Margin::same(6);
        style.spacing.indent = 14.0;
        style.spacing.interact_size = egui::vec2(0.0, 20.0);
        style.spacing.icon_width = 14.0;
        style.spacing.icon_width_inner = 8.0;
        style.spacing.icon_spacing = 5.0;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating = true;
        style.visuals = visuals();
        style.visuals.window_shadow = egui::epaint::Shadow::NONE;
        style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
    });
}

fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::default();

    // The Windows UI faces are used when present; the bundled Ubuntu/Hack
    // fonts remain as fallbacks so the app still renders on other machines.
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
        defs.font_data.insert(
            "segoe".into(),
            std::sync::Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe".into());
    }
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\segoeuib.ttf") {
        defs.font_data.insert(
            "segoe-bold".into(),
            std::sync::Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, "segoe-bold".into());
    }
    if let Ok(bytes) = std::fs::read(r"C:\Windows\Fonts\consola.ttf") {
        defs.font_data.insert(
            "consolas".into(),
            std::sync::Arc::new(FontData::from_owned(bytes)),
        );
        defs.families
            .entry(FontFamily::Monospace)
            .or_default()
            .insert(0, "consolas".into());
    }
    defs
}

fn text_styles() -> BTreeMap<TextStyle, FontId> {
    let mut map = BTreeMap::new();
    map.insert(
        TextStyle::Heading,
        FontId::new(17.0, FontFamily::Proportional),
    );
    map.insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
    map.insert(
        TextStyle::Button,
        FontId::new(12.5, FontFamily::Proportional),
    );
    map.insert(
        TextStyle::Small,
        FontId::new(11.0, FontFamily::Proportional),
    );
    map.insert(
        TextStyle::Monospace,
        FontId::new(12.0, FontFamily::Monospace),
    );
    map
}

fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.dark_mode = true;
    v.panel_fill = palette::BG;
    v.window_fill = palette::PANEL;
    v.extreme_bg_color = palette::BG;
    v.faint_bg_color = palette::PANEL;
    v.code_bg_color = palette::ELEVATED;
    v.text_edit_bg_color = Some(palette::BG);
    v.override_text_color = Some(palette::TEXT);
    v.weak_text_color = Some(palette::TEXT_DIM);
    v.hyperlink_color = palette::ACCENT;
    v.warn_fg_color = palette::AMBER;
    v.error_fg_color = palette::RED;
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_stroke = Stroke::new(1.0, palette::BORDER);
    v.window_highlight_topmost = false;
    v.striped = false;
    v.button_frame = true;
    v.collapsing_header_frame = false;
    v.indent_has_left_vline = false;
    v.slider_trailing_fill = true;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    v.image_loading_spinners = false;
    v.disabled_alpha = 0.38;

    v.selection.bg_fill = palette::ACCENT_DIM;
    v.selection.stroke = Stroke::new(1.0, palette::ACCENT);

    let radius = CornerRadius::same(5);
    let border = Stroke::new(1.0, palette::BORDER);

    v.widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill: palette::PANEL,
        weak_bg_fill: palette::PANEL,
        bg_stroke: border,
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, palette::TEXT),
        expansion: 0.0,
    };
    v.widgets.inactive = egui::style::WidgetVisuals {
        bg_fill: palette::ELEVATED,
        weak_bg_fill: palette::ELEVATED,
        bg_stroke: border,
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, palette::TEXT),
        expansion: 0.0,
    };
    v.widgets.hovered = egui::style::WidgetVisuals {
        bg_fill: palette::HOVER,
        weak_bg_fill: palette::HOVER,
        bg_stroke: Stroke::new(1.0, palette::BORDER_STRONG),
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, Color32::WHITE),
        expansion: 0.0,
    };
    v.widgets.active = egui::style::WidgetVisuals {
        bg_fill: palette::ACCENT_DIM,
        weak_bg_fill: palette::ACCENT_DIM,
        bg_stroke: Stroke::new(1.0, palette::ACCENT),
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, Color32::WHITE),
        expansion: 0.0,
    };
    v.widgets.open = egui::style::WidgetVisuals {
        bg_fill: palette::PANEL,
        weak_bg_fill: palette::PANEL,
        bg_stroke: Stroke::new(1.0, palette::BORDER_STRONG),
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, palette::TEXT),
        expansion: 0.0,
    };
    v
}
