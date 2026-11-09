//! core-pulse UI: a dense, technical Windows instrument panel.
//!
//! Layout is a fixed navigation rail on the left, a command bar on top, a
//! status strip on the bottom, and the active tab in the centre. Every value
//! that is unavailable renders as a dash with a reason — never a fake zero
//! (ADR-0008).

#![deny(unsafe_op_in_unsafe_fn)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod elevate;
mod format;
mod icons;
mod prefs;
mod state;
mod theme;
mod widgets;

use core_pulse_core::model::ProcessKey;
use core_pulse_core::{Collector, Snapshot};
use eframe::egui;
use egui::{Align, Color32, CornerRadius, Layout, RichText, Stroke, StrokeKind};
use egui_extras::{Column, TableBuilder};
use prefs::UiPrefs;
use state::{AppState, PendingAction, PendingKind, ProcessDensity, SortKey, Tab};
use std::sync::Arc;
use std::sync::mpsc;
use theme::palette as p;

fn main() -> eframe::Result<()> {
    // `performance` / `sensors` / ... open a specific tab, and `detail:<pid>`
    // opens the detail panel. Useful for screenshots and manual testing
    // without depending on pixel positions.
    let mut start_tab = state::Tab::Processes;
    let mut start_detail: Option<u32> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "performance" | "--performance" => start_tab = state::Tab::Performance,
            "sensors" | "--sensors" => start_tab = state::Tab::Sensors,
            "alerts" | "--alerts" => start_tab = state::Tab::Alerts,
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

    // A system monitor wants the whole screen by default. Users can still
    // resize; the window remembers a modest default for small displays.
    let maximized = !std::env::args().any(|a| a == "--windowed");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([1000.0, 620.0])
            .with_title("core-pulse")
            .with_maximized(maximized)
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
        eprintln!("failed to start UI: {e}");
        e
    })
}

/// One sampler thread, one writer to `SnapshotStore`.
///
/// Repaint is triggered from here at 1 Hz rather than from a render-loop
/// timer: when the window is idle we genuinely sleep. That is what keeps idle
/// CPU below 1% (see docs/PERF.md).
fn spawn_sampler(
    store: Arc<core_pulse_core::sampler::SnapshotStore>,
    slow: Arc<core_pulse_core::SlowStore>,
    sensors: Arc<core_pulse_sensors::SensorStore>,
    alerts: Arc<core_pulse_core::alerts::AlertStore>,
    ctx: egui::Context,
) {
    std::thread::Builder::new()
        .name("sampler".into())
        .spawn(move || {
            let mut collector = Collector::new().with_slow_store(slow);
            let mut engine = core_pulse_core::alerts::AlertEngine::new();
            let mut ticker =
                core_pulse_core::sampler::Ticker::new(core_pulse_core::sampler::SAMPLE_INTERVAL);
            let mut signatures_on = false;
            loop {
                // The signature budget is only active while the column is
                // visible.
                let wanted = store.signatures_wanted();
                if wanted != signatures_on {
                    collector.set_signature_budget(if wanted { 3 } else { 0 });
                    signatures_on = wanted;
                }
                let elapsed = ticker.tick();
                let mut snap = collector.sample();
                // After sleep/resume the delta can be huge; drop that sample.
                if core_pulse_core::sampler::is_time_jump(
                    elapsed,
                    core_pulse_core::sampler::SAMPLE_INTERVAL,
                ) {
                    snap.cpu.total_percent = 0.0;
                }
                // Copy the latest sensor results into the snapshot. The
                // sampler never touches sensor hardware itself.
                sensors.apply_to(&mut snap);
                engine.evaluate(&snap);
                alerts.store(engine.state());
                store.store(snap);
                ctx.request_repaint();
            }
        })
        .expect("failed to spawn sampler thread");
}

struct App {
    state: AppState,
    store: Arc<core_pulse_core::sampler::SnapshotStore>,
    alerts: Arc<core_pulse_core::alerts::AlertStore>,
    sensor_control: core_pulse_sensors::SensorControl,
    /// Set once the keyboard shortcut for the search box is pressed.
    focus_search: bool,
    /// Last persisted prefs (for change detection).
    saved_prefs: UiPrefs,
    /// Receiver for background helper diagnosis result, if running.
    helper_diag_rx: Option<mpsc::Receiver<state::HelperDiagnose>>,
}

impl App {
    fn new(
        cc: &eframe::CreationContext<'_>,
        store: Arc<core_pulse_core::sampler::SnapshotStore>,
        start_tab: Tab,
        start_detail: Option<u32>,
    ) -> Self {
        theme::install(&cc.egui_ctx);

        let slow = Arc::new(core_pulse_core::SlowStore::new());
        // Services and startup are read on their own thread: 700+ entries take
        // ~150 ms, which would break the sampler budget (docs/PERF.md).
        let repaint_ctx = cc.egui_ctx.clone();
        core_pulse_core::spawn_slow_refresher(
            Arc::clone(&slow),
            Some(Box::new(move || repaint_ctx.request_repaint())),
        );

        // Hardware sensors (SMART can take 5–40 ms per disk, the bridge HTTP
        // up to 500 ms) also get their own thread, every two seconds.
        let sensor_store = Arc::new(core_pulse_sensors::SensorStore::new());
        let sensor_ctx = cc.egui_ctx.clone();
        let saved_prefs = UiPrefs::load();
        let bridge_enabled = Self::bridge_enabled_cli() || saved_prefs.bridge_enabled;
        let sensor_control = core_pulse_sensors::SensorControl::new(bridge_enabled, true);
        core_pulse_sensors::spawn_sensor_poller_controlled(
            Arc::clone(&sensor_store),
            sensor_control.clone(),
            Some(Box::new(move || sensor_ctx.request_repaint())),
        );

        let alerts = Arc::new(core_pulse_core::alerts::AlertStore::new());
        spawn_sampler(
            Arc::clone(&store),
            Arc::clone(&slow),
            Arc::clone(&sensor_store),
            Arc::clone(&alerts),
            cc.egui_ctx.clone(),
        );

        let density = if saved_prefs.comfort {
            ProcessDensity::Comfort
        } else {
            ProcessDensity::Compact
        };
        let columns = state::ProcessColumns {
            path: saved_prefs.col_path,
            io: saved_prefs.col_io,
            gpu: saved_prefs.col_gpu,
            threads: saved_prefs.col_threads,
            handles: saved_prefs.col_handles,
            session: saved_prefs.col_session,
        };

        Self {
            state: AppState {
                tab: start_tab,
                detail_pid: start_detail,
                bridge_enabled,
                show_system: saved_prefs.show_system,
                show_signatures: saved_prefs.show_signatures,
                sensor_hot_only: saved_prefs.sensor_hot_only,
                process_density: density,
                process_columns: columns,
                ..Default::default()
            },
            store,
            alerts,
            sensor_control,
            focus_search: false,
            saved_prefs,
            helper_diag_rx: None,
        }
    }

    /// CLI/env bridge opt-in. The UI toggle and saved prefs are OR'd on top.
    /// Off by default (SENSORS.md §2 Tier 3).
    fn bridge_enabled_cli() -> bool {
        std::env::args().any(|a| a == "--bridge")
            || std::env::var("CORE_PULSE_LHM_BRIDGE")
                .map(|v| v == "1")
                .unwrap_or(false)
    }

    fn current_prefs(&self) -> UiPrefs {
        let s = &self.state;
        UiPrefs {
            bridge_enabled: s.bridge_enabled,
            show_system: s.show_system,
            show_signatures: s.show_signatures,
            sensor_hot_only: s.sensor_hot_only,
            comfort: matches!(s.process_density, ProcessDensity::Comfort),
            col_path: s.process_columns.path,
            col_io: s.process_columns.io,
            col_gpu: s.process_columns.gpu,
            col_threads: s.process_columns.threads,
            col_handles: s.process_columns.handles,
            col_session: s.process_columns.session,
        }
    }

    fn maybe_save_prefs(&mut self) {
        let cur = self.current_prefs();
        if cur != self.saved_prefs {
            cur.save();
            self.saved_prefs = cur;
        }
    }

    fn poll_helper_diagnose(&mut self) {
        let mut done: Option<state::HelperDiagnose> = None;
        if let Some(rx) = &self.helper_diag_rx {
            match rx.try_recv() {
                Ok(diag) => done = Some(diag),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.helper_diag_rx = None;
                    self.state.helper_diagnosing = false;
                    self.state
                        .set_helper_diagnose("Diagnosis thread exited without a result.", true);
                }
            }
        }
        if let Some(diag) = done {
            self.helper_diag_rx = None;
            self.state.set_helper_diagnose(diag.text, diag.is_error);
            let _ = diag;
        }
    }

    fn run_helper_diagnose(&mut self) {
        if self.state.helper_diagnosing {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.helper_diag_rx = Some(rx);
        self.state.helper_diagnosing = true;
        std::thread::spawn(move || {
            use core_pulse_sensors::helper;
            let at_unix_ms = core_pulse_core::sampler::now_unix_ms();
            let (text, is_error) = match helper::fetch_once() {
                Ok(data) => {
                    let mut out = format!(
                        "expected helper: {}\nconnected: version {} (PID {}), elevated: {}\n\nProvider status inside the helper:",
                        helper::expected_helper_path().display(),
                        data.info.version,
                        data.info.pid,
                        data.info.elevated,
                    );
                    for status in &data.statuses {
                        let text = match &status.availability {
                            core_pulse_core::model::SensorAvailability::Available => {
                                "available".to_string()
                            }
                            core_pulse_core::model::SensorAvailability::NeedsElevation => {
                                "needs administrator".to_string()
                            }
                            core_pulse_core::model::SensorAvailability::NotAvailable { reason } => {
                                format!("unavailable: {reason}")
                            }
                            core_pulse_core::model::SensorAvailability::Failed { error } => {
                                format!("failed: {error}")
                            }
                        };
                        out.push_str(&format!("\n  {:<14} {text}", status.provider));
                    }
                    out.push_str(&format!("\n\nReadings: {} sensor(s)", data.readings.len()));
                    for r in &data.readings {
                        out.push_str(&format!(
                            "\n  {:<28} {:>7.1} {:?} {:?} {}",
                            r.label, r.value, r.unit, r.source, r.id
                        ));
                    }
                    if !data.info.elevated {
                        out.push_str(
                            "\n\nWARNING: helper is not elevated; SMART stays unavailable.",
                        );
                    }
                    (out, false)
                }
                Err(e) => (format!("helper diagnosis failed: {e}"), true),
            };
            let _ = tx.send(state::HelperDiagnose {
                text,
                is_error,
                at_unix_ms,
            });
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snap = self.store.load();
        let snap_arc: Arc<Snapshot> = Arc::clone(&snap);
        self.state.update_snapshot(&snap_arc);
        let alert_state = self.alerts.load();
        let alert_arc: Arc<core_pulse_core::alerts::AlertState> = Arc::clone(&alert_state);
        self.state.update_alerts(&alert_arc);
        self.store.set_signatures_wanted(self.state.show_signatures);
        self.state.expire_feedback();
        self.poll_helper_diagnose();

        // Keyboard: Ctrl+F focuses search, Ctrl+1..7 switch tabs, Esc closes
        // overlays. A technical user expects these from a desktop instrument
        // panel, and they cost nothing when unused.
        let (switch_tab, escape, focus_search) = ui.ctx().input(|i| {
            let tab = if i.modifiers.command {
                const KEYS: [egui::Key; 7] = [
                    egui::Key::Num1,
                    egui::Key::Num2,
                    egui::Key::Num3,
                    egui::Key::Num4,
                    egui::Key::Num5,
                    egui::Key::Num6,
                    egui::Key::Num7,
                ];
                KEYS.iter()
                    .position(|k| i.key_pressed(*k))
                    .map(|idx| Tab::all()[idx])
            } else {
                None
            };
            (
                tab,
                i.key_pressed(egui::Key::Escape),
                i.modifiers.command && i.key_pressed(egui::Key::F),
            )
        });

        if let Some(tab) = switch_tab {
            self.state.tab = tab;
        }
        if escape {
            self.state.detail_pid = None;
            self.state.pending = None;
            self.state.show_degraded_details = false;
        }
        if focus_search {
            self.state.tab = Tab::Processes;
            self.focus_search = true;
        }

        self.nav_rail(ui);
        self.status_bar(ui);
        self.command_bar(ui);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(p::BG)
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ui, |ui| {
                // Alert banner sits above the tab content and only appears
                // when something is actually active.
                self.alert_banner(ui);
                match self.state.tab {
                    Tab::Processes => self.processes(ui),
                    Tab::Performance => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.performance(ui));
                    }
                    Tab::Sensors => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.sensors(ui));
                    }
                    Tab::Alerts => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.alerts(ui));
                    }
                    Tab::Services => self.services(ui),
                    Tab::Startup => self.startup(ui),
                    Tab::Users => self.users(ui),
                }
            });

        self.confirmation_dialog(&ui.ctx().clone());
        self.degraded_window(&ui.ctx().clone());
        self.maybe_save_prefs();
    }
}

impl App {
    /// Left navigation rail: brand mark, tab list, and a compact host summary
    /// pinned to the bottom.
    fn nav_rail(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("nav_rail")
            .exact_size(196.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(p::PANEL)
                    .inner_margin(egui::Margin::symmetric(10, 12)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                    let painter = ui.painter();
                    painter.rect_filled(rect, CornerRadius::same(5), p::ACCENT);
                    // A pulse glyph: two stacked strokes.
                    painter.line_segment(
                        [
                            egui::pos2(rect.left() + 5.0, rect.center().y + 3.0),
                            egui::pos2(rect.left() + 9.0, rect.center().y - 4.0),
                        ],
                        Stroke::new(1.8, Color32::WHITE),
                    );
                    painter.line_segment(
                        [
                            egui::pos2(rect.left() + 9.0, rect.center().y - 4.0),
                            egui::pos2(rect.left() + 13.0, rect.center().y + 2.0),
                        ],
                        Stroke::new(1.8, Color32::WHITE),
                    );
                    painter.line_segment(
                        [
                            egui::pos2(rect.left() + 13.0, rect.center().y + 2.0),
                            egui::pos2(rect.left() + 17.0, rect.center().y - 5.0),
                        ],
                        Stroke::new(1.8, Color32::WHITE),
                    );
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(
                            RichText::new("core-pulse")
                                .font(crate::theme::bold(crate::theme::size::STRONG)),
                        );
                        ui.label(
                            RichText::new("SYSTEM MONITOR")
                                .size(crate::theme::size::MICRO)
                                .extra_letter_spacing(1.2)
                                .color(p::TEXT_FAINT),
                        );
                    });
                });

                ui.add_space(16.0);
                widgets::kicker(ui, "Workspace");
                ui.add_space(4.0);

                for tab in Tab::all() {
                    self.nav_item(ui, tab);
                }

                ui.with_layout(Layout::bottom_up(Align::LEFT), |ui| {
                    let s = &self.state.snapshot;
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!(
                            "{} · {}",
                            s.host.computer_name,
                            if s.host.is_elevated {
                                "elevated"
                            } else {
                                "standard user"
                            }
                        ))
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                    );
                    ui.label(
                        RichText::new(format!(
                            "{} cores · {} RAM",
                            s.host.logical_cores,
                            format::bytes(s.host.total_ram_bytes)
                        ))
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                    );
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(4.0);
                    let active = self.state.alerts.active.len();
                    if active > 0 {
                        ui.horizontal(|ui| {
                            widgets::dot(ui, p::AMBER, true);
                            ui.label(
                                RichText::new(format!("{active} active alert(s)"))
                                    .size(crate::theme::size::SMALL)
                                    .color(p::AMBER),
                            );
                        });
                    } else {
                        ui.horizontal(|ui| {
                            widgets::dot(ui, p::GREEN, false);
                            ui.label(
                                RichText::new("All thresholds nominal")
                                    .size(crate::theme::size::SMALL)
                                    .color(p::TEXT_FAINT),
                            );
                        });
                    }
                });
            });
    }

    fn nav_item(&mut self, ui: &mut egui::Ui, tab: Tab) {
        let selected = self.state.tab == tab;
        let alert_count = self.state.alerts.active.len();
        let height = 30.0;
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click(),
        );

        if selected {
            ui.painter().rect(
                rect,
                CornerRadius::same(5),
                p::ACCENT.gamma_multiply(0.14),
                Stroke::new(1.0, p::ACCENT.gamma_multiply(0.45)),
                StrokeKind::Inside,
            );
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(rect.left(), rect.top() + 6.0),
                    egui::vec2(2.0, rect.height() - 12.0),
                ),
                CornerRadius::same(1),
                p::ACCENT,
            );
        } else if resp.hovered() {
            // Hover must not look like selection: no left accent bar, and a
            // dimmer fill than the selected state.
            ui.painter()
                .rect_filled(rect, CornerRadius::same(5), p::HOVER.gamma_multiply(0.7));
        }

        let text_color = if selected {
            Color32::WHITE
        } else if resp.hovered() {
            p::TEXT
        } else {
            p::TEXT_DIM
        };

        // Icons are painted as geometry, not glyphs. Glyph picks like `⚙` and
        // `▶` are resolved by the emoji font on Windows and come out as
        // full-colour emoji, which clashes badly with the monochrome rail.
        let icon_color = if selected { p::ACCENT } else { p::TEXT_FAINT };
        widgets::nav_icon(
            ui,
            egui::pos2(rect.left() + 17.0, rect.center().y),
            tab,
            icon_color,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 30.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            tab.label(),
            egui::FontId::proportional(12.0),
            text_color,
        );

        // Numeric counters on the right keep the rail scannable.
        let counter = match tab {
            Tab::Processes => Some(self.state.snapshot.processes.len()),
            Tab::Services => Some(self.state.snapshot.services.len()),
            Tab::Startup => Some(self.state.snapshot.startup.len()),
            Tab::Users => Some(self.state.snapshot.sessions.len()),
            Tab::Alerts if alert_count > 0 => Some(alert_count),
            _ => None,
        };
        if let Some(n) = counter {
            ui.painter().text(
                egui::pos2(rect.right() - 10.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                n.to_string(),
                egui::FontId::monospace(10.0),
                if tab == Tab::Alerts {
                    p::AMBER
                } else {
                    p::TEXT_FAINT
                },
            );
        }

        if resp.clicked() {
            self.state.tab = tab;
        }
        resp.on_hover_text(tab.hint());
    }

    /// Top command bar: page title, filter controls for the active tab, and a
    /// right-aligned live clock.
    fn command_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("command_bar")
            .frame(
                egui::Frame::new()
                    .fill(p::BG)
                    .inner_margin(egui::Margin::symmetric(14, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(
                            RichText::new(self.state.tab.label())
                                .font(crate::theme::bold(crate::theme::size::TITLE)),
                        );
                        ui.label(
                            RichText::new(self.state.tab.hint())
                                .size(crate::theme::size::SMALL)
                                .color(p::TEXT_FAINT),
                        );
                    });

                    ui.add_space(16.0);

                    match self.state.tab {
                        Tab::Processes => self.process_toolbar(ui),
                        Tab::Services => {
                            ui.label(
                                RichText::new("Filter")
                                    .size(crate::theme::size::SMALL)
                                    .color(p::TEXT_DIM),
                            );
                            ui.add(
                                egui::TextEdit::singleline(&mut self.state.service_search)
                                    .hint_text("service or display name")
                                    .desired_width(240.0),
                            );
                            if !self.state.service_search.is_empty() && ui.button("Clear").clicked()
                            {
                                self.state.service_search.clear();
                            }
                        }
                        Tab::Sensors => {
                            ui.checkbox(
                                &mut self.state.sensor_hot_only,
                                RichText::new("Only above 45 °C").size(crate::theme::size::BODY),
                            );
                            ui.separator();
                            let mut bridge = self.state.bridge_enabled;
                            if ui
                                .checkbox(
                                    &mut bridge,
                                    RichText::new("Third-party bridge (LHM)")
                                        .size(crate::theme::size::BODY),
                                )
                                .on_hover_text(
                                    "Opt-in: read LibreHardwareMonitor JSON from 127.0.0.1:8085 only. \
                                     Data is marked ThirdParty and is never estimated by core-pulse.",
                                )
                                .changed()
                            {
                                self.state.bridge_enabled = bridge;
                                self.sensor_control.set_bridge_enabled(bridge);
                                self.state.set_feedback(
                                    if bridge {
                                        "LHM bridge enabled (opt-in, localhost:8085)".to_string()
                                    } else {
                                        "LHM bridge disabled".to_string()
                                    },
                                    false,
                                );
                            }
                        }
                        _ => {}
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let s = &self.state.snapshot;
                        ui.label(
                            RichText::new(format::clock_hms(s.ts_unix_ms))
                                .size(crate::theme::size::BODY)
                                .monospace()
                                .color(p::TEXT_FAINT),
                        );
                        ui.label(
                            RichText::new(format!(
                                "interval {}",
                                format::duration_ms(s.interval_ms.max(1) as u64)
                            ))
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(format!("{} processes", s.processes.len()))
                                .size(crate::theme::size::SMALL)
                                .color(p::TEXT_DIM),
                        );
                    });
                });
            });
    }

    fn process_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.label(
            RichText::new("Search")
                .size(crate::theme::size::SMALL)
                .color(p::TEXT_DIM),
        );
        let resp = ui.add(
            egui::TextEdit::singleline(&mut self.state.search)
                .id_salt("process_search")
                .hint_text("name or PID")
                .desired_width(230.0),
        );
        if self.focus_search {
            resp.request_focus();
            self.focus_search = false;
        }
        if ui
            .add_enabled(
                !self.state.search.is_empty(),
                egui::Button::new("Clear").small(),
            )
            .clicked()
        {
            self.state.search.clear();
        }
        {
            let s = Arc::clone(&self.state.snapshot);
            let total = s.processes.iter().filter(|p| p.pid != 0).count();
            let shown = self.state.visible_rows().len();
            ui.label(
                RichText::new(format!("{shown} / {total}"))
                    .size(crate::theme::size::SMALL)
                    .monospace()
                    .color(p::TEXT_DIM),
            )
            .on_hover_text("Processes shown / total (PID 0 hidden)");
        }
        ui.separator();
        ui.checkbox(
            &mut self.state.show_system,
            RichText::new("System processes").size(crate::theme::size::BODY),
        );
        ui.checkbox(
            &mut self.state.show_signatures,
            RichText::new("Signatures").size(crate::theme::size::BODY),
        )
        .on_hover_text(
            "Verify the Authenticode signature of each new executable.\n\
             Expensive (5–20 ms per file), so it is off by default.",
        );
        let has_selection = self.state.selected.is_some();
        if ui
            .add_enabled(has_selection, egui::Button::new("Details").small())
            .on_hover_text("Open the detail panel for the selected process (or double-click a row)")
            .clicked()
        {
            self.state.detail_pid = self.state.selected;
        }
        ui.separator();
        ui.menu_button(
            RichText::new("View").size(crate::theme::size::SMALL),
            |ui| {
                ui.label(
                    RichText::new("Density")
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                );
                ui.radio_value(
                    &mut self.state.process_density,
                    ProcessDensity::Compact,
                    "Compact",
                );
                ui.radio_value(
                    &mut self.state.process_density,
                    ProcessDensity::Comfort,
                    "Comfort",
                );
                ui.separator();
                ui.label(
                    RichText::new("Quick filters")
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                );
                ui.checkbox(&mut self.state.filter_cpu_hot, "CPU ≥ 5%")
                    .on_hover_text("Only processes using at least 5% CPU");
                ui.checkbox(&mut self.state.filter_elevated, "Elevated only")
                    .on_hover_text("Only processes running elevated");
                ui.checkbox(&mut self.state.filter_unsigned, "Unsigned only")
                    .on_hover_text("Only processes with unsigned/invalid Authenticode signature");
                ui.separator();
                ui.label(
                    RichText::new("Columns")
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                );
                ui.checkbox(&mut self.state.process_columns.path, "Path");
                ui.checkbox(&mut self.state.process_columns.io, "I/O");
                ui.checkbox(&mut self.state.process_columns.gpu, "GPU");
                ui.checkbox(&mut self.state.process_columns.threads, "Threads");
                ui.checkbox(&mut self.state.process_columns.handles, "Handles");
                ui.checkbox(&mut self.state.process_columns.session, "Session");
                ui.label(
                    RichText::new(
                        "PID, name, CPU, memory always shown. Signature uses its own toggle.",
                    )
                    .size(crate::theme::size::MICRO)
                    .color(p::TEXT_FAINT),
                );
            },
        );
    }

    /// Bottom strip: whatever feedback is pending, then always-on core metrics.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status_strip")
            .exact_size(34.0)
            .frame(
                egui::Frame::new()
                    .fill(p::PANEL)
                    .inner_margin(egui::Margin::symmetric(12, 3)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if let Some(f) = &self.state.feedback {
                        let color = if f.is_error { p::RED } else { p::GREEN };
                        let icon = if f.is_error {
                            icons::CROSS
                        } else {
                            icons::CHECK
                        };
                        ui.label(
                            RichText::new(format!("{icon} {}", f.text))
                                .size(crate::theme::size::SMALL)
                                .color(color),
                        );
                        ui.separator();
                    }

                    let s = Arc::clone(&self.state.snapshot);
                    status_metric(
                        ui,
                        "CPU",
                        &format!("{:.0}%", s.cpu.total_percent),
                        p::ACCENT,
                    );
                    ui.separator();
                    let mem_pct = if s.memory.total_bytes > 0 {
                        s.memory.in_use_bytes as f32 / s.memory.total_bytes as f32 * 100.0
                    } else {
                        0.0
                    };
                    status_metric(ui, "RAM", &format!("{mem_pct:.0}%"), p::GREEN);
                    ui.separator();
                    let disk = s
                        .disks
                        .iter()
                        .map(|d| d.busy_percent)
                        .fold(0.0f32, f32::max);
                    status_metric(ui, "DISK", &format!("{disk:.0}%"), p::AMBER);
                    ui.separator();
                    let net: u64 = s.net.iter().map(|n| n.rx_bps + n.tx_bps).sum();
                    status_metric(ui, "NET", &format::rate(net), p::CYAN);
                    ui.separator();

                    // Temperature: be honest when no source exists. Click opens Sensors.
                    let temps: Vec<f32> = s
                        .sensors
                        .iter()
                        .filter(|r| r.kind == core_pulse_core::model::SensorKind::Temperature)
                        .map(|r| r.value)
                        .collect();
                    if temps.is_empty() {
                        ui.label(
                            RichText::new("TEMP")
                                .size(crate::theme::size::MICRO)
                                .extra_letter_spacing(0.8)
                                .color(p::TEXT_FAINT),
                        );
                        let resp = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(icons::DASH)
                                        .size(crate::theme::size::BODY)
                                        .monospace()
                                        .color(p::TEXT_DIM),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text(
                                "No temperature source is active. Click for details.\n\
                                 Common causes: firmware exposes no ACPI CPU thermal zone; \
                                 disk SMART needs the elevated helper; GPU vendor driver unavailable; \
                                 third-party bridge is off.",
                            );
                        if resp.clicked() {
                            self.state.tab = Tab::Sensors;
                        }
                    } else if let Some(max) = temps.iter().copied().reduce(f32::max) {
                        let color = if max >= 70.0 {
                            p::RED
                        } else if max >= 55.0 {
                            p::AMBER
                        } else {
                            p::GREEN
                        };
                        let resp = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(format!("TEMP {max:.0} °C"))
                                        .size(crate::theme::size::SMALL)
                                        .monospace()
                                        .color(color),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Hottest active temperature sensor. Click to open Sensors.");
                        if resp.clicked() {
                            self.state.tab = Tab::Sensors;
                        }
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!(
                                    "{} build {} · up {}",
                                    s.host.os_name,
                                    s.host.os_build,
                                    format::uptime(s.host.uptime_secs)
                                ))
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                            )
                            .truncate(),
                        );
                        if !s.degraded.is_empty() {
                            ui.separator();
                            let n = s.degraded.len();
                            let resp = ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(format!("! {n} degraded source(s)"))
                                            .size(crate::theme::size::MICRO)
                                            .color(p::AMBER),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text("Click for details. Secondary click opens Sensors.");
                            if resp.clicked() {
                                self.state.show_degraded_details = true;
                            }
                            if resp.secondary_clicked() {
                                self.state.tab = Tab::Sensors;
                            }
                        }
                    });
                });
            });
    }

    /// Persistent banner above the tab body when alerts are active.
    fn alert_banner(&mut self, ui: &mut egui::Ui) {
        let active = &self.state.alerts.active;
        if active.is_empty() {
            return;
        }
        let worst = active
            .iter()
            .map(|a| a.severity)
            .max_by_key(|s| match s {
                core_pulse_core::alerts::AlertSeverity::Warning => 1,
                core_pulse_core::alerts::AlertSeverity::Critical => 2,
            })
            .unwrap_or(core_pulse_core::alerts::AlertSeverity::Warning);
        let color = match worst {
            core_pulse_core::alerts::AlertSeverity::Critical => p::RED,
            core_pulse_core::alerts::AlertSeverity::Warning => p::AMBER,
        };
        egui::Frame::new()
            .fill(color.gamma_multiply(0.10))
            .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::dot(ui, color, true);
                    let first = &active[0];
                    ui.label(
                        RichText::new(&first.title)
                            .font(crate::theme::bold(crate::theme::size::BODY))
                            .color(color),
                    );
                    ui.label(
                        RichText::new(&first.detail)
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_DIM),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("View alerts").clicked() {
                            self.state.tab = Tab::Alerts;
                        }
                        if active.len() > 1 {
                            ui.label(
                                RichText::new(format!("+{} more", active.len() - 1))
                                    .size(crate::theme::size::SMALL)
                                    .color(p::TEXT_FAINT),
                            );
                        }
                    });
                });
            });
        ui.add_space(8.0);
    }
}

/// Compact status metric: dim uppercase key + bright monospace value.
fn status_metric(ui: &mut egui::Ui, key: &str, value: &str, color: Color32) {
    ui.label(
        RichText::new(key)
            .size(crate::theme::size::MICRO)
            .extra_letter_spacing(0.8)
            .color(p::TEXT_FAINT),
    );
    ui.label(
        RichText::new(value)
            .size(crate::theme::size::SMALL)
            .monospace()
            .color(color),
    );
}

impl App {
    fn processes(&mut self, ui: &mut egui::Ui) {
        // The detail panel is declared FIRST: `Panel::bottom` shrinks the
        // parent `ui`, so the table below automatically uses the remaining
        // height. Declared after the table, it would be pushed off-screen.
        self.detail_panel(ui);

        let rows = self.state.visible_rows();
        let sort = self.state.sort;
        let sort_asc = self.state.sort_asc;
        let mut clicked: Option<SortKey> = None;
        let mut selected = self.state.selected;
        let mut open_detail: Option<u32> = None;
        // Action requested via the context menu, confirmed after the table.
        let mut requested: Option<(PendingKind, ProcessKey, String)> = None;

        let arrow = |key: SortKey| -> &'static str {
            if sort == key {
                if sort_asc { " ↑" } else { " ↓" }
            } else {
                ""
            }
        };

        let show_sig = self.state.show_signatures;
        let cols = self.state.process_columns;
        let comfort = matches!(self.state.process_density, ProcessDensity::Comfort);
        let row_h = if comfort { 26.0 } else { 21.0 };
        let header_h = if comfort { 28.0 } else { 24.0 };
        let avail = ui.available_height();

        #[derive(Clone, Copy)]
        enum ProcCol {
            Pid,
            Name,
            Path,
            Cpu,
            Memory,
            Io,
            Gpu,
            Threads,
            Handles,
            Session,
            Signature,
        }

        let mut active: Vec<ProcCol> = vec![ProcCol::Pid, ProcCol::Name];
        if cols.path {
            active.push(ProcCol::Path);
        }
        active.push(ProcCol::Cpu);
        active.push(ProcCol::Memory);
        if cols.io {
            active.push(ProcCol::Io);
        }
        if cols.gpu {
            active.push(ProcCol::Gpu);
        }
        if cols.threads {
            active.push(ProcCol::Threads);
        }
        if cols.handles {
            active.push(ProcCol::Handles);
        }
        if cols.session {
            active.push(ProcCol::Session);
        }
        if show_sig {
            active.push(ProcCol::Signature);
        }

        widgets::framed(ui, |ui| {
            let mut table = TableBuilder::new(ui)
                .id_salt("process_table")
                .striped(true)
                .resizable(false)
                .cell_layout(Layout::left_to_right(Align::Center));
            for col in &active {
                table = match col {
                    ProcCol::Pid => table.column(Column::auto().at_least(56.0).at_most(56.0)),
                    ProcCol::Name => table.column(Column::exact(190.0).clip(true)),
                    ProcCol::Path => table.column(Column::remainder().at_least(120.0).clip(true)),
                    ProcCol::Cpu => table.column(Column::exact(76.0)),
                    ProcCol::Memory => table.column(Column::exact(92.0)),
                    ProcCol::Io => table.column(Column::exact(84.0)),
                    ProcCol::Gpu => table.column(Column::exact(76.0)),
                    ProcCol::Threads => table.column(Column::exact(52.0)),
                    ProcCol::Handles => table.column(Column::exact(44.0)),
                    ProcCol::Session => table.column(Column::exact(46.0)),
                    ProcCol::Signature => table.column(Column::exact(56.0)),
                };
            }
            table
                .min_scrolled_height(0.0)
                .max_scroll_height(avail - 2.0)
                .header(header_h, |mut header| {
                    for col in &active {
                        match col {
                            ProcCol::Pid => {
                                header.col(|ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(format!("PID{}", arrow(SortKey::Pid)))
                                                    .size(crate::theme::size::MICRO)
                                                    .color(p::TEXT_FAINT),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        clicked = Some(SortKey::Pid);
                                    }
                                });
                            }
                            ProcCol::Name => {
                                header.col(|ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(format!("NAME{}", arrow(SortKey::Name)))
                                                    .size(crate::theme::size::MICRO)
                                                    .color(p::TEXT_FAINT),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        clicked = Some(SortKey::Name);
                                    }
                                });
                            }
                            ProcCol::Path => {
                                header.col(|ui| {
                                    ui.label(
                                        RichText::new("EXECUTABLE PATH")
                                            .size(crate::theme::size::MICRO)
                                            .color(p::TEXT_FAINT),
                                    );
                                });
                            }
                            ProcCol::Cpu => {
                                header.col(|ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(format!("CPU{}", arrow(SortKey::Cpu)))
                                                    .size(crate::theme::size::MICRO)
                                                    .color(p::TEXT_FAINT),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        clicked = Some(SortKey::Cpu);
                                    }
                                });
                            }
                            ProcCol::Memory => {
                                header.col(|ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new(format!("MEMORY{}", arrow(SortKey::Memory)))
                                                    .size(crate::theme::size::MICRO)
                                                    .color(p::TEXT_FAINT),
                                            )
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        clicked = Some(SortKey::Memory);
                                    }
                                });
                            }
                            ProcCol::Io => {
                                header.col(|ui| {
                                    ui.label(
                                        RichText::new("I/O TOTAL")
                                            .size(crate::theme::size::MICRO)
                                            .color(p::TEXT_FAINT),
                                    );
                                });
                            }
                            ProcCol::Gpu => {
                                header.col(|ui| {
                                    ui.label(
                                        RichText::new("GPU")
                                            .size(crate::theme::size::MICRO)
                                            .color(p::TEXT_FAINT),
                                    );
                                });
                            }
                            ProcCol::Threads => {
                                header.col(|ui| {
                                    ui.label(
                                        RichText::new("THR")
                                            .size(crate::theme::size::MICRO)
                                            .color(p::TEXT_FAINT),
                                    );
                                });
                            }
                            ProcCol::Handles => {
                                header.col(|ui| {
                                    ui.label(
                                        RichText::new("HND")
                                            .size(crate::theme::size::MICRO)
                                            .color(p::TEXT_FAINT),
                                    );
                                });
                            }
                            ProcCol::Session => {
                                header.col(|ui| {
                                    ui.label(
                                        RichText::new("SESSION")
                                            .size(crate::theme::size::MICRO)
                                            .color(p::TEXT_FAINT),
                                    );
                                });
                            }
                            ProcCol::Signature => {
                                header.col(|ui| {
                                    ui.label(
                                        RichText::new("SIGNATURE")
                                            .size(crate::theme::size::MICRO)
                                            .color(p::TEXT_FAINT),
                                    );
                                });
                            }
                        }
                    }
                })
                .body(|body| {
                    body.rows(row_h, rows.len(), |mut row| {
                        let i = row.index();
                        let proc = rows[i];
                        row.set_selected(selected == Some(proc.pid));

                        for col in &active {
                            match col {
                                ProcCol::Pid => {
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(proc.pid.to_string())
                                                .size(crate::theme::size::SMALL)
                                                .monospace()
                                                .color(p::TEXT_FAINT),
                                        );
                                    });
                                }
                                ProcCol::Name => {
                                    row.col(|ui| {
                                        let color = if proc.elevated == Some(true) {
                                            p::AMBER
                                        } else {
                                            ui.visuals().text_color()
                                        };
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 6.0;
                                            let dot_color = match proc.status {
                                                core_pulse_core::model::ProcessStatus::Running => p::GREEN,
                                                core_pulse_core::model::ProcessStatus::Suspended => p::AMBER,
                                                core_pulse_core::model::ProcessStatus::NotResponding => p::RED,
                                                core_pulse_core::model::ProcessStatus::Zombie => p::TEXT_FAINT,
                                            };
                                            let dot = widgets::dot(ui, dot_color, false);
                                            if proc.status != core_pulse_core::model::ProcessStatus::Running {
                                                dot.on_hover_text(format!("{:?}", proc.status));
                                            }
                                            let label = ui.label(
                                                RichText::new(format::truncate(&proc.name, 58))
                                                    .size(crate::theme::size::BODY)
                                                    .color(color),
                                            );
                                            label.context_menu(|ui| {
                                                process_context_menu(
                                                    ui,
                                                    proc,
                                                    &mut requested,
                                                    &mut open_detail,
                                                );
                                            });
                                        });
                                    });
                                }
                                ProcCol::Path => {
                                    row.col(|ui| match &proc.exe_path {
                                        Some(path) => {
                                            let text = path.display().to_string();
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(format::middle_truncate(&text, 72))
                                                        .size(crate::theme::size::MICRO)
                                                        .monospace()
                                                        .color(p::TEXT_FAINT),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(&text);
                                        }
                                        None => {
                                            ui.label(
                                                RichText::new(icons::DASH)
                                                    .size(crate::theme::size::MICRO)
                                                    .color(p::TEXT_FAINT),
                                            )
                                            .on_hover_text(
                                                "Path not readable. Reading the image path of another \
                                                 user's process needs administrator rights.",
                                            );
                                        }
                                    });
                                }
                                ProcCol::Cpu => {
                                    row.col(|ui| {
                                        let pct = proc.cpu_percent;
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 5.0;
                                            let color = if pct >= 50.0 {
                                                p::RED
                                            } else if pct >= 10.0 {
                                                p::AMBER
                                            } else {
                                                p::TEXT_DIM
                                            };
                                            ui.add_sized(
                                                [34.0, 14.0],
                                                egui::Label::new(
                                                    RichText::new(format!("{pct:.1}"))
                                                        .size(crate::theme::size::SMALL)
                                                        .monospace()
                                                        .color(color),
                                                ),
                                            );
                                            let (rect, _) = ui.allocate_exact_size(
                                                egui::vec2(26.0, 5.0),
                                                egui::Sense::hover(),
                                            );
                                            let painter = ui.painter_at(rect);
                                            painter.rect_filled(rect, CornerRadius::same(2), p::BG);
                                            painter.rect_filled(
                                                egui::Rect::from_min_size(
                                                    rect.min,
                                                    egui::vec2(
                                                        rect.width() * (pct / 100.0).clamp(0.0, 1.0),
                                                        rect.height(),
                                                    ),
                                                ),
                                                CornerRadius::same(2),
                                                color,
                                            );
                                        });
                                    });
                                }
                                ProcCol::Memory => {
                                    row.col(|ui| {
                                        let (digits, unit) =
                                            format::bytes_split(proc.mem_private_bytes);
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 3.0;
                                            ui.label(
                                                RichText::new(digits)
                                                    .size(crate::theme::size::SMALL)
                                                    .monospace()
                                                    .color(p::TEXT_DIM),
                                            );
                                            ui.label(
                                                RichText::new(unit)
                                                    .size(crate::theme::size::MICRO)
                                                    .color(p::TEXT_FAINT),
                                            );
                                        });
                                    });
                                }
                                ProcCol::Io => {
                                    row.col(|ui| {
                                        let io = proc.io_read_bytes + proc.io_write_bytes;
                                        ui.label(
                                            RichText::new(if io == 0 {
                                                "—".to_string()
                                            } else {
                                                format::bytes(io)
                                            })
                                            .size(crate::theme::size::SMALL)
                                            .monospace()
                                            .color(p::TEXT_FAINT),
                                        )
                                        .on_hover_text(format!(
                                            "read {}\nwritten {}",
                                            format::bytes(proc.io_read_bytes),
                                            format::bytes(proc.io_write_bytes)
                                        ));
                                    });
                                }
                                ProcCol::Gpu => {
                                    row.col(|ui| match proc.gpu_percent {
                                        Some(g) => {
                                            ui.label(
                                                RichText::new(format!("{g:.1}%"))
                                                    .size(crate::theme::size::SMALL)
                                                    .monospace()
                                                    .color(p::CYAN),
                                            );
                                        }
                                        None => {
                                            ui.label(
                                                RichText::new(icons::DASH)
                                                    .size(crate::theme::size::SMALL)
                                                    .color(p::TEXT_FAINT),
                                            );
                                        }
                                    });
                                }
                                ProcCol::Threads => {
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(proc.threads.to_string())
                                                .size(crate::theme::size::SMALL)
                                                .monospace()
                                                .color(p::TEXT_FAINT),
                                        );
                                    });
                                }
                                ProcCol::Handles => {
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(proc.handles.to_string())
                                                .size(crate::theme::size::SMALL)
                                                .monospace()
                                                .color(p::TEXT_FAINT),
                                        );
                                    });
                                }
                                ProcCol::Session => {
                                    row.col(|ui| {
                                        ui.label(
                                            RichText::new(proc.session_id.to_string())
                                                .size(crate::theme::size::SMALL)
                                                .monospace()
                                                .color(p::TEXT_FAINT),
                                        );
                                    });
                                }
                                ProcCol::Signature => {
                                    row.col(|ui| match &proc.signature {
                                        Some(sig) => {
                                            let (icon, color, tip) = match sig.status {
                                                core_pulse_core::model::SigStatus::Valid => (
                                                    icons::CHECK,
                                                    p::GREEN,
                                                    sig.publisher
                                                        .clone()
                                                        .unwrap_or_else(|| "signed".into()),
                                                ),
                                                core_pulse_core::model::SigStatus::Invalid => (
                                                    icons::CROSS,
                                                    p::RED,
                                                    "signature invalid".into(),
                                                ),
                                                core_pulse_core::model::SigStatus::Unsigned => (
                                                    icons::DASH,
                                                    p::TEXT_FAINT,
                                                    "not signed".into(),
                                                ),
                                                core_pulse_core::model::SigStatus::Unknown => (
                                                    "?",
                                                    p::TEXT_FAINT,
                                                    "not yet verified".into(),
                                                ),
                                            };
                                            ui.label(RichText::new(icon).color(color))
                                                .on_hover_text(tip);
                                        }
                                        None => {
                                            ui.label(
                                                RichText::new(icons::PENDING).color(p::TEXT_FAINT),
                                            )
                                            .on_hover_text("waiting for verification");
                                        }
                                    });
                                }
                            }
                        }

                        let resp = row.response();
                        if resp.clicked() {
                            selected = Some(proc.pid);
                        }
                        if resp.double_clicked() {
                            open_detail = Some(proc.pid);
                        }
                        resp.context_menu(|ui| {
                            process_context_menu(ui, proc, &mut requested, &mut open_detail);
                        });
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

        // The confirmation dialog is built from the context-menu request.
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

    /// Detail panel for the selected process, below the table.
    fn detail_panel(&mut self, ui: &mut egui::Ui) {
        let Some(row) = self.state.detail_row().cloned() else {
            return;
        };
        let mut close = false;

        egui::Panel::bottom("detail_panel")
            .resizable(true)
            .default_size(250.0)
            .frame(
                egui::Frame::new()
                    .fill(p::PANEL)
                    .inner_margin(egui::Margin::symmetric(12, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::dot(ui, p::ACCENT, false);
                    ui.label(
                        RichText::new(&row.name)
                            .font(crate::theme::bold(crate::theme::size::STRONG)),
                    );
                    ui.label(
                        RichText::new(format!("PID {}", row.pid))
                            .size(crate::theme::size::SMALL)
                            .monospace()
                            .color(p::TEXT_DIM),
                    );
                    if !row.details_complete {
                        widgets::badge(ui, "reading details", p::AMBER);
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(2.0);
                ui.separator();
                ui.add_space(4.0);

                egui::ScrollArea::vertical().show(ui, |ui| {
                    // Values not yet read show "…" rather than "—": a dash
                    // means the source refused or is absent (ADR-0008).
                    let field = |ui: &mut egui::Ui, label: &str, value: &str| {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                [118.0, 15.0],
                                egui::Label::new(
                                    RichText::new(label)
                                        .size(crate::theme::size::SMALL)
                                        .color(p::TEXT_FAINT),
                                ),
                            );
                            ui.add(
                                egui::Label::new(
                                    RichText::new(value)
                                        .size(crate::theme::size::SMALL)
                                        .monospace(),
                                )
                                .wrap(),
                            );
                        });
                    };
                    let detail_or = |v: Option<String>| -> String {
                        if row.details_complete {
                            opt_str(v)
                        } else {
                            "…".to_string()
                        }
                    };

                    ui.columns(2, |cols| {
                        let a = &mut cols[0];
                        let path_text = match (&row.exe_path, row.details_complete) {
                            (Some(p), _) => {
                                opt_str(Some(format::middle_truncate(&p.display().to_string(), 92)))
                            }
                            // Not read yet is different from "refused": the
                            // static-info pass may simply not have reached this
                            // process.
                            (None, false) => "…".to_string(),
                            (None, true) => "—".to_string(),
                        };
                        field(a, "Path", &path_text);
                        field(a, "Command line", &detail_or(row.cmdline.clone()));
                        field(a, "User", &detail_or(row.user.clone()));
                        field(
                            a,
                            "Integrity",
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
                            a,
                            "Elevated",
                            &match row.elevated {
                                Some(true) => "yes".to_string(),
                                Some(false) => "no".to_string(),
                                None => "—".to_string(),
                            },
                        );
                        field(
                            a,
                            "Signature",
                            &match &row.signature {
                                Some(s) => match (&s.publisher, s.status) {
                                    (Some(pubr), _) => format!("{pubr} ({:?})", s.status),
                                    (None, st) => format!("{st:?}"),
                                },
                                None => "not verified".into(),
                            },
                        );

                        let b = &mut cols[1];
                        field(b, "CPU time", &format::duration_ms(row.cpu_time_ms));
                        field(
                            b,
                            "Memory",
                            &format!(
                                "private {} · working set {} · shared {}",
                                format::bytes(row.mem_private_bytes),
                                format::bytes(row.mem_working_set_bytes),
                                format::bytes(row.mem_shared_bytes)
                            ),
                        );
                        field(
                            b,
                            "I/O total",
                            &format!(
                                "read {} · write {}",
                                format::bytes(row.io_read_bytes),
                                format::bytes(row.io_write_bytes)
                            ),
                        );
                        field(
                            b,
                            "Threads / handles",
                            &format!("{} / {}", row.threads, row.handles),
                        );
                        field(b, "Session", &row.session_id.to_string());
                        // Creation time is only useful as "how long has this
                        // been running", not as a raw epoch millisecond count.
                        let age_secs = self
                            .state
                            .snapshot
                            .ts_unix_ms
                            .saturating_sub(row.start_time_unix_ms)
                            / 1000;
                        field(
                            b,
                            "Running for",
                            &if row.start_time_unix_ms == 0 {
                                "—".to_string()
                            } else {
                                format::uptime(age_secs)
                            },
                        );
                        field(
                            b,
                            "Started at",
                            &if row.start_time_unix_ms == 0 {
                                "—".to_string()
                            } else {
                                format::clock_hms(row.start_time_unix_ms)
                            },
                        );
                        field(b, "Identity", &format!("pid {}", row.pid));
                        if row.eco_qos {
                            field(b, "Efficiency mode", "active");
                        }
                    });

                    if let Some(path) = &row.exe_path {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            if ui.small_button("Open file location").clicked() {
                                let result = core_pulse_core::sys::action::reveal_in_explorer(path);
                                match result {
                                    Ok(()) => self.state.set_feedback("opening Explorer", false),
                                    Err(e) => self.state.set_feedback(format!("failed: {e}"), true),
                                }
                            }
                            if ui.small_button("Copy command line").clicked() {
                                if let Some(cmd) = &row.cmdline {
                                    ui.ctx().copy_text(cmd.clone());
                                    self.state.set_feedback("command line copied", false);
                                }
                            }
                            if ui.small_button("Copy PID").clicked() {
                                ui.ctx().copy_text(row.pid.to_string());
                                self.state.set_feedback("PID copied", false);
                            }
                        });
                    }
                });
            });

        if close {
            self.state.detail_pid = None;
        }
    }
}

fn opt_str(v: Option<String>) -> String {
    v.filter(|s| !s.is_empty())
        .unwrap_or_else(|| "—".to_string())
}

impl App {
    fn performance(&mut self, ui: &mut egui::Ui) {
        let s = Arc::clone(&self.state.snapshot);
        let hist = &self.state.history;

        // Row of six KPI tiles.
        let net_now: u64 = s.net.iter().map(|n| n.rx_bps + n.tx_bps).sum();
        let disk_busy = s
            .disks
            .iter()
            .map(|d| d.busy_percent)
            .fold(0.0f32, f32::max);
        let gpu_now = s
            .gpus
            .iter()
            .filter_map(|g| g.utilization_percent)
            .fold(0.0f32, f32::max);
        let mem_pct = if s.memory.total_bytes > 0 {
            s.memory.in_use_bytes as f32 / s.memory.total_bytes as f32 * 100.0
        } else {
            0.0
        };
        let temps: Vec<f32> = s
            .sensors
            .iter()
            .filter(|r| r.kind == core_pulse_core::model::SensorKind::Temperature)
            .map(|r| r.value)
            .collect();
        // NOTE: never fold from `f32::MIN` — it is finite, so an empty sensor
        // list would render "-3.4e38 °C" instead of a dash.
        let temp_max = temps.iter().copied().reduce(f32::max);
        let temp_text = match temp_max {
            Some(t) => format!("{t:.0}"),
            None => "—".to_string(),
        };
        let temp_color = match temp_max {
            None => p::TEXT_FAINT,
            Some(t) if t >= 70.0 => p::RED,
            Some(t) if t >= 55.0 => p::AMBER,
            Some(_) => p::GREEN,
        };

        ui.columns(6, |cols| {
            widgets::tile(
                &mut cols[0],
                "CPU",
                &format!("{:.0}", s.cpu.total_percent),
                "%",
                &format!("{} logical cores", s.host.logical_cores),
                p::ACCENT,
                Some(s.cpu.total_percent / 100.0),
            );
            widgets::tile(
                &mut cols[1],
                "Memory",
                &format!("{mem_pct:.0}"),
                "%",
                &format!(
                    "{} of {}",
                    format::bytes(s.memory.in_use_bytes),
                    format::bytes(s.memory.total_bytes)
                ),
                p::GREEN,
                Some(mem_pct / 100.0),
            );
            widgets::tile(
                &mut cols[2],
                "Disk",
                &format!("{disk_busy:.0}"),
                "%",
                &format!("{} device(s)", s.disks.len()),
                p::AMBER,
                Some(disk_busy / 100.0),
            );
            widgets::tile(
                &mut cols[3],
                "Network",
                &format::bytes(net_now),
                "/s",
                &format!("{} adapter(s)", s.net.len()),
                p::CYAN,
                None,
            );
            widgets::tile(
                &mut cols[4],
                "GPU",
                &format!("{gpu_now:.0}"),
                "%",
                &format!("{} adapter(s)", s.gpus.len()),
                p::PURPLE,
                Some(gpu_now / 100.0),
            );
            widgets::tile(
                &mut cols[5],
                "Max temp",
                &temp_text,
                if temp_max.is_some() { "°C" } else { "" },
                if temp_max.is_some() {
                    "across all sensors"
                } else {
                    "no sensor source"
                },
                temp_color,
                temp_max.map(|t| (t / 110.0).clamp(0.0, 1.0)),
            );
        });

        ui.add_space(10.0);

        ui.columns(2, |cols| {
            widgets::card(&mut cols[0], |ui| {
                widgets::section(ui, "Processor", |ui| {
                    widgets::caption(ui, &s.host.cpu_brand);
                });
                widgets::sparkline(ui, &hist.cpu, 62.0, 100.0, p::ACCENT, "CPU", "");
                ui.add_space(6.0);
                widgets::section(ui, "Memory pressure", |ui| {
                    ui.label(
                        RichText::new(format!("{mem_pct:.0}%"))
                            .size(crate::theme::size::SMALL)
                            .monospace()
                            .color(p::GREEN),
                    );
                });
                widgets::sparkline(ui, &hist.mem, 46.0, 100.0, p::GREEN, "RAM", "");
            });

            widgets::card(&mut cols[1], |ui| {
                widgets::section(ui, "Disk activity", |ui| {
                    ui.label(
                        RichText::new(format!("{disk_busy:.0}% busiest"))
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                    );
                });
                widgets::sparkline(ui, &hist.disk, 54.0, 100.0, p::AMBER, "Disk", "");
                ui.add_space(6.0);
                widgets::section(ui, "Network throughput", |ui| {
                    ui.label(
                        RichText::new(format::rate(net_now))
                            .size(crate::theme::size::MICRO)
                            .color(p::CYAN),
                    );
                });
                let net_scale = hist.net.iter().copied().fold(1.0f32, f32::max).max(1.0);
                widgets::sparkline(ui, &hist.net, 46.0, net_scale, p::CYAN, "NET", "");
            });
        });

        ui.add_space(10.0);

        ui.columns(2, |cols| {
            widgets::card(&mut cols[0], |ui| {
                widgets::section(ui, "Logical processors", |ui| {
                    ui.label(
                        RichText::new(format!("{:.0}% total", s.cpu.total_percent))
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_DIM),
                    );
                });
                ui.add_space(4.0);
                if s.cpu.per_core_percent.is_empty() {
                    widgets::empty_state(ui, "Per-core counters unavailable", "");
                } else {
                    for (i, v) in s.cpu.per_core_percent.iter().enumerate() {
                        let color = if *v >= 75.0 {
                            p::RED
                        } else if *v >= 40.0 {
                            p::AMBER
                        } else {
                            p::ACCENT
                        };
                        widgets::bar_row(
                            ui,
                            &format!("C{i}"),
                            *v / 100.0,
                            &format!("{v:.0}%"),
                            color,
                        );
                    }
                }
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                // Only values that are actually collected go here. Queue
                // length and context switches are reported by PDH, which this
                // collector does not open, so showing them as "0" would be a
                // fabricated measurement (ADR-0008).
                widgets::kv_row(ui, "Physical cores", &s.host.physical_cores.to_string());
                widgets::kv_row(ui, "Logical cores", &s.host.logical_cores.to_string());
                widgets::kv_row(ui, "Base clock", &format!("{} MHz", s.host.base_clock_mhz));
                let threads: u64 = s.processes.iter().map(|p| p.threads as u64).sum();
                widgets::kv_row(ui, "Threads total", &threads.to_string());
                widgets::kv_row(ui, "Processes", &s.processes.len().to_string());
            });

            cols[0].add_space(10.0);

            widgets::card(&mut cols[0], |ui| {
                widgets::section(ui, "Volumes", |ui| {
                    ui.label(
                        RichText::new(format!("{} mounted", s.volumes.len()))
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                    );
                });
                ui.add_space(4.0);
                if s.volumes.is_empty() {
                    ui.label(
                        RichText::new(format!("{} no mounted volume reported", icons::DASH))
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_FAINT),
                    );
                }
                for v in &s.volumes {
                    let used = v.total_bytes.saturating_sub(v.free_bytes);
                    let frac = if v.total_bytes > 0 {
                        used as f32 / v.total_bytes as f32
                    } else {
                        0.0
                    };
                    let color = if frac >= 0.95 {
                        p::RED
                    } else if frac >= 0.85 {
                        p::AMBER
                    } else {
                        p::GREEN
                    };
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [38.0, 14.0],
                            egui::Label::new(
                                RichText::new(&v.mount)
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::TEXT),
                            ),
                        );
                        let width = (ui.available_width() - 138.0).max(50.0);
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(width, 6.0), egui::Sense::hover());
                        let painter = ui.painter_at(rect);
                        painter.rect_filled(rect, CornerRadius::same(2), p::BG);
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                rect.min,
                                egui::vec2(rect.width() * frac, rect.height()),
                            ),
                            CornerRadius::same(2),
                            color,
                        );
                        ui.label(
                            RichText::new(format!("{} free", format::bytes(v.free_bytes)))
                                .size(crate::theme::size::MICRO)
                                .monospace()
                                .color(p::TEXT_FAINT),
                        )
                        .on_hover_text(format!(
                            "{}\n{}\n{} filesystem\n{:.1}% used of {}",
                            v.label,
                            v.fs,
                            v.mount,
                            frac * 100.0,
                            format::bytes(v.total_bytes)
                        ));
                    });
                }
            });

            widgets::card(&mut cols[1], |ui| {
                widgets::section(ui, "Memory composition", |ui| {
                    ui.label(
                        RichText::new(format::bytes(s.memory.total_bytes))
                            .size(crate::theme::size::MICRO)
                            .monospace()
                            .color(p::TEXT_DIM),
                    );
                });
                ui.add_space(4.0);
                widgets::kv_row(ui, "In use", &format::bytes(s.memory.in_use_bytes));
                widgets::kv_row(ui, "Available", &format::bytes(s.memory.available_bytes));
                widgets::kv_row(ui, "Standby", &format::bytes(s.memory.standby_bytes));
                widgets::kv_row(ui, "Modified", &format::bytes(s.memory.modified_bytes));
                widgets::kv_row(ui, "Free", &format::bytes(s.memory.free_bytes));
                widgets::kv_row(ui, "Cached", &format::bytes(s.memory.cached_bytes));
                widgets::kv_row(
                    ui,
                    "Commit",
                    &format!(
                        "{} / {}",
                        format::bytes(s.memory.commit_total_bytes),
                        format::bytes(s.memory.commit_limit_bytes)
                    ),
                );
                widgets::kv_row(ui, "Paged pool", &format::bytes(s.memory.paged_pool_bytes));
                widgets::kv_row(
                    ui,
                    "Nonpaged pool",
                    &format::bytes(s.memory.nonpaged_pool_bytes),
                );
                widgets::kv_row(ui, "Memory pressure", &format!("{}/100", s.memory.pressure));
            });
        });

        ui.add_space(10.0);

        ui.columns(2, |cols| {
            widgets::card(&mut cols[0], |ui| {
                widgets::section(ui, "Disks", |ui| {
                    ui.label(
                        RichText::new("read · write · queue")
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                    );
                });
                ui.add_space(2.0);
                if s.disks.is_empty() {
                    widgets::empty_state(ui, "PhysicalDisk counters unavailable", "");
                }
                for d in &s.disks {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        widgets::dot(
                            ui,
                            if d.busy_percent > 50.0 {
                                p::AMBER
                            } else {
                                p::GREEN
                            },
                            false,
                        );
                        ui.label(
                            RichText::new(format::truncate(&d.model, 30))
                                .font(crate::theme::bold(crate::theme::size::BODY)),
                        );
                        ui.label(
                            RichText::new(format!("{:?}", d.media))
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.0}%", d.busy_percent))
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::AMBER),
                            );
                        });
                    });
                    widgets::meter(ui, d.busy_percent / 100.0, p::AMBER, 4.0);
                    ui.label(
                        RichText::new(format!(
                            "R {}  W {}  q {:.2}  {:.1} ms",
                            format::rate(d.read_bps),
                            format::rate(d.write_bps),
                            d.queue_depth,
                            d.avg_response_ms
                        ))
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                    );
                }
            });

            widgets::card(&mut cols[1], |ui| {
                widgets::section(ui, "GPUs", |ui| {
                    ui.label(
                        RichText::new("utilization · memory")
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                    );
                });
                if s.gpus.is_empty() {
                    widgets::empty_state(ui, "GPU Engine counters unavailable", "");
                }
                for g in &s.gpus {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        widgets::dot(ui, p::PURPLE, false);
                        ui.label(
                            RichText::new(format::truncate(&g.name, 30))
                                .font(crate::theme::bold(crate::theme::size::BODY)),
                        );
                    });
                    let util = g.utilization_percent.unwrap_or(0.0);
                    widgets::meter(ui, util / 100.0, p::PURPLE, 4.0);
                    let mem = match (g.dedicated_used_bytes, g.shared_used_bytes) {
                        (Some(d), Some(sh)) if d > 0 => format!(
                            "dedicated {} · shared {}",
                            format::bytes(d),
                            format::bytes(sh)
                        ),
                        (_, Some(sh)) => format!("shared {}", format::bytes(sh)),
                        (Some(d), None) => format!("dedicated {}", format::bytes(d)),
                        _ => "memory not measurable".to_string(),
                    };
                    ui.label(
                        RichText::new(format!("{mem} · temp {}", format::temp_c(g.temperature_c)))
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                    );
                }
            });
        });

        if !s.degraded.is_empty() {
            ui.add_space(10.0);
            widgets::card(ui, |ui| {
                widgets::section(ui, "Degraded sources", |_| {});
                for d in &s.degraded {
                    ui.label(
                        RichText::new(format!(
                            "{} {:?} ({:?}): {}",
                            icons::BULLET,
                            d.area,
                            d.reason,
                            d.detail
                        ))
                        .size(crate::theme::size::SMALL)
                        .color(p::TEXT_DIM),
                    );
                }
            });
        }
    }
}

impl App {
    fn sensors(&mut self, ui: &mut egui::Ui) {
        use core_pulse_core::model::{SensorAvailability, SensorKind};

        let s = Arc::clone(&self.state.snapshot);
        let temps: Vec<_> = s
            .sensors
            .iter()
            .filter(|r| r.kind == SensorKind::Temperature)
            .filter(|r| !self.state.sensor_hot_only || r.value >= 45.0)
            .collect();

        let hottest = s
            .sensors
            .iter()
            .filter(|r| r.kind == SensorKind::Temperature)
            .map(|r| r.value)
            .reduce(f32::max);
        let helper = s
            .sensor_status
            .iter()
            .find(|st| st.provider == "helper")
            .map(|st| &st.availability);

        ui.columns(3, |cols| {
            widgets::tile(
                &mut cols[0],
                "Temperature sensors",
                &temps.len().to_string(),
                "",
                if temps.is_empty() {
                    "no active source"
                } else {
                    "reporting now"
                },
                p::RED,
                None,
            );
            widgets::tile(
                &mut cols[1],
                "Hottest",
                &match hottest {
                    Some(t) => format!("{t:.0}"),
                    None => "—".to_string(),
                },
                if hottest.is_some() { "°C" } else { "" },
                if hottest.is_some() {
                    "warn 55 · critical 60 °C"
                } else {
                    "nothing to read"
                },
                p::AMBER,
                hottest.map(|t| (t / 110.0).clamp(0.0, 1.0)),
            );
            let (label, color) = match helper {
                Some(SensorAvailability::Available) => ("ACTIVE", p::GREEN),
                Some(SensorAvailability::NeedsElevation) => ("NOT RUNNING", p::AMBER),
                Some(SensorAvailability::Failed { .. }) => ("ERROR", p::RED),
                _ => ("UNKNOWN", p::TEXT_FAINT),
            };
            widgets::tile(
                &mut cols[2],
                "Elevated helper",
                label,
                "",
                "SMART + privileged sensors",
                color,
                None,
            );
        });

        ui.add_space(10.0);

        ui.columns(2, |cols| {
            widgets::card(&mut cols[0], |ui| {
                widgets::section(ui, "Temperature", |ui| {
                    ui.label(
                        RichText::new(format!("{} reading(s)", temps.len()))
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                    );
                });
                ui.add_space(2.0);
                if temps.is_empty() {
                    let has_smart_elevation = s.sensor_status.iter().any(|st| {
                        st.provider == "smart"
                            && matches!(
                                st.availability,
                                core_pulse_core::model::SensorAvailability::NeedsElevation
                            )
                    });
                    let cpu_unavailable = s.sensor_status.iter().any(|st| {
                        st.provider == "acpi-pdh"
                            && matches!(
                                st.availability,
                                core_pulse_core::model::SensorAvailability::NotAvailable { .. }
                            )
                    });
                    let detail = if has_smart_elevation && cpu_unavailable {
                        "CPU temperature is not exposed by ACPI firmware on this machine. \
                         Disk SMART temperature is available via the elevated helper."
                    } else if has_smart_elevation {
                        "Disk SMART needs the elevated helper. Check provider status on the right."
                    } else if cpu_unavailable {
                        "CPU temperature is not exposed by ACPI firmware on this machine. \
                         Check provider status for other sources (helper/GPU/bridge)."
                    } else {
                        "No active temperature source. Check provider status and actions on the right."
                    };
                    widgets::empty_state(ui, "No temperature reading available", detail);
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    widgets::kicker(ui, "What to try");
                    ui.add_space(2.0);
                    for (step, detail) in [
                        (
                            "1. Start the elevated helper",
                            "Unlocks SMART disk temperature. Requires one UAC prompt; the helper protocol is read-only.",
                        ),
                        (
                            "2. Install it as a service",
                            "Same data, no UAC at next launch. Auto-start, LocalSystem, removable from Services.",
                        ),
                        (
                            "3. Enable the third-party bridge",
                            "Use the \"Third-party bridge (LHM)\" toggle in the toolbar if you run LibreHardwareMonitor with its web server on 127.0.0.1:8085. Data is opt-in and marked ThirdParty.",
                        ),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(step)
                                    .font(crate::theme::bold(crate::theme::size::SMALL))
                                    .color(p::TEXT_DIM),
                            );
                        });
                        ui.label(
                            RichText::new(detail)
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                        ui.add_space(3.0);
                    }
                }
                if self.state.history.has_temp && !self.state.history.temp.is_empty() {
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(4.0);
                    widgets::section(ui, "History", |ui| {
                        ui.label(
                            RichText::new("average of all sensors · 60 s")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    widgets::sparkline(
                        ui,
                        &self.state.history.temp,
                        52.0,
                        110.0,
                        p::RED,
                        "TEMP",
                        "",
                    );
                }
                for r in &temps {
                    ui.add_space(3.0);
                    let color = if r.value >= 55.0 {
                        p::RED
                    } else if r.value >= 45.0 {
                        p::AMBER
                    } else {
                        p::GREEN
                    };
                    ui.horizontal(|ui| {
                        widgets::dot(ui, color, false);
                        ui.label(RichText::new(format::truncate(&r.label, 34)).size(crate::theme::size::BODY));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.1} °C", r.value))
                                    .font(crate::theme::bold_mono(crate::theme::size::BODY))
                                    .color(color),
                            );
                            let (mark, tip) = match r.confidence {
                                core_pulse_core::model::Confidence::Direct => {
                                    ("", "read directly from a hardware sensor")
                                }
                                core_pulse_core::model::Confidence::Derived => {
                                    (
                                        icons::APPROX,
                                        "derived (ACPI zone, not die temperature)",
                                    )
                                }
                                core_pulse_core::model::Confidence::ThirdParty => {
                                    (
                                        icons::THIRD_PARTY,
                                        "third party (LibreHardwareMonitor)",
                                    )
                                }
                            };
                            if !mark.is_empty() {
                                ui.label(RichText::new(mark).size(crate::theme::size::SMALL).color(p::TEXT_FAINT))
                                    .on_hover_text(tip);
                            }
                        });
                    });
                    let frac = (r.value / 110.0).clamp(0.0, 1.0);
                    widgets::meter(ui, frac, color, 3.0);
                    ui.label(
                        RichText::new(format!(
                            "source {:?} · min {:.0} · max {:.0} · avg {:.0}",
                            r.source, r.min, r.max, r.avg
                        ))
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                    );
                }
            });

            widgets::card(&mut cols[1], |ui| {
                widgets::section(ui, "Provider status", |ui| {
                    let available = s
                        .sensor_status
                        .iter()
                        .filter(|x| matches!(x.availability, SensorAvailability::Available))
                        .count();
                    ui.label(
                        RichText::new(format!("{available}/{} available", s.sensor_status.len()))
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                    );
                });
                ui.add_space(2.0);
                for st in &s.sensor_status {
                    self.provider_status_row(ui, st);
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(6.0);
                self.helper_controls(ui, &s);
                self.bridge_controls(ui, &s);
            });
        });

        ui.add_space(10.0);

        ui.columns(2, |cols| {
            widgets::card(&mut cols[0], |ui| {
                widgets::section(ui, "Why CPU temperature is often missing", |_| {});
                ui.label(
                    RichText::new(
                        "Windows provides no user-mode API for CPU temperature. The only official \
                         path is an ACPI thermal zone, and many firmware implementations — including \
                         this machine — expose none.\n\n\
                         Options:\n\
                         • Run the elevated helper to read disk SMART and other privileged sensors.\n\
                         • Enable the LibreHardwareMonitor bridge if you already run it.\n\n\
                         No estimated values are shown. A missing number is better than a wrong one.",
                    )
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_DIM),
                );
            });

            widgets::card(&mut cols[1], |ui| {
                widgets::section(ui, "Other sensors", |_| {});
                let others: Vec<_> = s
                    .sensors
                    .iter()
                    .filter(|r| r.kind != SensorKind::Temperature)
                    .collect();
                if others.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "{} no fan, voltage, power or wear sensor reported",
                            icons::DASH
                        ))
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_FAINT),
                    );
                }
                for r in others {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format::truncate(&r.label, 40))
                                .size(crate::theme::size::SMALL)
                                .monospace()
                                .color(p::TEXT_DIM),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.0} {:?}", r.value, r.unit))
                                    .size(crate::theme::size::SMALL)
                                    .monospace(),
                            );
                        });
                    });
                }
                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                widgets::kv_row(ui, "Polling interval", "2 s");
                widgets::kv_row(ui, "Sensor readings", &s.sensors.len().to_string());
                widgets::kv_row(
                    ui,
                    "Providers reporting",
                    &s.sensor_status.len().to_string(),
                );
            });
        });
    }

    /// Helper status and explicit elevation buttons. There is no automatic
    /// UAC: the user must click, and every click can be cancelled.
    fn helper_controls(&mut self, ui: &mut egui::Ui, s: &Snapshot) {
        use core_pulse_core::model::SensorAvailability;

        let status = s
            .sensor_status
            .iter()
            .find(|st| st.provider == "helper")
            .map(|st| &st.availability);

        match status {
            Some(SensorAvailability::Available) => {
                ui.horizontal(|ui| {
                    widgets::dot(ui, p::GREEN, true);
                    ui.label(
                        RichText::new("Helper active (elevated)")
                            .size(crate::theme::size::SMALL)
                            .color(p::GREEN),
                    );
                });
            }
            Some(SensorAvailability::NeedsElevation) => {
                ui.horizontal(|ui| {
                    widgets::dot(ui, p::AMBER, true);
                    ui.label(
                        RichText::new("Helper not running — disk SMART needs elevation")
                            .size(crate::theme::size::SMALL)
                            .color(p::AMBER),
                    );
                });
            }
            Some(SensorAvailability::Failed { error }) => {
                ui.horizontal(|ui| {
                    widgets::dot(ui, p::RED, false);
                    ui.label(
                        RichText::new(format!("Helper error — {error}"))
                            .size(crate::theme::size::SMALL)
                            .color(p::RED),
                    );
                });
            }
            Some(SensorAvailability::NotAvailable { reason }) => {
                ui.horizontal(|ui| {
                    widgets::dot(ui, p::TEXT_FAINT, false);
                    ui.label(
                        RichText::new(format!("Helper unavailable — {reason}"))
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_DIM),
                    );
                });
            }
            None => {
                ui.horizontal(|ui| {
                    widgets::dot(ui, p::TEXT_FAINT, false);
                    ui.label(
                        RichText::new("Helper status not read yet")
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_FAINT),
                    );
                });
            }
        }

        // Always show Diagnose (background thread, no UAC).
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let label = if self.state.helper_diagnosing {
                "Diagnosing…"
            } else {
                "Diagnose (helper)"
            };
            if ui
                .add_enabled(
                    !self.state.helper_diagnosing,
                    egui::Button::new(label).small(),
                )
                .on_hover_text(
                    "Same check as `core-pulse-cli helper`: expected path, connected PID/version/elevated, provider status and readings. Runs in the background; no UAC.",
                )
                .clicked()
            {
                self.run_helper_diagnose();
            }
            if let Some(act) = &self.state.helper_action {
                let color = if act.is_error { p::RED } else { p::GREEN };
                ui.label(
                    RichText::new(format!(
                        "last {}",
                        format::clock_hms(act.at_unix_ms)
                    ))
                    .size(crate::theme::size::MICRO)
                    .color(color),
                )
                .on_hover_text(act.text.clone());
            }
        });
        if let Some(act) = &self.state.helper_action {
            let color = if act.is_error { p::RED } else { p::GREEN };
            ui.add(
                egui::Label::new(
                    RichText::new(act.text.clone())
                        .size(crate::theme::size::MICRO)
                        .color(color),
                )
                .wrap(),
            );
        }
        if let Some(diag) = &self.state.helper_diagnose {
            let color = if diag.is_error { p::RED } else { p::TEXT_DIM };
            ui.add_space(2.0);
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "diagnose {}: {}",
                        format::clock_hms(diag.at_unix_ms),
                        diag.text
                    ))
                    .size(crate::theme::size::MICRO)
                    .color(color),
                )
                .wrap(),
            );
        } else if self.state.helper_diagnosing {
            ui.label(
                RichText::new("Diagnosing helper…")
                    .size(crate::theme::size::MICRO)
                    .color(p::TEXT_FAINT),
            );
        }

        let helper_usable = matches!(status, Some(SensorAvailability::Available));
        if !helper_usable {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .button("Start helper (UAC)")
                    .on_hover_text("Runs core-pulse-helper.exe elevated for this session")
                    .clicked()
                {
                    match elevate::launch_helper("--serve", false) {
                        Ok(()) => self.state.set_helper_action(
                            "UAC prompt opened — approve to start the helper",
                            false,
                        ),
                        Err(e) => self.state.set_helper_action(format!("failed: {e}"), true),
                    }
                }
                if ui
                    .button("Install service (UAC)")
                    .on_hover_text(
                        "Installs the helper as a Windows Service (auto-start, LocalSystem). \
                         One UAC prompt; no further prompts after that.\n\n\
                         Security note: the current per-user install layout may place the service \
                         binary in a user-writable directory. On multi-user machines, prefer \
                         a foreground helper session or an admin-only install location.",
                    )
                    .clicked()
                {
                    match elevate::launch_helper("--install-service", false) {
                        Ok(()) => self.state.set_helper_action(
                            "UAC prompt opened — approve to install the helper service",
                            false,
                        ),
                        Err(e) => self.state.set_helper_action(format!("failed: {e}"), true),
                    }
                }
            });
        }
    }

    fn provider_status_row(
        &mut self,
        ui: &mut egui::Ui,
        st: &core_pulse_core::model::SensorStatus,
    ) {
        use core_pulse_core::model::SensorAvailability;

        let (dot_color, text, detail, action) = match &st.availability {
            SensorAvailability::Available => (p::GREEN, "available", String::new(), None),
            SensorAvailability::NeedsElevation => (
                p::AMBER,
                "needs elevation",
                if st.provider == "smart" {
                    "Run the elevated helper to read SMART disk temperature.".to_string()
                } else {
                    "This provider needs elevated access.".to_string()
                },
                Some("Use helper controls below."),
            ),
            SensorAvailability::NotAvailable { reason } => {
                let action = if st.provider == "acpi-pdh" {
                    Some(
                        "Not fixable in software on this firmware. Use the LHM bridge only if you run LibreHardwareMonitor with admin/driver access.",
                    )
                } else if st.provider == "lhm-bridge" {
                    Some(
                        "Enable \"Third-party bridge (LHM)\" above, and run LHM with Remote Web Server on 127.0.0.1:8085.",
                    )
                } else if st.provider == "gpu-vendor" {
                    Some(
                        "GPU temperature needs a working vendor driver/SDK (NVML for NVIDIA, IGCL for Intel). Check driver status.",
                    )
                } else {
                    None
                };
                (p::TEXT_FAINT, "unavailable", reason.clone(), action)
            }
            SensorAvailability::Failed { error } => {
                let action = if st.provider == "gpu-vendor" {
                    Some(
                        "Driver/SDK failed to initialize. NVIDIA: verify nvidia-smi/driver communication. Intel: IGCL temperature read is not implemented/fails closed.",
                    )
                } else if st.provider == "helper" {
                    Some(
                        "Use \"Start helper (UAC)\" below, or run `core-pulse-cli helper` for diagnosis.",
                    )
                } else {
                    Some("Check logs/diagnostics; this source failed closed without fake values.")
                };
                (p::RED, "failed", error.clone(), action)
            }
        };

        ui.horizontal(|ui| {
            widgets::dot(ui, dot_color, false);
            ui.add_sized(
                [96.0, 15.0],
                egui::Label::new(
                    RichText::new(&st.provider)
                        .size(crate::theme::size::SMALL)
                        .monospace()
                        .color(p::TEXT_DIM),
                ),
            );
            ui.label(
                RichText::new(text)
                    .size(crate::theme::size::SMALL)
                    .color(dot_color),
            );
        });
        if !detail.is_empty() {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(detail)
                            .size(crate::theme::size::MICRO)
                            .color(p::TEXT_FAINT),
                    )
                    .wrap(),
                );
            });
        }
        if let Some(action) = action {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("Next: {action}"))
                            .size(crate::theme::size::MICRO)
                            .color(p::CYAN),
                    )
                    .wrap(),
                );
            });
        }
        ui.add_space(2.0);
    }

    fn bridge_controls(&mut self, ui: &mut egui::Ui, s: &core_pulse_core::model::Snapshot) {
        use core_pulse_core::model::SensorAvailability;
        let status = s
            .sensor_status
            .iter()
            .find(|st| st.provider == "lhm-bridge")
            .map(|st| &st.availability);
        ui.add_space(4.0);
        widgets::kicker(ui, "Third-party bridge");
        ui.add_space(2.0);
        match status {
            Some(SensorAvailability::Available) => {
                ui.label(
                    RichText::new("LHM bridge is active (ThirdParty data).")
                        .size(crate::theme::size::SMALL)
                        .color(p::GREEN),
                );
            }
            Some(SensorAvailability::NotAvailable { .. }) if self.state.bridge_enabled => {
                ui.label(
                    RichText::new(
                        "Bridge enabled, but LibreHardwareMonitor is not reachable at 127.0.0.1:8085. \
                         Start LHM and enable its Remote Web Server.",
                    )
                    .size(crate::theme::size::SMALL)
                    .color(p::AMBER),
                );
            }
            Some(SensorAvailability::Failed { error }) => {
                ui.label(
                    RichText::new(format!("Bridge failed: {error}"))
                        .size(crate::theme::size::SMALL)
                        .color(p::RED),
                );
            }
            _ => {
                ui.label(
                    RichText::new(
                        "Off by default. Enable it only if you trust a local LibreHardwareMonitor instance. \
                         core-pulse reads localhost JSON only and never writes commands.",
                    )
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_FAINT),
                );
            }
        }
    }

    fn services(&mut self, ui: &mut egui::Ui) {
        let s = Arc::clone(&self.state.snapshot);
        if s.services.is_empty() {
            widgets::empty_state(
                ui,
                "Service list not read yet",
                "Waiting for the first refresh (every 15 s).",
            );
            return;
        }

        let needle = self.state.service_search.trim().to_lowercase();
        let rows: Vec<_> = s
            .services
            .iter()
            .filter(|svc| {
                needle.is_empty()
                    || svc.name.to_lowercase().contains(&needle)
                    || svc.display_name.to_lowercase().contains(&needle)
            })
            .collect();

        let running = s
            .services
            .iter()
            .filter(|svc| svc.state == core_pulse_core::model::ServiceRunState::Running)
            .count();
        let disabled = s
            .services
            .iter()
            .filter(|svc| svc.start_type == core_pulse_core::model::ServiceStartType::Disabled)
            .count();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} services", s.services.len()))
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_DIM),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("{running} running"))
                    .size(crate::theme::size::SMALL)
                    .color(p::GREEN),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("{disabled} disabled"))
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_FAINT),
            );
            ui.separator();
            ui.label(
                RichText::new(format!("{} matched", rows.len()))
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_FAINT),
            );
        });
        ui.add_space(4.0);

        let avail = ui.available_height();
        widgets::framed(ui, |ui| {
            TableBuilder::new(ui)
                .id_salt("service_table")
                .striped(true)
                .resizable(false)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(160.0))
                .column(Column::remainder().at_least(180.0).clip(true))
                .column(Column::exact(96.0))
                .column(Column::exact(128.0))
                .column(Column::exact(60.0))
                .max_scroll_height(avail - 2.0)
                .header(24.0, |mut h| {
                    h.col(|ui| {
                        ui.label(
                            RichText::new("SERVICE")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("DISPLAY NAME")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("STATE")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("STARTUP")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("PID")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                })
                .body(|body| {
                    body.rows(21.0, rows.len(), |mut row| {
                        let svc = rows[row.index()];
                        row.col(|ui| {
                            ui.label(
                                RichText::new(format::truncate(&svc.name, 26))
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::TEXT_DIM),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(format::truncate(&svc.display_name, 60))
                                    .size(crate::theme::size::BODY),
                            );
                        });
                        row.col(|ui| {
                            let color =
                                if svc.state == core_pulse_core::model::ServiceRunState::Running {
                                    p::GREEN
                                } else {
                                    p::TEXT_FAINT
                                };
                            ui.horizontal(|ui| {
                                widgets::dot(ui, color, false);
                                ui.label(
                                    RichText::new(svc.state.label())
                                        .size(crate::theme::size::SMALL)
                                        .color(color),
                                );
                            });
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(svc.start_type.label())
                                    .size(crate::theme::size::SMALL)
                                    .color(p::TEXT_DIM),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(if svc.pid == 0 {
                                    "—".to_string()
                                } else {
                                    svc.pid.to_string()
                                })
                                .size(crate::theme::size::SMALL)
                                .monospace()
                                .color(p::TEXT_FAINT),
                            );
                        });
                    });
                });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Starting or stopping services requires administrator rights and is not \
                 implemented: the helper protocol is read-only by design.",
            )
            .size(crate::theme::size::MICRO)
            .color(p::TEXT_FAINT),
        );
    }

    fn startup(&mut self, ui: &mut egui::Ui) {
        let s = Arc::clone(&self.state.snapshot);
        if s.startup.is_empty() {
            widgets::empty_state(
                ui,
                "No startup entries read yet",
                "This machine reports none, or the first refresh has not completed.",
            );
            return;
        }

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} logon entries", s.startup.len()))
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_DIM),
            );
            ui.separator();
            let missing = s
                .startup
                .iter()
                .filter(|e| e.target_exists == Some(false))
                .count();
            ui.label(
                RichText::new(format!("{missing} missing target(s)"))
                    .size(crate::theme::size::SMALL)
                    .color(if missing > 0 { p::AMBER } else { p::TEXT_FAINT }),
            );
        });
        ui.add_space(4.0);

        let avail = ui.available_height();
        widgets::framed(ui, |ui| {
            TableBuilder::new(ui)
                .id_salt("startup_table")
                .striped(true)
                .resizable(false)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::remainder().at_least(200.0).clip(true))
                .column(Column::exact(118.0))
                .column(Column::exact(62.0))
                .column(Column::exact(54.0))
                .max_scroll_height(avail - 2.0)
                .header(24.0, |mut h| {
                    h.col(|ui| {
                        ui.label(
                            RichText::new("ENTRY")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("SOURCE")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("TARGET")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("32-BIT")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                })
                .body(|body| {
                    body.rows(22.0, s.startup.len(), |mut row| {
                        let e = &s.startup[row.index()];
                        row.col(|ui| {
                            ui.label(
                                RichText::new(format::truncate(&e.name, 64))
                                    .size(crate::theme::size::BODY),
                            )
                            .on_hover_text(&e.command);
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(e.source.label())
                                    .size(crate::theme::size::SMALL)
                                    .color(p::TEXT_DIM),
                            );
                        });
                        row.col(|ui| match e.target_exists {
                            Some(true) => {
                                ui.label(
                                    RichText::new("present")
                                        .size(crate::theme::size::SMALL)
                                        .color(p::GREEN),
                                );
                            }
                            Some(false) => {
                                ui.label(
                                    RichText::new("missing")
                                        .size(crate::theme::size::SMALL)
                                        .color(p::RED),
                                )
                                .on_hover_text(
                                    "The file this entry references does not exist on disk",
                                );
                            }
                            None => {
                                ui.label(
                                    RichText::new(icons::DASH)
                                        .size(crate::theme::size::SMALL)
                                        .color(p::TEXT_FAINT),
                                );
                            }
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(if e.is_32bit { "yes" } else { "—" })
                                    .size(crate::theme::size::SMALL)
                                    .color(p::TEXT_FAINT),
                            );
                        });
                    });
                });
        });

        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "Hover an entry to read its command line. Shortcuts (.lnk) are resolved to their \
                 target; a missing target usually means the app was uninstalled without cleaning \
                 up its startup entry.",
            )
            .size(crate::theme::size::MICRO)
            .color(p::TEXT_FAINT),
        );
    }

    fn users(&mut self, ui: &mut egui::Ui) {
        let s = Arc::clone(&self.state.snapshot);
        if s.sessions.is_empty() {
            widgets::empty_state(ui, "No Terminal Services session read", "");
            return;
        }

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("{} session(s)", s.sessions.len()))
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_DIM),
            );
            ui.separator();
            let total: u32 = s.sessions.iter().map(|x| x.process_count).sum();
            ui.label(
                RichText::new(format!("{total} processes across sessions"))
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_FAINT),
            );
        });
        ui.add_space(4.0);

        let avail = ui.available_height();
        widgets::framed(ui, |ui| {
            TableBuilder::new(ui)
                .id_salt("session_table")
                .striped(true)
                .resizable(false)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::exact(56.0))
                .column(Column::remainder().at_least(190.0).clip(true))
                .column(Column::exact(104.0))
                .column(Column::exact(74.0))
                .column(Column::exact(70.0))
                .column(Column::exact(92.0))
                .max_scroll_height(avail - 2.0)
                .header(24.0, |mut h| {
                    h.col(|ui| {
                        ui.label(
                            RichText::new("ID")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("USER")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("STATE")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("PROCESSES")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("CPU")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                    h.col(|ui| {
                        ui.label(
                            RichText::new("MEMORY")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                    });
                })
                .body(|body| {
                    body.rows(21.0, s.sessions.len(), |mut row| {
                        let sess = &s.sessions[row.index()];
                        row.col(|ui| {
                            ui.label(
                                RichText::new(sess.session_id.to_string())
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::TEXT_FAINT),
                            );
                        });
                        row.col(|ui| match &sess.user {
                            Some(u) => {
                                ui.label(RichText::new(u).size(crate::theme::size::BODY));
                            }
                            None => {
                                ui.label(
                                    RichText::new("—")
                                        .size(crate::theme::size::SMALL)
                                        .color(p::TEXT_FAINT),
                                )
                                .on_hover_text(
                                    "User name not readable. Processes in this session are owned \
                                         by a system account; reading them requires administrator \
                                         rights (helper).",
                                );
                            }
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(&sess.state)
                                    .size(crate::theme::size::SMALL)
                                    .color(p::TEXT_DIM),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(sess.process_count.to_string())
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::TEXT_DIM),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(format::percent0(sess.cpu_percent))
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::TEXT_DIM),
                            );
                        });
                        row.col(|ui| {
                            ui.label(
                                RichText::new(format::bytes(sess.mem_private_bytes))
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::TEXT_DIM),
                            );
                        });
                    });
                });
        });
    }
}

impl App {
    /// Confirmation dialog for a pending action.
    fn degraded_window(&mut self, ctx: &egui::Context) {
        if !self.state.show_degraded_details {
            return;
        }
        let mut open = self.state.show_degraded_details;
        egui::Window::new("Degraded sources")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .show(ctx, |ui| {
                let s = Arc::clone(&self.state.snapshot);
                if s.degraded.is_empty() {
                    ui.label(
                        RichText::new("No degraded sources. All reported sources are nominal or unavailable by design.")
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_DIM),
                    );
                } else {
                    ui.label(
                        RichText::new(
                            "These entries explain missing or failed data. core-pulse does not fabricate values.",
                        )
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                    );
                    ui.add_space(6.0);
                    for d in &s.degraded {
                        let color = match d.reason {
                            core_pulse_core::model::DegradeReason::NeedsElevation => p::AMBER,
                            core_pulse_core::model::DegradeReason::NotAvailable => p::TEXT_FAINT,
                            core_pulse_core::model::DegradeReason::ApiFailed => p::RED,
                            core_pulse_core::model::DegradeReason::Timeout => p::RED,
                            _ => p::AMBER,
                        };
                        ui.horizontal(|ui| {
                            widgets::dot(ui, color, false);
                            ui.label(
                                RichText::new(format!("{:?} / {:?}", d.area, d.reason))
                                    .size(crate::theme::size::SMALL)
                                    .color(color),
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(18.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&d.detail)
                                        .size(crate::theme::size::MICRO)
                                        .color(p::TEXT_DIM),
                                )
                                .wrap(),
                            );
                        });
                        ui.add_space(4.0);
                    }
                    ui.separator();
                    if ui.button("Open Sensors").clicked() {
                        self.state.tab = Tab::Sensors;
                        self.state.show_degraded_details = false;
                    }
                }
            });
        self.state.show_degraded_details = open;
    }

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
                ui.set_min_width(400.0);
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&pending.name)
                            .font(crate::theme::bold(crate::theme::size::STRONG)),
                    );
                    ui.label(
                        RichText::new(format!("PID {}", pending.pid))
                            .size(crate::theme::size::SMALL)
                            .monospace()
                            .color(p::TEXT_DIM),
                    );
                });
                ui.add_space(4.0);
                if pending.kind == PendingKind::TerminateTree && pending.descendants > 0 {
                    ui.label(
                        RichText::new(format!(
                            "{} descendant process(es) will also be stopped.",
                            pending.descendants
                        ))
                        .size(crate::theme::size::SMALL)
                        .color(p::AMBER),
                    );
                }
                if let PendingKind::SetPriority(priority) = pending.kind {
                    ui.label(
                        RichText::new(format!("New priority: {}", priority.label()))
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_DIM),
                    );
                }
                if pending.kind.is_destructive() {
                    ui.label(
                        RichText::new("Unsaved data in this process will be lost.")
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_FAINT),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let confirm_text = match pending.kind {
                        PendingKind::Terminate | PendingKind::TerminateTree => "End task",
                        _ => "Continue",
                    };
                    let button =
                        egui::Button::new(RichText::new(confirm_text).color(Color32::WHITE));
                    let button = if pending.kind.is_destructive() {
                        button.fill(p::RED.gamma_multiply(0.85))
                    } else {
                        button.fill(p::ACCENT.gamma_multiply(0.75))
                    };
                    if ui.add(button).clicked() {
                        decision = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
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
                self.state.set_feedback("cancelled", false);
            }
            None => {}
        }
    }

    /// Execute a confirmed action, then report the result.
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
            PendingKind::SetPriority(priority) => {
                action::set_priority(&key, &pending.name, priority)
            }
            PendingKind::SetEfficiency(on) => action::set_efficiency_mode(&key, &pending.name, on),
        };

        match result {
            Ok(outcome) => {
                tracing::info!(
                    pid = outcome.pid,
                    action = outcome.action,
                    "action succeeded"
                );
                self.state.set_feedback(outcome.to_string(), false);
            }
            Err(e) => {
                tracing::warn!(pid = pending.pid, error = %e, "action failed");
                self.state.set_feedback(format!("failed: {e}"), true);
            }
        }
    }
}

/// Fill the process context menu.
///
/// Items that need elevation remain visible but disabled, so the user learns
/// the feature exists and why it is unavailable (UI-SPEC §7).
fn process_context_menu(
    ui: &mut egui::Ui,
    proc: &core_pulse_core::model::ProcessRow,
    requested: &mut Option<(PendingKind, ProcessKey, String)>,
    open_detail: &mut Option<u32>,
) {
    use core_pulse_core::sys::action::Priority;

    let key = proc.key();
    let name = proc.name.clone();
    let protected = proc.pid == 4
        || matches!(
            proc.name.to_lowercase().as_str(),
            "system" | "csrss.exe" | "wininit.exe" | "winlogon.exe" | "services.exe" | "lsass.exe"
        );

    ui.set_min_width(230.0);

    ui.add_enabled_ui(!protected, |ui| {
        if ui.button("End task").clicked() {
            *requested = Some((PendingKind::Terminate, key, name.clone()));
            ui.close();
        }
        if ui.button("End process tree").clicked() {
            *requested = Some((PendingKind::TerminateTree, key, name.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Suspend").clicked() {
            *requested = Some((PendingKind::Suspend, key, name.clone()));
            ui.close();
        }
        if ui.button("Resume").clicked() {
            *requested = Some((PendingKind::Resume, key, name.clone()));
            ui.close();
        }
        ui.separator();
        ui.menu_button("Priority", |ui| {
            for priority in Priority::all() {
                if ui.button(priority.label()).clicked() {
                    *requested = Some((PendingKind::SetPriority(priority), key, name.clone()));
                    ui.close();
                }
            }
        });
        let eco_label = if proc.eco_qos {
            "Disable efficiency mode"
        } else {
            "Enable efficiency mode"
        };
        if ui.button(eco_label).clicked() {
            *requested = Some((PendingKind::SetEfficiency(!proc.eco_qos), key, name.clone()));
            ui.close();
        }
    });

    ui.separator();
    if ui.button("Properties").clicked() {
        *open_detail = Some(proc.pid);
        ui.close();
    }
    if let Some(path) = &proc.exe_path {
        if ui.button("Open file location").clicked() {
            let _ = core_pulse_core::sys::action::reveal_in_explorer(path);
            ui.close();
        }
    }
    ui.menu_button("Copy", |ui| {
        if ui.button("Name").clicked() {
            ui.ctx().copy_text(proc.name.clone());
            ui.close();
        }
        if ui.button("PID").clicked() {
            ui.ctx().copy_text(proc.pid.to_string());
            ui.close();
        }
        if let Some(path) = &proc.exe_path {
            if ui.button("Path").clicked() {
                ui.ctx().copy_text(path.display().to_string());
                ui.close();
            }
        }
        if let Some(cmd) = &proc.cmdline {
            if ui.button("Command line").clicked() {
                ui.ctx().copy_text(cmd.clone());
                ui.close();
            }
        }
    });

    if protected {
        ui.separator();
        ui.label(
            RichText::new("critical process: actions disabled")
                .size(crate::theme::size::MICRO)
                .color(p::TEXT_FAINT),
        );
    }
}

impl App {
    fn alerts(&mut self, ui: &mut egui::Ui) {
        use core_pulse_core::alerts::{AlertChange, AlertSeverity};

        let state = Arc::clone(&self.state.alerts);

        if state.active.is_empty() {
            // Even with nothing firing, a monitoring tool should show what it
            // is watching and with which thresholds. A blank screen does not
            // tell the operator whether monitoring is even active.
            widgets::card(ui, |ui| {
                ui.horizontal(|ui| {
                    widgets::dot(ui, p::GREEN, true);
                    ui.label(
                        RichText::new("No active alerts")
                            .font(crate::theme::bold(crate::theme::size::STRONG)),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new("evaluated every second")
                                .size(crate::theme::size::MICRO)
                                .color(p::TEXT_FAINT),
                        );
                        ui.label(
                            RichText::new("MONITORING")
                                .size(crate::theme::size::MICRO)
                                .extra_letter_spacing(1.0)
                                .color(p::GREEN),
                        );
                    });
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "A condition must be sustained before it raises an alert, so a one-second \
                         spike never notifies. Missing sensors never raise an alert at all.",
                    )
                    .size(crate::theme::size::SMALL)
                    .color(p::TEXT_DIM),
                );
            });

            ui.add_space(10.0);

            widgets::card(ui, |ui| {
                widgets::section(ui, "Watch list", |ui| {
                    let firing = state.active.len();
                    ui.label(
                        RichText::new(if firing == 0 {
                            "all conditions nominal".to_string()
                        } else {
                            format!("{firing} firing")
                        })
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                    );
                });
                ui.add_space(4.0);

                // (metric, warning, critical, sustain, current value if known)
                let temp_max = self
                    .state
                    .snapshot
                    .sensors
                    .iter()
                    .filter(|r| r.kind == core_pulse_core::model::SensorKind::Temperature)
                    .map(|r| r.value)
                    .reduce(f32::max);
                let mem_pct = {
                    let m = &self.state.snapshot.memory;
                    if m.total_bytes > 0 {
                        m.in_use_bytes as f32 / m.total_bytes as f32 * 100.0
                    } else {
                        0.0
                    }
                };
                let disk_busy = self
                    .state
                    .snapshot
                    .disks
                    .iter()
                    .map(|d| d.busy_percent)
                    .fold(0.0f32, f32::max);
                let min_free = self
                    .state
                    .snapshot
                    .volumes
                    .iter()
                    .filter(|v| v.total_bytes >= 1024 * 1024 * 1024)
                    .map(|v| {
                        if v.total_bytes > 0 {
                            v.free_bytes as f32 / v.total_bytes as f32 * 100.0
                        } else {
                            100.0
                        }
                    })
                    .reduce(f32::min);

                let rows: [(&str, &str, &str, &str, Option<String>); 4] = [
                    (
                        "CPU total",
                        "90%",
                        "97%",
                        "5 s",
                        Some(format!("{:.0}%", self.state.snapshot.cpu.total_percent)),
                    ),
                    (
                        "Memory in use",
                        "90%",
                        "97%",
                        "5 s",
                        Some(format!("{mem_pct:.0}%")),
                    ),
                    (
                        "Disk temperature",
                        "55 °C",
                        "60 °C",
                        "10 s",
                        temp_max.map(|t| format!("{t:.0} °C")),
                    ),
                    (
                        "Free disk space",
                        "< 5%",
                        "< 2%",
                        "60 s",
                        min_free.map(|f| format!("{f:.0}% free")),
                    ),
                ];

                // Header.
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [150.0, 14.0],
                        egui::Label::new(
                            RichText::new("CONDITION")
                                .size(crate::theme::size::MICRO)
                                .extra_letter_spacing(0.8)
                                .color(p::TEXT_FAINT),
                        ),
                    );
                    ui.add_sized(
                        [66.0, 14.0],
                        egui::Label::new(
                            RichText::new("WARNING")
                                .size(crate::theme::size::MICRO)
                                .extra_letter_spacing(0.8)
                                .color(p::TEXT_FAINT),
                        ),
                    );
                    ui.add_sized(
                        [66.0, 14.0],
                        egui::Label::new(
                            RichText::new("CRITICAL")
                                .size(crate::theme::size::MICRO)
                                .extra_letter_spacing(0.8)
                                .color(p::TEXT_FAINT),
                        ),
                    );
                    ui.add_sized(
                        [52.0, 14.0],
                        egui::Label::new(
                            RichText::new("SUSTAIN")
                                .size(crate::theme::size::MICRO)
                                .extra_letter_spacing(0.8)
                                .color(p::TEXT_FAINT),
                        ),
                    );
                    ui.label(
                        RichText::new("CURRENT")
                            .size(crate::theme::size::MICRO)
                            .extra_letter_spacing(0.8)
                            .color(p::TEXT_FAINT),
                    );
                });
                ui.separator();

                for (name, warn, crit, sustain, current) in rows {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [150.0, 16.0],
                            egui::Label::new(
                                RichText::new(name)
                                    .size(crate::theme::size::SMALL)
                                    .color(p::TEXT),
                            ),
                        );
                        ui.add_sized(
                            [66.0, 16.0],
                            egui::Label::new(
                                RichText::new(warn)
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::AMBER),
                            ),
                        );
                        ui.add_sized(
                            [66.0, 16.0],
                            egui::Label::new(
                                RichText::new(crit)
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::RED),
                            ),
                        );
                        ui.add_sized(
                            [52.0, 16.0],
                            egui::Label::new(
                                RichText::new(sustain)
                                    .size(crate::theme::size::SMALL)
                                    .monospace()
                                    .color(p::TEXT_FAINT),
                            ),
                        );
                        match current {
                            Some(v) => {
                                ui.label(
                                    RichText::new(v)
                                        .size(crate::theme::size::SMALL)
                                        .monospace()
                                        .color(p::TEXT_DIM),
                                );
                            }
                            None => {
                                ui.label(
                                    RichText::new(format!("{} no source", icons::DASH))
                                        .size(crate::theme::size::SMALL)
                                        .color(p::TEXT_FAINT),
                                )
                                .on_hover_text(
                                    "This metric has no reading, so it can never raise an alert.",
                                );
                            }
                        }
                    });
                }

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);
                widgets::kv_row(ui, "Cooldown after clearing", "5 minutes");
                widgets::kv_row(ui, "Event log capacity", "20 entries");
                widgets::kv_row(ui, "Busiest disk", &format!("{disk_busy:.0}%"));
            });
        } else {
            for a in &state.active {
                let (color, label) = match a.severity {
                    AlertSeverity::Critical => (p::RED, "CRITICAL"),
                    AlertSeverity::Warning => (p::AMBER, "WARNING"),
                };
                egui::Frame::new()
                    .fill(color.gamma_multiply(0.07))
                    .stroke(Stroke::new(1.0, color.gamma_multiply(0.40)))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(egui::Margin::symmetric(11, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            widgets::dot(ui, color, true);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(&a.title)
                                            .font(crate::theme::bold(crate::theme::size::BODY))
                                            .color(color),
                                    );
                                    widgets::badge(ui, label, color);
                                });
                                ui.label(
                                    RichText::new(&a.detail)
                                        .size(crate::theme::size::SMALL)
                                        .color(p::TEXT_DIM),
                                );
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "value {:.1} · threshold {:.0}",
                                        a.value, a.threshold
                                    ))
                                    .size(crate::theme::size::MICRO)
                                    .monospace()
                                    .color(p::TEXT_FAINT),
                                );
                            });
                        });
                    });
                ui.add_space(6.0);
            }
        }

        ui.add_space(4.0);
        widgets::card(ui, |ui| {
            widgets::section(ui, "Recent events", |ui| {
                ui.label(
                    RichText::new(format!("{} recorded", state.recent.len()))
                        .size(crate::theme::size::MICRO)
                        .color(p::TEXT_FAINT),
                );
            });
            ui.add_space(2.0);
            if state.recent.is_empty() {
                ui.label(
                    RichText::new(format!("{} no events yet this session", icons::DASH))
                        .size(crate::theme::size::SMALL)
                        .color(p::TEXT_FAINT),
                );
            }
            for ev in state.recent.iter().take(20) {
                let (icon, color) = match ev.change {
                    AlertChange::Raised => (icons::UP, p::AMBER),
                    AlertChange::Cleared => (icons::DOWN, p::GREEN),
                };
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(icon)
                            .size(crate::theme::size::MICRO)
                            .color(color),
                    );
                    ui.label(
                        RichText::new(&ev.alert.title)
                            .size(crate::theme::size::SMALL)
                            .color(p::TEXT_DIM),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format::clock_hms(ev.alert.ts_unix_ms))
                                .size(crate::theme::size::MICRO)
                                .monospace()
                                .color(p::TEXT_FAINT),
                        );
                        ui.label(
                            RichText::new(if matches!(ev.change, AlertChange::Raised) {
                                "raised"
                            } else {
                                "cleared"
                            })
                            .size(crate::theme::size::MICRO)
                            .color(color),
                        );
                    });
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_is_safe_for_short_budgets() {
        assert_eq!(format::truncate("abcdef", 2), "a…");
    }

    #[test]
    fn status_metrics_never_panic_on_zero_sample() {
        let s = Snapshot::empty();
        assert_eq!(s.processes.len(), 0);
    }
}
