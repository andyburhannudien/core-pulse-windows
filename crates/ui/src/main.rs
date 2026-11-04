//! UI core-pulse: tabel proses dan tab kinerja.

#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod format;
mod state;

use core_pulse_core::{Collector, Snapshot};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use state::{AppState, SortKey, Tab};
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    // `--tab performance` membuka langsung di tab Kinerja (berguna untuk
    // screenshot dan pengujian manual).
    let start_tab = match std::env::args().nth(1).as_deref() {
        Some("performance") | Some("--performance") => state::Tab::Performance,
        _ => state::Tab::Processes,
    };

    let store = Arc::new(core_pulse_core::sampler::SnapshotStore::new());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("core-pulse")
            .with_visible(true),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "core-pulse",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, store, start_tab)))),
    )
    .map_err(|e| {
        eprintln!("gagal menjalankan UI: {e}");
        e
    })
}

/// Satu thread sampler, satu penulis ke `SnapshotStore`.
///
/// Repaint dipicu dari sini (1 Hz), bukan dari timer di render loop: saat
/// window tidak berinteraksi, kita benar-benar tidur. Ini yang menjaga CPU idle
/// di bawah 1% (lihat docs/PERF.md).
fn spawn_sampler(store: Arc<core_pulse_core::sampler::SnapshotStore>, ctx: egui::Context) {
    std::thread::Builder::new()
        .name("sampler".into())
        .spawn(move || {
            let mut collector = Collector::new();
            let mut ticker =
                core_pulse_core::sampler::Ticker::new(core_pulse_core::sampler::SAMPLE_INTERVAL);
            loop {
                let elapsed = ticker.tick();
                let mut snap = collector.sample();
                // Setelah sleep/resume delta bisa sangat besar; buang sampel itu.
                if core_pulse_core::sampler::is_time_jump(
                    elapsed,
                    core_pulse_core::sampler::SAMPLE_INTERVAL,
                ) {
                    snap.cpu.total_percent = 0.0;
                }
                store.store(snap);
                ctx.request_repaint();
            }
        })
        .expect("gagal membuat thread sampler");
}

struct App {
    state: AppState,
    store: Arc<core_pulse_core::sampler::SnapshotStore>,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        store: Arc<core_pulse_core::sampler::SnapshotStore>,
        start_tab: Tab,
    ) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        spawn_sampler(Arc::clone(&store), cc.egui_ctx.clone());
        Self {
            state: AppState {
                tab: start_tab,
                ..Default::default()
            },
            store,
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.store.load();
        let snap_arc: Arc<Snapshot> = Arc::clone(&snap);
        self.state.update_snapshot(&snap_arc);

        egui::Panel::top("nav").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.state.tab, Tab::Processes, "Proses");
                ui.selectable_value(&mut self.state.tab, Tab::Performance, "Kinerja");
                ui.separator();
                if self.state.tab == Tab::Processes {
                    ui.label("Cari:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.search)
                            .hint_text("nama atau PID")
                            .desired_width(220.0),
                    );
                    ui.checkbox(&mut self.state.show_system, "proses sistem");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} proses",
                            self.state.snapshot.processes.len()
                        ))
                        .weak(),
                    );
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let s = &self.state.snapshot;
                ui.label(format!("CPU {}", format::percent(s.cpu.total_percent)));
                ui.separator();
                let mem_pct = if s.memory.total_bytes > 0 {
                    s.memory.in_use_bytes as f32 / s.memory.total_bytes as f32 * 100.0
                } else {
                    0.0
                };
                ui.label(format!(
                    "RAM {} / {} ({})",
                    format::bytes(s.memory.in_use_bytes),
                    format::bytes(s.memory.total_bytes),
                    format::percent(mem_pct)
                ));
                ui.separator();
                ui.label(format!("uptime {}", format::uptime(s.host.uptime_secs)));
                ui.separator();
                // Suhu: jujur kalau tidak ada sumber (ADR-0008), bukan "0 °C".
                let temp = s.gpus.iter().find_map(|g| g.temperature_c);
                match temp {
                    Some(t) => {
                        ui.label(format!("Suhu {}", format::temp_c(Some(t))));
                    }
                    None => {
                        ui.label(
                            egui::RichText::new("Suhu tidak tersedia")
                                .color(egui::Color32::from_rgb(0x9A, 0x9A, 0xA2)),
                        )
                        .on_hover_text(
                            "Belum ada sumber sensor suhu yang aktif.\n\
                             Thermal zone ACPI tidak diekspos firmware; sensor disk (SMART) \
                             dan GPU menyusul di P3.",
                        );
                    }
                }
                if !s.degraded.is_empty() {
                    ui.separator();
                    let n = s.degraded.len();
                    ui.label(
                        egui::RichText::new(format!("{n} sumber terbatas"))
                            .color(egui::Color32::from_rgb(0xE0, 0xA0, 0x30)),
                    )
                    .on_hover_text(
                        s.degraded
                            .iter()
                            .map(|d| format!("{:?}/{:?}: {}", d.area, d.reason, d.detail))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} · build {}",
                            s.host.os_name, s.host.os_build
                        ))
                        .weak(),
                    );
                });
            });
        });

        egui::CentralPanel::default().show(ui, |ui| match self.state.tab {
            Tab::Processes => self.processes(ui),
            Tab::Performance => {
                // Konten Kinerja lebih tinggi dari window minimum; biarkan
                // scroll supaya tidak ada kartu yang terpotong di 560 px.
                egui::ScrollArea::vertical().show(ui, |ui| self.performance(ui));
            }
        });
    }
}

impl App {
    fn processes(&mut self, ui: &mut egui::Ui) {
        let rows = self.state.visible_rows();
        let sort = self.state.sort;
        let sort_asc = self.state.sort_asc;
        let mut clicked: Option<SortKey> = None;
        let mut selected = self.state.selected;

        let arrow = |key: SortKey| -> &'static str {
            if sort == key {
                if sort_asc { " ▲" } else { " ▼" }
            } else {
                ""
            }
        };

        let avail = ui.available_height();
        TableBuilder::new(ui)
            .id_salt("process_table")
            .striped(true)
            .resizable(false)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(240.0).clip(true))
            .column(Column::exact(70.0))
            .column(Column::exact(80.0))
            .column(Column::exact(100.0))
            .column(Column::exact(70.0))
            .column(Column::exact(70.0))
            .column(Column::exact(80.0))
            .min_scrolled_height(0.0)
            .max_scroll_height(avail)
            .header(22.0, |mut header| {
                header.col(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!("Nama{}", arrow(SortKey::Name)))
                                    .strong(),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        clicked = Some(SortKey::Name);
                    }
                });
                header.col(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!("PID{}", arrow(SortKey::Pid))).strong(),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        clicked = Some(SortKey::Pid);
                    }
                });
                header.col(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!("CPU{}", arrow(SortKey::Cpu))).strong(),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        clicked = Some(SortKey::Cpu);
                    }
                });
                header.col(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!("Memori{}", arrow(SortKey::Memory)))
                                    .strong(),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        clicked = Some(SortKey::Memory);
                    }
                });
                header.col(|ui| {
                    ui.strong("GPU");
                });
                header.col(|ui| {
                    ui.strong("Thread");
                });
                header.col(|ui| {
                    ui.strong("Handle");
                });
            })
            .body(|body| {
                body.rows(20.0, rows.len(), |mut row| {
                    let i = row.index();
                    let p = rows[i];
                    row.set_selected(selected == Some(p.pid));

                    row.col(|ui| {
                        let color = if p.elevated == Some(true) {
                            egui::Color32::from_rgb(0xE0, 0xA0, 0x30)
                        } else {
                            ui.visuals().text_color()
                        };
                        ui.label(egui::RichText::new(format::truncate(&p.name, 60)).color(color));
                    });
                    row.col(|ui| {
                        ui.monospace(p.pid.to_string());
                    });
                    row.col(|ui| {
                        ui.monospace(format::percent(p.cpu_percent));
                    });
                    row.col(|ui| {
                        ui.monospace(format::bytes(p.mem_private_bytes));
                    });
                    row.col(|ui| {
                        // `None` = tidak terukur (kolom GPU mati), bukan 0%.
                        ui.monospace(
                            p.gpu_percent
                                .map(|g| format!("{g:.1}%"))
                                .unwrap_or_else(|| "—".into()),
                        );
                    });
                    row.col(|ui| {
                        ui.monospace(p.threads.to_string());
                    });
                    row.col(|ui| {
                        ui.monospace(p.handles.to_string());
                    });

                    if row.response().clicked() {
                        selected = Some(p.pid);
                    }
                });
            });

        if let Some(key) = clicked {
            self.state.set_sort(key);
        }
        self.state.selected = selected;
    }

    fn performance(&mut self, ui: &mut egui::Ui) {
        let s = Arc::clone(&self.state.snapshot);
        ui.horizontal(|ui| {
            ui.heading("Kinerja");
            ui.label(
                egui::RichText::new(format!("diperbarui tiap {} ms", s.interval_ms.max(1)))
                    .weak()
                    .size(11.0),
            );
        });
        ui.add_space(6.0);

        // Dua kolom: CPU kiri, Memori kanan. `ui.columns` memaksa layout
        // vertikal di dalam setiap kolom, jadi isi kartu tidak meluber.
        ui.columns(2, |cols| {
            egui::Frame::group(cols[0].style()).show(&mut cols[0], |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("CPU").strong());
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", s.cpu.total_percent))
                                .size(24.0)
                                .strong(),
                        );
                    });
                    ui.label(egui::RichText::new(&s.host.cpu_brand).weak().size(11.0));
                    ui.add_space(6.0);
                    let bar_w = (ui.available_width() - 34.0).max(80.0);
                    for (i, v) in s.cpu.per_core_percent.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format!("C{i}")).weak().monospace());
                            ui.add(
                                egui::ProgressBar::new(v / 100.0)
                                    .desired_width(bar_w)
                                    .text(format!("{v:.0}%")),
                            );
                        });
                    }
                });
            });

            egui::Frame::group(cols[1].style()).show(&mut cols[1], |ui| {
                ui.vertical(|ui| {
                    let pct = if s.memory.total_bytes > 0 {
                        s.memory.in_use_bytes as f32 / s.memory.total_bytes as f32 * 100.0
                    } else {
                        0.0
                    };
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("Memori").strong());
                        ui.label(
                            egui::RichText::new(format!("{pct:.0}%"))
                                .size(24.0)
                                .strong(),
                        );
                    });
                    ui.add(
                        egui::ProgressBar::new(pct / 100.0)
                            .desired_width(ui.available_width().max(80.0)),
                    );
                    ui.add_space(6.0);
                    let rows = [
                        ("Dipakai", s.memory.in_use_bytes),
                        ("Standby", s.memory.standby_bytes),
                        ("Modified", s.memory.modified_bytes),
                        ("Gratis", s.memory.free_bytes),
                        ("Commit", s.memory.commit_total_bytes),
                    ];
                    for (label, value) in rows {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(label).weak());
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.monospace(format::bytes(value));
                                },
                            );
                        });
                    }
                });
            });
        });

        ui.add_space(8.0);
        ui.columns(2, |cols| {
            // Kartu Disk
            egui::Frame::group(cols[0].style()).show(&mut cols[0], |ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Disk").strong());
                    if s.disks.is_empty() {
                        unavailable_note(ui, "counter PhysicalDisk tidak tersedia");
                        return;
                    }
                    for d in &s.disks {
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(format::truncate(&d.model, 26)).strong());
                            ui.label(
                                egui::RichText::new(format!("{:?}", d.media))
                                    .weak()
                                    .size(10.0),
                            );
                        });
                        let bar_w = (ui.available_width() - 8.0).max(80.0);
                        ui.add(
                            egui::ProgressBar::new(d.busy_percent / 100.0)
                                .desired_width(bar_w)
                                .text(format!("{:.0}% aktif", d.busy_percent)),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "R {}  W {}  antrean {:.2}  {:.1} ms",
                                format::bytes(d.read_bps),
                                format::bytes(d.write_bps),
                                d.queue_depth,
                                d.avg_response_ms
                            ))
                            .weak()
                            .size(11.0),
                        );
                    }
                });
            });

            // Kartu GPU
            egui::Frame::group(cols[1].style()).show(&mut cols[1], |ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("GPU").strong());
                    if s.gpus.is_empty() {
                        unavailable_note(ui, "counter GPU Engine tidak tersedia");
                        return;
                    }
                    for g in &s.gpus {
                        ui.add_space(2.0);
                        ui.label(egui::RichText::new(format::truncate(&g.name, 34)).strong());
                        let bar_w = (ui.available_width() - 8.0).max(80.0);
                        ui.add(
                            egui::ProgressBar::new(g.utilization_percent.unwrap_or(0.0) / 100.0)
                                .desired_width(bar_w)
                                .text(format!("{:.0}%", g.utilization_percent.unwrap_or(0.0))),
                        );
                        let mem = match (g.dedicated_used_bytes, g.shared_used_bytes) {
                            (Some(d), Some(sh)) if d > 0 => format!(
                                "khusus {}  bersama {}",
                                format::bytes(d),
                                format::bytes(sh)
                            ),
                            (_, Some(sh)) => format!("bersama {}", format::bytes(sh)),
                            (Some(d), None) => format!("khusus {}", format::bytes(d)),
                            _ => "memori tidak terukur".to_string(),
                        };
                        ui.label(egui::RichText::new(mem).weak().size(11.0));
                        ui.label(
                            egui::RichText::new(format!(
                                "suhu {}",
                                format::temp_c(g.temperature_c)
                            ))
                            .weak()
                            .size(11.0),
                        );
                    }
                });
            });
        });

        ui.add_space(8.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Jaringan").strong());
                if s.net.is_empty() {
                    unavailable_note(ui, "tidak ada adapter non-loopback");
                    return;
                }
                for n in &s.net {
                    ui.horizontal(|ui| {
                        let dot = if n.is_up { "●" } else { "○" };
                        let color = if n.is_up {
                            egui::Color32::from_rgb(0x4A, 0xC0, 0x7A)
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        ui.label(egui::RichText::new(dot).color(color));
                        ui.label(format::truncate(&n.name, 22));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.monospace(format!(
                                "↓ {}  ↑ {}",
                                format::bytes(n.rx_bps),
                                format::bytes(n.tx_bps)
                            ));
                        });
                    });
                }
            });
        });

        ui.add_space(8.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("Riwayat 60 detik").strong());
                let hist = &self.state.history;
                let w = ui.available_width();
                let rows: [(&[f32], egui::Color32, &str); 4] = [
                    (&hist.cpu, egui::Color32::from_rgb(0x4A, 0x9E, 0xFF), "CPU"),
                    (&hist.mem, egui::Color32::from_rgb(0x4A, 0xC0, 0x7A), "RAM"),
                    (&hist.gpu, egui::Color32::from_rgb(0xB0, 0x7A, 0xE0), "GPU"),
                    (
                        &hist.disk,
                        egui::Color32::from_rgb(0xE0, 0xA0, 0x30),
                        "Disk",
                    ),
                ];
                for (i, (values, color, label)) in rows.into_iter().enumerate() {
                    if i > 0 {
                        ui.add_space(4.0);
                    }
                    draw_sparkline(ui, values, w, 0.0..=100.0, color, label);
                }
            });
        });

        if !s.degraded.is_empty() {
            ui.add_space(8.0);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new("Sumber data terbatas").strong());
                    for d in &s.degraded {
                        ui.label(format!("• {:?} ({:?}): {}", d.area, d.reason, d.detail));
                    }
                });
            });
        }
    }
}

/// Catatan kecil untuk bagian yang tidak punya sumber data.
fn unavailable_note(ui: &mut egui::Ui, reason: &str) {
    ui.label(egui::RichText::new(format!("— {reason}")).weak().size(11.0));
}

/// Hitung titik polyline untuk sparkline. Nilai lebih besar = y lebih kecil
/// (lebih tinggi di layar). Dipisah dari rendering supaya bisa diuji.
fn spark_points(values: &[f32], lo: f32, hi: f32, rect: egui::Rect) -> Vec<egui::Pos2> {
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

/// Sparkline sederhana tanpa dependensi tambahan.
fn draw_sparkline(
    ui: &mut egui::Ui,
    values: &[f32],
    width: f32,
    range: std::ops::RangeInclusive<f32>,
    color: egui::Color32,
    label: &str,
) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(width.max(120.0), 44.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);

    if values.len() < 2 {
        painter.text(
            rect.left_center() + egui::vec2(6.0, 0.0),
            egui::Align2::LEFT_CENTER,
            format!("{label}: menunggu data…"),
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );
        return;
    }

    let (lo, hi) = (*range.start(), *range.end());
    let points = spark_points(values, lo, hi, rect);

    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
    painter.text(
        rect.right_top() + egui::vec2(-6.0, 3.0),
        egui::Align2::RIGHT_TOP,
        format!("{label} {:.0}%", values.last().copied().unwrap_or(0.0)),
        egui::FontId::proportional(11.0),
        ui.visuals().weak_text_color(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 50.0))
    }

    #[test]
    fn higher_value_is_higher_on_screen() {
        let pts = spark_points(&[10.0, 90.0], 0.0, 100.0, rect());
        assert!(pts[1].y < pts[0].y, "90% harus lebih tinggi dari 10%");
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
        assert!((pts[0].y - 50.0).abs() < 0.01, "nilai negatif = dasar");
        assert!((pts[1].y - 0.0).abs() < 0.01, "nilai >100 = puncak");
    }

    #[test]
    fn single_value_does_not_divide_by_zero() {
        let pts = spark_points(&[42.0], 0.0, 100.0, rect());
        assert_eq!(pts.len(), 1);
        assert!(pts[0].x.is_finite() && pts[0].y.is_finite());
    }
}
