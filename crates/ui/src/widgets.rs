//! Reusable drawing primitives for the technical UI.
//!
//! These are deliberately small and dependency-free. Each one paints what it
//! is given; none of them decides whether a value exists — callers pass
//! `Option` and the primitive renders a dash.

use crate::theme::palette as p;
use eframe::egui;
use egui::{
    Align2, Color32, CornerRadius, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

/// Card surface used for every grouped block.
///
/// The width is reduced by the frame's own margins; without that, each card
/// would be wider than its column and the rightmost column would be clipped.
pub fn card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> Response {
    /// inner margin (10 × 2) + stroke (1 × 2)
    const OVERHEAD: f32 = 22.0;
    let width = (ui.available_width() - OVERHEAD).max(80.0);
    egui::Frame::new()
        .fill(p::PANEL)
        .stroke(Stroke::new(1.0, p::BORDER))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(width);
            add(ui);
        })
        .response
}

/// Thin uppercase label used as a section kicker.
pub fn kicker(ui: &mut Ui, text: &str) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .size(9.5)
            .extra_letter_spacing(1.1)
            .color(p::TEXT_FAINT)
            .strong(),
    );
}

/// Section heading with an optional right-aligned companion built by `right`.
pub fn section(ui: &mut Ui, title: &str, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .size(12.5)
                .strong()
                .color(p::TEXT),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            right(ui);
        });
    });
}

/// Dim caption that truncates instead of pushing the layout wider. Use this
/// for long machine strings (CPU brand, device names) placed at the right of
/// a section header.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(9.5).color(p::TEXT_FAINT)).truncate());
}

/// Small status pill. `color` sets the text and border tint.
pub fn badge(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let font = FontId::proportional(10.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, color);
    let size = Vec2::new(galley.size().x + 14.0, 17.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(4),
        color.gamma_multiply(0.14),
        Stroke::new(1.0, color.gamma_multiply(0.55)),
        StrokeKind::Inside,
    );
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

/// A filled dot, optionally with a soft halo. Used for state indicators.
pub fn dot(ui: &mut Ui, color: Color32, glow: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    let painter = ui.painter();
    if glow {
        painter.circle_filled(rect.center(), 4.5, color.gamma_multiply(0.22));
    }
    painter.circle_filled(rect.center(), 3.0, color);
    resp
}

/// Big numeric tile: mono value, small unit, label above, context below.
pub fn tile(
    ui: &mut Ui,
    label: &str,
    value: &str,
    unit: &str,
    sub: &str,
    accent: Color32,
    fraction: Option<f32>,
) {
    egui::Frame::new()
        .fill(p::PANEL)
        .stroke(Stroke::new(1.0, p::BORDER))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(egui::Margin::symmetric(11, 9))
        .show(ui, |ui| {
            /// horizontal margin (11 × 2) + stroke (1 × 2)
            const OVERHEAD: f32 = 24.0;
            ui.set_width((ui.available_width() - OVERHEAD).max(60.0));
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(label.to_uppercase())
                        .size(9.5)
                        .extra_letter_spacing(1.0)
                        .color(p::TEXT_FAINT)
                        .strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    dot(ui, accent, fraction.is_some_and(|f| f > 0.55));
                });
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 3.0;
                ui.label(
                    egui::RichText::new(value)
                        .size(22.0)
                        .monospace()
                        .color(p::TEXT),
                );
                if !unit.is_empty() {
                    ui.label(
                        egui::RichText::new(unit)
                            .size(10.5)
                            .color(p::TEXT_DIM)
                            .monospace(),
                    );
                }
            });
            if let Some(frac) = fraction {
                meter(ui, frac, accent, 3.0);
            }
            if !sub.is_empty() {
                ui.label(egui::RichText::new(sub).size(10.0).color(p::TEXT_FAINT));
            }
        });
}

/// Segmented meter. The segment count is fixed so neighbouring meters align
/// on the same grid, which reads better than smooth gradients at this size.
pub fn meter(ui: &mut Ui, fraction: f32, color: Color32, height: f32) {
    const SEGMENTS: usize = 28;
    let width = ui.available_width().max(40.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    let gap = 2.0;
    let seg_w = ((rect.width() - gap * (SEGMENTS as f32 - 1.0)) / SEGMENTS as f32).max(1.0);
    let filled = (fraction.clamp(0.0, 1.0) * SEGMENTS as f32).round() as usize;

    for i in 0..SEGMENTS {
        let x = rect.left() + i as f32 * (seg_w + gap);
        let seg = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(seg_w, rect.height()));
        let tint = if i < filled {
            color
        } else {
            p::TEXT_FAINT.gamma_multiply(0.22)
        };
        painter.rect_filled(seg, CornerRadius::same(1), tint);
    }
}

/// Horizontal bar with a value label inside, for per-core and per-disk rows.
pub fn bar_row(ui: &mut Ui, label: &str, fraction: f32, text: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [34.0, 14.0],
            egui::Label::new(
                egui::RichText::new(label)
                    .size(10.0)
                    .monospace()
                    .color(p::TEXT_DIM),
            ),
        );
        let width = (ui.available_width() - 56.0).max(50.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 12.0), Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, CornerRadius::same(2), p::BG);
        let fill = Rect::from_min_size(
            rect.min,
            Vec2::new(rect.width() * fraction.clamp(0.0, 1.0), rect.height()),
        );
        painter.rect_filled(fill, CornerRadius::same(2), color.gamma_multiply(0.85));
        ui.add_sized(
            [50.0, 14.0],
            egui::Label::new(
                egui::RichText::new(text)
                    .size(10.0)
                    .monospace()
                    .color(p::TEXT_DIM),
            ),
        );
    });
}

/// Key/value line with the value right-aligned in monospace.
pub fn kv_row(ui: &mut Ui, key: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(key).size(10.5).color(p::TEXT_FAINT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(value)
                    .size(10.5)
                    .monospace()
                    .color(p::TEXT_DIM),
            );
        });
    });
}

/// Compute polyline points for a sparkline. Higher values sit higher on
/// screen. Pure, so the geometry can be tested without a window.
pub fn spark_points(values: &[f32], lo: f32, hi: f32, rect: Rect) -> Vec<egui::Pos2> {
    let span = (hi - lo).max(1.0);
    let step = rect.width() / (values.len().saturating_sub(1)).max(1) as f32;
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = rect.left() + i as f32 * step;
            let norm = ((v.clamp(lo, hi) - lo) / span).clamp(0.0, 1.0);
            let y = rect.bottom() - norm * rect.height();
            egui::pos2(x, y)
        })
        .collect()
}

/// Sparkline with a gradient fill, baseline grid, and a right-aligned label.
/// `scale` is the maximum Y value (100 for percent, 110 for °C).
pub fn sparkline(
    ui: &mut Ui,
    values: &[f32],
    height: f32,
    scale: f32,
    color: Color32,
    label: &str,
    value_text: &str,
) {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(4), p::BG);

    // Hairline grid at 25/50/75 %.
    for q in [0.25f32, 0.5, 0.75] {
        let y = rect.bottom() - rect.height() * q;
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            Stroke::new(1.0, p::BORDER.gamma_multiply(0.55)),
        );
    }

    if values.len() < 2 {
        painter.text(
            rect.left_center() + egui::vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            format!("{label}: waiting for data"),
            FontId::proportional(10.5),
            p::TEXT_FAINT,
        );
        return;
    }

    let points = spark_points(values, 0.0, scale, rect.shrink2(egui::vec2(0.0, 2.0)));

    // Gradient fill under the curve: a few stacked translucent quads keep it
    // cheap without a shader.
    let steps = 14;
    let base = rect.bottom();
    for i in 0..points.len().saturating_sub(1) {
        let a = points[i];
        let b = points[i + 1];
        for s in 0..steps {
            let t0 = s as f32 / steps as f32;
            let t1 = (s + 1) as f32 / steps as f32;
            let ya0 = a.y + (base - a.y) * t0;
            let ya1 = a.y + (base - a.y) * t1;
            let yb0 = b.y + (base - b.y) * t0;
            let yb1 = b.y + (base - b.y) * t1;
            let alpha = (0.16 * (1.0 - t0)).max(0.0);
            let fill = color.gamma_multiply(alpha);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(a.x, ya0),
                    egui::pos2(b.x, yb0),
                    egui::pos2(b.x, yb1),
                    egui::pos2(a.x, ya1),
                ],
                fill,
                Stroke::NONE,
            ));
        }
    }

    painter.add(egui::Shape::line(points, Stroke::new(1.4, color)));

    let last = values.last().copied().unwrap_or(0.0);
    painter.text(
        rect.right_top() + egui::vec2(-8.0, 6.0),
        Align2::RIGHT_TOP,
        format!(
            "{label}  {}",
            if value_text.is_empty() {
                format!("{last:.0}%")
            } else {
                value_text.to_string()
            }
        ),
        FontId::monospace(10.0),
        p::TEXT_DIM,
    );

    resp.on_hover_text(format!(
        "{label}\nlast {last:.1}\nmax {:.1}\nmin {:.1}",
        values.iter().copied().fold(f32::MIN, f32::max),
        values.iter().copied().fold(f32::MAX, f32::min)
    ));
}

/// Draw a thin border around the current content area. Used to frame tables
/// so they read as a fixed panel rather than free-floating text.
pub fn framed(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    /// inner margin (1 × 2) + stroke (1 × 2)
    const OVERHEAD: f32 = 4.0;
    let width = (ui.available_width() - OVERHEAD).max(80.0);
    egui::Frame::new()
        .fill(p::BG)
        .stroke(Stroke::new(1.0, p::BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::same(1))
        .show(ui, |ui| {
            ui.set_width(width);
            add(ui);
        });
}

/// Empty state: a quiet explanation, never a fake number.
pub fn empty_state(ui: &mut Ui, title: &str, detail: &str) {
    ui.add_space(18.0);
    ui.vertical_centered(|ui| {
        ui.label(egui::RichText::new(title).size(12.5).color(p::TEXT_DIM));
        if !detail.is_empty() {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(detail).size(10.5).color(p::TEXT_FAINT));
        }
    });
    ui.add_space(18.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 50.0))
    }

    #[test]
    fn higher_value_is_higher_on_screen() {
        let pts = spark_points(&[10.0, 90.0], 0.0, 100.0, rect());
        assert!(pts[1].y < pts[0].y, "90% must sit above 10%");
    }

    #[test]
    fn points_span_full_width() {
        let pts = spark_points(&[1.0, 2.0, 3.0], 0.0, 100.0, rect());
        assert!((pts[0].x - 0.0).abs() < 0.01);
        assert!((pts[2].x - 100.0).abs() < 0.01);
    }

    #[test]
    fn values_outside_range_are_clamped() {
        let pts = spark_points(&[-50.0, 150.0], 0.0, 100.0, rect());
        assert!((pts[0].y - 50.0).abs() < 0.01, "negative clamps to floor");
        assert!((pts[1].y - 0.0).abs() < 0.01, ">100 clamps to ceiling");
    }

    #[test]
    fn single_value_does_not_divide_by_zero() {
        let pts = spark_points(&[42.0], 0.0, 100.0, rect());
        assert_eq!(pts.len(), 1);
        assert!(pts[0].x.is_finite() && pts[0].y.is_finite());
    }
}
