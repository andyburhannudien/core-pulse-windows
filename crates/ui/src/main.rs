//! UI core-pulse: tabel proses dan tab kinerja.

#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod format;
mod state;

use core_pulse_core::model::ProcessKey;
use core_pulse_core::{Collector, Snapshot};
use eframe::egui;
use egui_extras::{Column, TableBuilder};
use state::{AppState, PendingAction, PendingKind, SortKey, Tab};
use std::sync::Arc;

fn main() -> eframe::Result<()> {
    // `performance`/`services`/... membuka langsung di tab tertentu, dan
    // `detail:<pid>` membuka panel detail — berguna untuk screenshot dan
    // pengujian manual tanpa bergantung pada posisi piksel.
    let mut start_tab = state::Tab::Processes;
    let mut start_detail: Option<u32> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "performance" | "--performance" => start_tab = state::Tab::Performance,
            "services" | "--services" => start_tab = state::Tab::Services,
            "startup" | "--startup" => start_tab = state::Tab::Startup,
            "users" | "--users" => start_tab = state::Tab::Users,
            other => {
                if let Some(pid) = other.strip_prefix("detail:").and_then(|p| p.parse().ok()) {
                    start_detail = Some(pid);
                }
            }
        }
    }

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
        Box::new(move |cc| Ok(Box::new(App::new(cc, store, start_tab, start_detail)))),
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
fn spawn_sampler(
    store: Arc<core_pulse_core::sampler::SnapshotStore>,
    slow: Arc<core_pulse_core::SlowStore>,
    ctx: egui::Context,
) {
    std::thread::Builder::new()
        .name("sampler".into())
        .spawn(move || {
            let mut collector = Collector::new().with_slow_store(slow);
            let mut ticker =
                core_pulse_core::sampler::Ticker::new(core_pulse_core::sampler::SAMPLE_INTERVAL);
            let mut signatures_on = false;
            loop {
                // Budget tanda tangan hanya aktif saat kolomnya terlihat.
                let wanted = store.signatures_wanted();
                if wanted != signatures_on {
                    collector.set_signature_budget(if wanted { 3 } else { 0 });
                    signatures_on = wanted;
                }
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
        start_detail: Option<u32>,
    ) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let slow = Arc::new(core_pulse_core::SlowStore::new());
        // Service dan startup dibaca di thread sendiri: 700+ entri butuh
        // ~150 ms, yang akan melanggar budget sampler (docs/PERF.md).
        let repaint_ctx = cc.egui_ctx.clone();
        core_pulse_core::spawn_slow_refresher(
            Arc::clone(&slow),
            Some(Box::new(move || repaint_ctx.request_repaint())),
        );
        spawn_sampler(Arc::clone(&store), slow, cc.egui_ctx.clone());
        Self {
            state: AppState {
                tab: start_tab,
                detail_pid: start_detail,
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
        self.store.set_signatures_wanted(self.state.show_signatures);
        self.state.expire_feedback();

        egui::Panel::top("nav").show(ui, |ui| {
            ui.horizontal(|ui| {
                for tab in Tab::all() {
                    ui.selectable_value(&mut self.state.tab, tab, tab.label());
                }
                ui.separator();
                if self.state.tab == Tab::Processes {
                    ui.label("Cari:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.state.search)
                            .hint_text("nama atau PID")
                            .desired_width(200.0),
                    );
                    ui.checkbox(&mut self.state.show_system, "proses sistem");
                    ui.checkbox(&mut self.state.show_signatures, "tanda tangan")
                        .on_hover_text(
                            "Verifikasi Authenticode tiap executable baru.\n\
                             Mahal (5–20 ms per file), jadi dimatikan secara default.",
                        );
                    let has_selection = self.state.selected.is_some();
                    if ui
                        .add_enabled(has_selection, egui::Button::new("Detail"))
                        .on_hover_text(
                            "Tampilkan panel detail proses terpilih (atau klik dua kali baris)",
                        )
                        .clicked()
                    {
                        self.state.detail_pid = self.state.selected;
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let count = match self.state.tab {
                        Tab::Services => self.state.snapshot.services.len(),
                        Tab::Startup => self.state.snapshot.startup.len(),
                        Tab::Users => self.state.snapshot.sessions.len(),
                        _ => self.state.snapshot.processes.len(),
                    };
                    let unit = match self.state.tab {
                        Tab::Services => "layanan",
                        Tab::Startup => "entri",
                        Tab::Users => "sesi",
                        _ => "proses",
                    };
                    ui.label(egui::RichText::new(format!("{count} {unit}")).weak());
                });
            });
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                // Feedback aksi menggantikan sebagian status bar selama 6 detik:
                // lebih terlihat daripada notifikasi terpisah.
                if let Some(f) = &self.state.feedback {
                    let color = if f.is_error {
                        egui::Color32::from_rgb(0xE0, 0x5A, 0x5A)
                    } else {
                        egui::Color32::from_rgb(0x4A, 0xC0, 0x7A)
                    };
                    let icon = if f.is_error { "✕" } else { "✓" };
                    ui.label(egui::RichText::new(format!("{icon} {}", f.text)).color(color));
                    ui.separator();
                }
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
            Tab::Services => self.services(ui),
            Tab::Startup => self.startup(ui),
            Tab::Users => self.users(ui),
        });

        self.confirmation_dialog(&ui.ctx().clone());
    }
}

impl App {
    fn processes(&mut self, ui: &mut egui::Ui) {
        // Panel detail dideklarasikan LEBIH DULU: `Panel::bottom` menyusutkan
        // `ui` induk, jadi tabel di bawahnya otomatis memakai sisa tinggi.
        // Kalau dipanggil setelah tabel, tabel sudah menghabiskan ruang dan
        // panel tidak akan terlihat.
        self.detail_panel(ui);

        let rows = self.state.visible_rows();
        let sort = self.state.sort;
        let sort_asc = self.state.sort_asc;
        let mut clicked: Option<SortKey> = None;
        let mut selected = self.state.selected;
        let mut open_detail: Option<u32> = None;
        // Aksi yang diminta lewat menu kontekstual; dikonfirmasi setelah tabel.
        let mut requested: Option<(PendingKind, ProcessKey, String)> = None;

        let arrow = |key: SortKey| -> &'static str {
            if sort == key {
                if sort_asc { " ▲" } else { " ▼" }
            } else {
                ""
            }
        };

        let show_sig = self.state.show_signatures;
        let avail = ui.available_height();
        TableBuilder::new(ui)
            .id_salt("process_table")
            .striped(true)
            .resizable(false)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::remainder().at_least(220.0).clip(true))
            .column(Column::exact(66.0))
            .column(Column::exact(74.0))
            .column(Column::exact(92.0))
            .column(Column::exact(64.0))
            .column(Column::exact(64.0))
            .column(Column::exact(74.0))
            .column(Column::exact(90.0))
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
                if show_sig {
                    header.col(|ui| {
                        ui.strong("Tanda tangan");
                    });
                }
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
                        let label = ui
                            .label(egui::RichText::new(format::truncate(&p.name, 60)).color(color));
                        // Menu kontekstual pada nama: lebih mudah ditemukan
                        // daripada klik kanan di mana saja pada baris.
                        label.context_menu(|ui| {
                            process_context_menu(ui, p, &mut requested, &mut open_detail);
                        });
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
                    if show_sig {
                        row.col(|ui| match &p.signature {
                            Some(sig) => {
                                let (icon, color, tip) = match sig.status {
                                    core_pulse_core::model::SigStatus::Valid => (
                                        "✓",
                                        egui::Color32::from_rgb(0x4A, 0xC0, 0x7A),
                                        sig.publisher
                                            .clone()
                                            .unwrap_or_else(|| "bertanda tangan".into()),
                                    ),
                                    core_pulse_core::model::SigStatus::Invalid => (
                                        "✕",
                                        egui::Color32::from_rgb(0xE0, 0x5A, 0x5A),
                                        "tanda tangan tidak valid".into(),
                                    ),
                                    core_pulse_core::model::SigStatus::Unsigned => (
                                        "·",
                                        ui.visuals().weak_text_color(),
                                        "tidak bertanda tangan".into(),
                                    ),
                                    core_pulse_core::model::SigStatus::Unknown => (
                                        "?",
                                        ui.visuals().weak_text_color(),
                                        "belum diperiksa".into(),
                                    ),
                                };
                                ui.label(egui::RichText::new(icon).color(color))
                                    .on_hover_text(tip);
                            }
                            None => {
                                ui.label(egui::RichText::new("…").weak())
                                    .on_hover_text("menunggu verifikasi");
                            }
                        });
                    }

                    let resp = row.response();
                    if resp.clicked() {
                        selected = Some(p.pid);
                    }
                    if resp.double_clicked() {
                        open_detail = Some(p.pid);
                    }
                    resp.context_menu(|ui| {
                        process_context_menu(ui, p, &mut requested, &mut open_detail);
                    });
                });
            });

        if let Some(key) = clicked {
            self.state.set_sort(key);
        }
        self.state.selected = selected;

        if let Some(pid) = open_detail {
            self.state.detail_pid = Some(pid);
        }

        // Dialog konfirmasi dibangun dari permintaan menu kontekstual.
        if let Some((kind, key, name)) = requested {
            let descendants = if kind == PendingKind::TerminateTree {
                self.state.descendant_count(key.pid)
            } else {
                0
            };
            self.state.pending = Some(PendingAction {
                kind,
                pid: key.pid,
                name,
                start_time_unix_ms: key.start_time_unix_ms,
                descendants,
            });
        }
    }

    /// Panel detail proses yang dipilih, di bawah tabel.
    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(row) = self.state.detail_row().cloned() else {
            return;
        };
        let mut close = false;

        egui::Panel::bottom("detail_panel")
            .resizable(true)
            .default_size(230.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{} (PID {})", row.name, row.pid)).strong(),
                    );
                    if !row.details_complete {
                        ui.label(egui::RichText::new("membaca detail…").weak().size(10.0));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Tutup").clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Nilai yang belum dibaca ditampilkan "…", bukan "—":
                    // "—" berarti sumbernya menolak/absent (ADR-0008).
                    let field = |ui: &mut egui::Ui, label: &str, value: &str| {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [140.0, 16.0],
                                egui::Label::new(egui::RichText::new(label).weak()),
                            );
                            ui.add(egui::Label::new(egui::RichText::new(value).monospace()).wrap());
                        });
                    };
                    let detail_or = |v: Option<String>| -> String {
                        if row.details_complete {
                            opt_str(v)
                        } else {
                            "…".to_string()
                        }
                    };

                    field(
                        ui,
                        "Path",
                        &opt_str(row.exe_path.as_ref().map(|p| p.display().to_string())),
                    );
                    field(ui, "Command line", &detail_or(row.cmdline.clone()));
                    field(ui, "Pengguna", &detail_or(row.user.clone()));
                    field(
                        ui,
                        "Integritas",
                        &if !row.details_complete {
                            "…".to_string()
                        } else {
                            match row.integrity {
                                Some(i) => format!("{i:?}"),
                                None => "—".into(),
                            }
                        },
                    );
                    field(
                        ui,
                        "Elevated",
                        &match row.elevated {
                            Some(true) => "ya".to_string(),
                            Some(false) => "tidak".to_string(),
                            None => "—".to_string(),
                        },
                    );
                    field(
                        ui,
                        "Tanda tangan",
                        &match &row.signature {
                            Some(s) => match (&s.publisher, s.status) {
                                (Some(p), _) => format!("{p} ({:?})", s.status),
                                (None, st) => format!("{st:?}"),
                            },
                            None => "belum diperiksa".into(),
                        },
                    );
                    field(ui, "CPU time", &format!("{} ms", row.cpu_time_ms));
                    field(
                        ui,
                        "Memori",
                        &format!(
                            "private {}, working set {}, shared {}",
                            format::bytes(row.mem_private_bytes),
                            format::bytes(row.mem_working_set_bytes),
                            format::bytes(row.mem_shared_bytes)
                        ),
                    );
                    field(
                        ui,
                        "I/O total",
                        &format!(
                            "baca {}, tulis {}",
                            format::bytes(row.io_read_bytes),
                            format::bytes(row.io_write_bytes)
                        ),
                    );
                    field(
                        ui,
                        "Thread / handle",
                        &format!("{} / {}", row.threads, row.handles),
                    );
                    field(ui, "Sesi", &row.session_id.to_string());
                    field(
                        ui,
                        "Identitas",
                        &format!("pid {} · dibuat {} ms", row.pid, row.start_time_unix_ms),
                    );
                    if row.eco_qos {
                        field(ui, "Mode efisiensi", "aktif");
                    }

                    if let Some(path) = &row.exe_path {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.button("Buka lokasi file").clicked() {
                                let result = core_pulse_core::sys::action::reveal_in_explorer(path);
                                match result {
                                    Ok(()) => self.state.set_feedback("membuka Explorer", false),
                                    Err(e) => self.state.set_feedback(format!("gagal: {e}"), true),
                                }
                            }
                            if ui.button("Salin command line").clicked() {
                                if let Some(cmd) = &row.cmdline {
                                    ui.ctx().copy_text(cmd.clone());
                                    self.state.set_feedback("command line disalin", false);
                                }
                            }
                            if ui.button("Salin PID").clicked() {
                                ui.ctx().copy_text(row.pid.to_string());
                                self.state.set_feedback("PID disalin", false);
                            }
                        });
                    }
                });
            });

        if close {
            self.state.detail_pid = None;
        }
    }

    fn services(&mut self, ui: &mut egui::Ui) {
        let s = Arc::clone(&self.state.snapshot);
        ui.horizontal(|ui| {
            ui.heading("Layanan");
            ui.label(
                egui::RichText::new(format!(
                    "diperbarui tiap 15 detik · {} layanan",
                    s.services.len()
                ))
                .weak()
                .size(11.0),
            );
        });
        ui.add_space(4.0);

        if s.services.is_empty() {
            ui.label(
                egui::RichText::new("— daftar layanan belum terbaca (menunggu pembaruan pertama)")
                    .weak(),
            );
            return;
        }

        let mut filter = self.state.search.clone();
        ui.horizontal(|ui| {
            ui.label("Saring:");
            ui.add(
                egui::TextEdit::singleline(&mut filter)
                    .hint_text("nama atau display name")
                    .desired_width(260.0),
            );
        });
        self.state.search = filter.clone();
        let needle = filter.trim().to_lowercase();

        let rows: Vec<_> = s
            .services
            .iter()
            .filter(|svc| {
                needle.is_empty()
                    || svc.name.to_lowercase().contains(&needle)
                    || svc.display_name.to_lowercase().contains(&needle)
            })
            .collect();

        let avail = ui.available_height();
        TableBuilder::new(ui)
            .id_salt("service_table")
            .striped(true)
            .resizable(false)
            .column(Column::exact(150.0))
            .column(Column::remainder().at_least(180.0).clip(true))
            .column(Column::exact(90.0))
            .column(Column::exact(110.0))
            .column(Column::exact(70.0))
            .max_scroll_height(avail)
            .header(22.0, |mut h| {
                h.col(|ui| {
                    ui.strong("Nama");
                });
                h.col(|ui| {
                    ui.strong("Nama tampilan");
                });
                h.col(|ui| {
                    ui.strong("Status");
                });
                h.col(|ui| {
                    ui.strong("Start");
                });
                h.col(|ui| {
                    ui.strong("PID");
                });
            })
            .body(|body| {
                body.rows(20.0, rows.len(), |mut row| {
                    let svc = rows[row.index()];
                    row.col(|ui| {
                        ui.monospace(format::truncate(&svc.name, 26));
                    });
                    row.col(|ui| {
                        ui.label(format::truncate(&svc.display_name, 60));
                    });
                    row.col(|ui| {
                        let color = if svc.state == core_pulse_core::model::ServiceRunState::Running
                        {
                            egui::Color32::from_rgb(0x4A, 0xC0, 0x7A)
                        } else {
                            ui.visuals().weak_text_color()
                        };
                        ui.label(egui::RichText::new(svc.state.label()).color(color));
                    });
                    row.col(|ui| {
                        ui.label(svc.start_type.label());
                    });
                    row.col(|ui| {
                        ui.monospace(if svc.pid == 0 {
                            "—".to_string()
                        } else {
                            svc.pid.to_string()
                        });
                    });
                });
            });

        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Menjalankan atau menghentikan layanan butuh hak administrator dan belum \
                 diimplementasikan (P4, lewat helper).",
            )
            .weak()
            .size(11.0),
        );
    }

    fn startup(&mut self, ui: &mut egui::Ui) {
        let s = Arc::clone(&self.state.snapshot);
        ui.horizontal(|ui| {
            ui.heading("Aplikasi saat mulai");
            ui.label(
                egui::RichText::new(format!(
                    "diperbarui tiap 15 detik · {} entri",
                    s.startup.len()
                ))
                .weak()
                .size(11.0),
            );
        });
        ui.add_space(4.0);

        if s.startup.is_empty() {
            ui.label(egui::RichText::new("— belum ada entri startup yang terbaca").weak());
            return;
        }

        let avail = ui.available_height();
        TableBuilder::new(ui)
            .id_salt("startup_table")
            .striped(true)
            .resizable(false)
            .column(Column::remainder().at_least(200.0).clip(true))
            .column(Column::exact(130.0))
            .column(Column::exact(60.0))
            .column(Column::exact(70.0))
            .max_scroll_height(avail)
            .header(22.0, |mut h| {
                h.col(|ui| {
                    ui.strong("Nama");
                });
                h.col(|ui| {
                    ui.strong("Sumber");
                });
                h.col(|ui| {
                    ui.strong("32-bit");
                });
                h.col(|ui| {
                    ui.strong("File");
                });
            })
            .body(|body| {
                body.rows(20.0, s.startup.len(), |mut row| {
                    let e = &s.startup[row.index()];
                    row.col(|ui| {
                        ui.label(format::truncate(&e.name, 60))
                            .on_hover_text(&e.command);
                    });
                    row.col(|ui| {
                        ui.label(e.source.label());
                    });
                    row.col(|ui| {
                        ui.label(if e.is_32bit { "ya" } else { "—" });
                    });
                    row.col(|ui| match e.target_exists {
                        Some(true) => {
                            ui.label(
                                egui::RichText::new("ada")
                                    .color(egui::Color32::from_rgb(0x4A, 0xC0, 0x7A)),
                            );
                        }
                        Some(false) => {
                            ui.label(
                                egui::RichText::new("hilang")
                                    .color(egui::Color32::from_rgb(0xE0, 0x5A, 0x5A)),
                            )
                            .on_hover_text("File yang dirujuk tidak ditemukan di disk");
                        }
                        None => {
                            ui.label(egui::RichText::new("—").weak());
                        }
                    });
                });
            });
    }

    fn users(&mut self, ui: &mut egui::Ui) {
        let s = Arc::clone(&self.state.snapshot);
        ui.horizontal(|ui| {
            ui.heading("Pengguna");
            ui.label(
                egui::RichText::new("sesi Terminal Services dan konsumsi agregatnya")
                    .weak()
                    .size(11.0),
            );
        });
        ui.add_space(4.0);

        if s.sessions.is_empty() {
            ui.label(egui::RichText::new("— belum ada sesi yang terbaca").weak());
            return;
        }

        let avail = ui.available_height();
        TableBuilder::new(ui)
            .id_salt("session_table")
            .striped(true)
            .resizable(false)
            .column(Column::exact(70.0))
            .column(Column::remainder().at_least(200.0).clip(true))
            .column(Column::exact(110.0))
            .column(Column::exact(90.0))
            .column(Column::exact(80.0))
            .column(Column::exact(100.0))
            .max_scroll_height(avail)
            .header(22.0, |mut h| {
                h.col(|ui| {
                    ui.strong("Sesi");
                });
                h.col(|ui| {
                    ui.strong("Pengguna");
                });
                h.col(|ui| {
                    ui.strong("Status");
                });
                h.col(|ui| {
                    ui.strong("Proses");
                });
                h.col(|ui| {
                    ui.strong("CPU");
                });
                h.col(|ui| {
                    ui.strong("Memori");
                });
            })
            .body(|body| {
                body.rows(20.0, s.sessions.len(), |mut row| {
                    let sess = &s.sessions[row.index()];
                    row.col(|ui| {
                        ui.monospace(sess.session_id.to_string());
                    });
                    row.col(|ui| match &sess.user {
                        Some(u) => {
                            ui.label(u.clone());
                        }
                        None => {
                            ui.label(egui::RichText::new("—").weak()).on_hover_text(
                                "Nama pengguna tidak terbaca. Proses di sesi ini \
                                     dimiliki akun sistem; membacanya butuh hak \
                                     administrator (helper, P4).",
                            );
                        }
                    });
                    row.col(|ui| {
                        ui.label(&sess.state);
                    });
                    row.col(|ui| {
                        ui.monospace(sess.process_count.to_string());
                    });
                    row.col(|ui| {
                        ui.monospace(format::percent(sess.cpu_percent));
                    });
                    row.col(|ui| {
                        ui.monospace(format::bytes(sess.mem_private_bytes));
                    });
                });
            });
    }

    /// Dialog konfirmasi untuk aksi yang menunggu.
    fn confirmation_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.state.pending.clone() else {
            return;
        };

        let mut decision: Option<bool> = None;
        egui::Window::new(pending.kind.title())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.label(
                    egui::RichText::new(format!("{} (PID {})", pending.name, pending.pid)).strong(),
                );
                if pending.kind == PendingKind::TerminateTree && pending.descendants > 0 {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} proses turunan juga akan dihentikan.",
                            pending.descendants
                        ))
                        .color(egui::Color32::from_rgb(0xE0, 0xA0, 0x30)),
                    );
                }
                if let PendingKind::SetPriority(p) = pending.kind {
                    ui.label(format!("Prioritas baru: {}", p.label()));
                }
                if pending.kind.is_destructive() {
                    ui.label(
                        egui::RichText::new("Data yang belum disimpan di proses ini akan hilang.")
                            .weak()
                            .size(11.0),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let confirm_text = match pending.kind {
                        PendingKind::Terminate | PendingKind::TerminateTree => "Akhiri",
                        _ => "Lanjutkan",
                    };
                    let button = egui::Button::new(confirm_text);
                    let button = if pending.kind.is_destructive() {
                        button.fill(egui::Color32::from_rgb(0xC0, 0x30, 0x30))
                    } else {
                        button
                    };
                    if ui.add(button).clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Batal").clicked() {
                        decision = Some(false);
                    }
                });
            });

        match decision {
            Some(true) => {
                self.state.pending = None;
                self.execute_action(&pending);
            }
            Some(false) => {
                self.state.pending = None;
                self.state.set_feedback("dibatalkan", false);
            }
            None => {}
        }
    }

    /// Jalankan aksi yang sudah dikonfirmasi, lalu laporkan hasilnya.
    fn execute_action(&mut self, pending: &PendingAction) {
        use core_pulse_core::sys::action;

        let key = ProcessKey {
            pid: pending.pid,
            start_time_unix_ms: pending.start_time_unix_ms,
        };

        let result = match pending.kind {
            PendingKind::Terminate => action::terminate(&key, &pending.name),
            PendingKind::TerminateTree => {
                match self
                    .state
                    .snapshot
                    .processes
                    .iter()
                    .find(|p| p.pid == pending.pid)
                {
                    Some(row) => {
                        let rows = self.state.snapshot.processes.clone();
                        action::terminate_tree(row, &rows)
                    }
                    None => Err(action::ActionError::ProcessGone),
                }
            }
            PendingKind::Suspend => action::suspend(&key, &pending.name),
            PendingKind::Resume => action::resume(&key, &pending.name),
            PendingKind::SetPriority(p) => action::set_priority(&key, &pending.name, p),
            PendingKind::SetEfficiency(on) => action::set_efficiency_mode(&key, &pending.name, on),
        };

        match result {
            Ok(outcome) => {
                tracing::info!(pid = outcome.pid, action = outcome.action, "aksi berhasil");
                self.state.set_feedback(outcome.to_string(), false);
            }
            Err(e) => {
                tracing::warn!(pid = pending.pid, error = %e, "aksi gagal");
                self.state.set_feedback(format!("gagal: {e}"), true);
            }
        }
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

fn opt_str(v: Option<String>) -> String {
    v.filter(|s| !s.is_empty())
        .unwrap_or_else(|| "—".to_string())
}

/// Isi menu kontekstual proses.
///
/// Item yang butuh elevasi tetap ditampilkan tapi nonaktif, supaya pengguna tahu
/// fitur itu ada dan kenapa tidak bisa dipakai (UI-SPEC §7).
fn process_context_menu(
    ui: &mut egui::Ui,
    p: &core_pulse_core::model::ProcessRow,
    requested: &mut Option<(PendingKind, ProcessKey, String)>,
    open_detail: &mut Option<u32>,
) {
    use core_pulse_core::sys::action::Priority;

    let key = p.key();
    let name = p.name.clone();
    let protected = p.pid == 4
        || matches!(
            p.name.to_lowercase().as_str(),
            "system" | "csrss.exe" | "wininit.exe" | "winlogon.exe" | "services.exe" | "lsass.exe"
        );

    ui.set_min_width(220.0);

    ui.add_enabled_ui(!protected, |ui| {
        if ui.button("Akhiri tugas").clicked() {
            *requested = Some((PendingKind::Terminate, key, name.clone()));
            ui.close();
        }
        if ui.button("Akhiri pohon proses").clicked() {
            *requested = Some((PendingKind::TerminateTree, key, name.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Tangguhkan").clicked() {
            *requested = Some((PendingKind::Suspend, key, name.clone()));
            ui.close();
        }
        if ui.button("Lanjutkan").clicked() {
            *requested = Some((PendingKind::Resume, key, name.clone()));
            ui.close();
        }
        ui.separator();
        ui.menu_button("Prioritas", |ui| {
            for priority in Priority::all() {
                if ui.button(priority.label()).clicked() {
                    *requested = Some((PendingKind::SetPriority(priority), key, name.clone()));
                    ui.close();
                }
            }
        });
        let eco_label = if p.eco_qos {
            "Matikan mode efisiensi"
        } else {
            "Aktifkan mode efisiensi"
        };
        if ui.button(eco_label).clicked() {
            *requested = Some((PendingKind::SetEfficiency(!p.eco_qos), key, name.clone()));
            ui.close();
        }
    });

    ui.separator();
    if ui.button("Properti").clicked() {
        *open_detail = Some(p.pid);
        ui.close();
    }
    if let Some(path) = &p.exe_path {
        if ui.button("Buka lokasi file").clicked() {
            let _ = core_pulse_core::sys::action::reveal_in_explorer(path);
            ui.close();
        }
    }
    ui.menu_button("Salin", |ui| {
        if ui.button("Nama").clicked() {
            ui.ctx().copy_text(p.name.clone());
            ui.close();
        }
        if ui.button("PID").clicked() {
            ui.ctx().copy_text(p.pid.to_string());
            ui.close();
        }
        if let Some(path) = &p.exe_path {
            if ui.button("Path").clicked() {
                ui.ctx().copy_text(path.display().to_string());
                ui.close();
            }
        }
        if let Some(cmd) = &p.cmdline {
            if ui.button("Command line").clicked() {
                ui.ctx().copy_text(cmd.clone());
                ui.close();
            }
        }
    });

    if protected {
        ui.separator();
        ui.label(
            egui::RichText::new("proses kritikal: aksi dinonaktifkan")
                .weak()
                .size(10.0),
        );
    }
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
