//! CLI headless: alat verifikasi untuk collector dan sensor.
//!
//! Perintah: `top`, `doctor`, `bench`, `record`.

use clap::{Parser, Subcommand};
use core_pulse_core::{Collector, Snapshot};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "core-pulse-cli", version, about = "core-pulse CLI (headless)")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Tampilkan proses sekali, atau terus-menerus dengan --watch.
    Top {
        /// Interval refresh, mis. "1s", "500ms". Tanpa flag ini, cetak sekali.
        #[arg(long)]
        watch: Option<String>,
        /// Keluarkan JSON snapshot lengkap.
        #[arg(long)]
        json: bool,
        /// Batasi jumlah baris yang ditampilkan.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Urutkan berdasarkan: cpu, mem, name, pid.
        #[arg(long, default_value = "cpu")]
        sort: String,
    },
    /// Laporkan kondisi mesin dan ketersediaan sumber data.
    Doctor,
    /// Ukur performa collector (Gate G0).
    Bench {
        #[arg(long, default_value_t = 10)]
        seconds: u64,
    },
    /// Bandingkan CPU% kita dengan `GetSystemTimes` (jalur independen).
    Compare {
        #[arg(long, default_value_t = 30)]
        seconds: u64,
    },
    /// Verifikasi silang disk/jaringan/GPU terhadap query PDH independen (Gate G1).
    Verify {
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// Ukur juga I/O nyata: tulis file sebesar ini lalu bandingkan dengan
        /// angka yang dilaporkan collector. 0 = lewati.
        #[arg(long, default_value_t = 32)]
        io_size_mb: u64,
    },
    /// Uji guard PID-reuse dan aksi proses (Gate G2).
    ActionSelftest,
    /// Daftar layanan Windows.
    Services {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Daftar entri startup.
    Startup,
    /// Daftar sesi pengguna.
    Users,
    /// Tampilkan status dan bacaan semua provider sensor (Gate G3).
    Sensors {
        /// Aktifkan bridge LibreHardwareMonitor (opt-in, default mati).
        #[arg(long)]
        bridge: bool,
        /// Gabungkan bacaan dari helper elevated (kalau berjalan).
        #[arg(long)]
        helper: bool,
        /// Jumlah sampel berturut-turut.
        #[arg(long, default_value_t = 2)]
        samples: u32,
    },
    /// Periksa helper elevated: versi, hak akses, status provider, dan bacaan.
    Helper,
    /// Uji helper end-to-end: spawn, frame rusak, helper mati (Gate G4).
    HelperSelftest,
    /// Rekam N snapshot ke file untuk fixture test.
    Record {
        #[arg(long, default_value = "fixture.json")]
        out: String,
        #[arg(long, default_value_t = 1)]
        samples: u32,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Top {
        watch: None,
        json: false,
        limit: 20,
        sort: "cpu".into(),
    }) {
        Command::Top {
            watch,
            json,
            limit,
            sort,
        } => top(watch, json, limit, &sort),
        Command::Doctor => doctor(),
        Command::Bench { seconds } => bench(seconds),
        Command::Compare { seconds } => compare(seconds),
        Command::Verify {
            seconds,
            io_size_mb,
        } => verify(seconds, io_size_mb),
        Command::ActionSelftest => action_selftest(),
        Command::Services { limit } => list_services(limit),
        Command::Startup => list_startup(),
        Command::Users => list_users(),
        Command::Sensors {
            bridge,
            helper,
            samples,
        } => list_sensors(bridge, helper, samples),
        Command::Helper => helper_probe(),
        Command::HelperSelftest => helper_selftest(),
        Command::Record { out, samples } => record(&out, samples),
    }
}

fn parse_interval(s: &str) -> Duration {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        return Duration::from_millis(ms.parse().unwrap_or(1000));
    }
    if let Some(sec) = s.strip_suffix('s') {
        let v: f64 = sec.parse().unwrap_or(1.0);
        return Duration::from_secs_f64(v.max(0.05));
    }
    Duration::from_secs(1)
}

fn top(watch: Option<String>, json: bool, limit: usize, sort: &str) -> anyhow::Result<()> {
    let mut collector = Collector::new();

    // Sampel pertama hanya untuk mengisi baseline delta; tanpa itu semua CPU%
    // akan 0 karena tidak ada pembanding.
    let _baseline = collector.sample();

    match watch {
        Some(interval_str) => {
            let interval = parse_interval(&interval_str);
            let mut ticker = core_pulse_core::sampler::Ticker::new(interval);
            loop {
                ticker.tick();
                let snap = collector.sample();
                if json {
                    print_json(&snap)?;
                } else {
                    print_table(&snap, limit, sort);
                }
            }
        }
        None => {
            // Beri jendela 500 ms supaya CPU% punya arti, termasuk untuk --json.
            std::thread::sleep(Duration::from_millis(500));
            let snap = collector.sample();
            if json {
                print_json(&snap)?;
            } else {
                print_table(&snap, limit, sort);
            }
        }
    }
    Ok(())
}

fn print_json(snap: &Snapshot) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(snap)?);
    Ok(())
}

fn print_table(snap: &Snapshot, limit: usize, sort: &str) {
    // PID 0 is idle: showing it as the "hungriest CPU process" only
    // menyesatkan. Task Manager juga tidak menampilkannya di tab Processes.
    let mut rows: Vec<&core_pulse_core::ProcessRow> =
        snap.processes.iter().filter(|p| p.pid != 0).collect();
    match sort {
        "mem" => rows.sort_by_key(|r| std::cmp::Reverse(r.mem_private_bytes)),
        "name" => rows.sort_by_key(|r| r.name.to_lowercase()),
        "pid" => rows.sort_by_key(|r| r.pid),
        _ => rows.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    println!(
        "core-pulse  seq {}  {:.1}% CPU  {:.1}/{:.1} GB RAM (pressure {})  {} processes",
        snap.seq,
        snap.cpu.total_percent,
        snap.memory.in_use_bytes as f64 / 1e9,
        snap.memory.total_bytes as f64 / 1e9,
        snap.memory.pressure,
        snap.processes.len()
    );
    println!(
        "{:<28} {:>7} {:>7} {:>12} {:>7} {:>7} {:>7}",
        "NAME", "PID", "CPU%", "MEM(MB)", "GPU%", "THREADS", "HANDLES"
    );
    for r in rows.into_iter().take(limit) {
        println!(
            "{:<28} {:>7} {:>7.1} {:>12.1} {:>7} {:>7} {:>7}",
            truncate(&r.name, 28),
            r.pid,
            r.cpu_percent,
            r.mem_private_bytes as f64 / 1e6,
            r.gpu_percent
                .map(|g| format!("{g:.1}"))
                .unwrap_or_else(|| "—".into()),
            r.threads,
            r.handles
        );
    }

    if !snap.disks.is_empty() {
        println!("\ndisks:");
        for d in &snap.disks {
            println!(
                "  #{} {:<24} busy {:>5.1}%  R {:>9}/s  W {:>9}/s  q {:.2}",
                d.index,
                truncate(&d.model, 24),
                d.busy_percent,
                human_bytes(d.read_bps),
                human_bytes(d.write_bps),
                d.queue_depth
            );
        }
    }
    if !snap.gpus.is_empty() {
        println!("\ngpus:");
        for g in &snap.gpus {
            println!(
                "  {:<32} util {}",
                truncate(&g.name, 32),
                g.utilization_percent
                    .map(|u| format!("{u:.1}%"))
                    .unwrap_or_else(|| "—".into())
            );
        }
    }
    if !snap.net.is_empty() {
        println!("\nnetwork:");
        for n in &snap.net {
            println!(
                "  {:<24} {:>9}/s down  {:>9}/s up    {}",
                truncate(&n.name, 24),
                human_bytes(n.rx_bps),
                human_bytes(n.tx_bps),
                if n.is_up { "up" } else { "down" }
            );
        }
    }
    if !snap.degraded.is_empty() {
        println!("\ndegraded sources:");
        for d in &snap.degraded {
            println!("  - {:?}/{:?}: {}", d.area, d.reason, d.detail);
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn doctor() -> anyhow::Result<()> {
    let mut collector = Collector::new();
    // Dua sampel: counter rate (disk, jaringan, GPU) butuh delta antar sampel.
    let _ = collector.sample();
    std::thread::sleep(Duration::from_millis(700));
    let snap = collector.sample();

    println!("core-pulse doctor");
    println!(
        "  OS          : {} (build {})",
        snap.host.os_name, snap.host.os_build
    );
    println!("  Machine     : {}", snap.host.computer_name);
    println!("  CPU         : {}", snap.host.cpu_brand);
    println!(
        "  Cores       : {} physical / {} logical @ {} MHz",
        snap.host.physical_cores, snap.host.logical_cores, snap.host.base_clock_mhz
    );
    println!(
        "  RAM         : {:.1} GB",
        snap.host.total_ram_bytes as f64 / 1e9
    );
    println!("  Elevated    : {}", snap.host.is_elevated);
    println!("  Processes   : {}", snap.processes.len());
    println!(
        "  Memory      : in use {:.2} GB, standby {:.2} GB, free {:.2} GB, commit {:.2}/{:.2} GB",
        snap.memory.in_use_bytes as f64 / 1e9,
        snap.memory.standby_bytes as f64 / 1e9,
        snap.memory.free_bytes as f64 / 1e9,
        snap.memory.commit_total_bytes as f64 / 1e9,
        snap.memory.commit_limit_bytes as f64 / 1e9,
    );

    println!("\n  Data sources:");
    println!("    CPU per-core      : {}", ok(has_per_core(&snap)));
    println!(
        "    Memory breakdown  : {}",
        ok(snap.memory.standby_bytes > 0)
    );
    println!(
        "    Disk (PDH)        : {}",
        availability(snap.disks.len(), "disk")
    );
    println!(
        "    Network           : {}",
        availability(snap.net.len(), "network adapter")
    );
    println!(
        "    GPU (PDH)         : {}",
        availability(snap.gpus.len(), "GPU adapter")
    );
    println!(
        "    ACPI/PDH temp     : {}",
        match snap
            .degraded
            .iter()
            .find(|d| d.area == core_pulse_core::Area::Sensors)
        {
            Some(d) => format!("unavailable ({:?})", d.reason),
            None => "not probed yet".into(),
        }
    );
    println!("    Disk SMART temp   : needs the elevated helper");
    println!("    GPU temperature   : needs a vendor SDK or the LHM bridge");

    if !snap.disks.is_empty() {
        println!("\n  Physical disks:");
        for d in &snap.disks {
            println!(
                "    #{} {:<26} {:?}/{:?} {:>9}{}  busy {:>5.1}%  R {:>9}/s  W {:>9}/s  q {:.2}",
                d.index,
                d.model,
                d.bus,
                d.media,
                human_bytes(d.size_bytes),
                if d.size_is_estimate {
                    " (estimated)"
                } else {
                    ""
                },
                d.busy_percent,
                human_bytes(d.read_bps),
                human_bytes(d.write_bps),
                d.queue_depth
            );
        }
    }
    if !snap.volumes.is_empty() {
        println!("\n  Volumes:");
        for v in &snap.volumes {
            println!(
                "    {:<4} {:<18} {:<6} {:>9} free / {:>9}  disk {:?}",
                v.mount,
                v.label,
                v.fs,
                human_bytes(v.free_bytes),
                human_bytes(v.total_bytes),
                v.disk_index
            );
        }
    }
    if !snap.gpus.is_empty() {
        println!("\n  GPUs:");
        for g in &snap.gpus {
            println!(
                "    {:<34} {:?}  util {:>6}  dedicated {:>9}  shared {:>9}",
                g.name,
                g.vendor,
                g.utilization_percent
                    .map(|u| format!("{u:.1}%"))
                    .unwrap_or_else(|| "—".into()),
                g.dedicated_used_bytes
                    .map(human_bytes)
                    .unwrap_or_else(|| "—".into()),
                g.shared_used_bytes
                    .map(human_bytes)
                    .unwrap_or_else(|| "—".into()),
            );
        }
    }
    Ok(())
}

/// Uji guard PID-reuse secara nyata, bukan hanya lewat unit test.
///
/// Empat skenario, semuanya harus sesuai harapan:
/// 1. aksi dengan identitas benar → berhasil
/// 2. aksi dengan identitas basi (PID sama, waktu beda) → ditolak, proses hidup
/// 3. aksi pada PID yang sudah mati → ditolak
/// 4. aksi pada proses kritikal (PID 4) → ditolak
fn action_selftest() -> anyhow::Result<()> {
    use core_pulse_core::sys::action::{self, ActionError};
    use core_pulse_core::sys::proc::ProcCollector;
    use std::process::{Command, Stdio};

    let mut failures = 0u32;

    fn check(label: &str, ok: bool, detail: &str) -> bool {
        println!(
            "  [{}] {label}{}",
            if ok { "PASS" } else { "FAIL" },
            if detail.is_empty() {
                String::new()
            } else {
                format!(" — {detail}")
            }
        );
        ok
    }

    fn spawn_and_locate() -> (u32, u64) {
        let child = Command::new("cmd.exe")
            .args(["/c", "ping", "-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn the test process");
        let pid = child.id();
        std::mem::forget(child);
        std::thread::sleep(Duration::from_millis(400));

        let mut c = ProcCollector::new();
        let _ = c.collect(1.0);
        let (rows, _) = c.collect(1.0);
        let row = rows
            .iter()
            .find(|r| r.pid == pid)
            .expect("test process disappeared");
        (pid, row.start_time_unix_ms)
    }

    println!("core-pulse action-selftest");
    println!("(each scenario uses a real process that is actually running)\n");

    // --- 1. Correct identity: the action must succeed ---
    let (pid, start) = spawn_and_locate();
    let key = core_pulse_core::model::ProcessKey {
        pid,
        start_time_unix_ms: start,
    };
    let ok = match action::terminate(&key, "cmd.exe") {
        Ok(outcome) => check(
            "correct identity → terminate succeeds",
            outcome.affected == 1,
            "",
        ),
        Err(e) => check(
            "correct identity → terminate succeeds",
            false,
            &e.to_string(),
        ),
    };
    if !ok {
        failures += 1;
    }

    // --- 2. Stale identity: must be rejected and the process must stay alive ---
    let (pid, start) = spawn_and_locate();
    let stale = core_pulse_core::model::ProcessKey {
        pid,
        start_time_unix_ms: start.wrapping_sub(5_000),
    };
    match action::terminate(&stale, "cmd.exe") {
        Err(ActionError::IdentityChanged {
            expected_ms,
            actual_ms,
        }) => {
            if !check(
                "stale identity → rejected",
                true,
                &format!("PID {pid}, expected {expected_ms} ms, actual {actual_ms} ms"),
            ) {
                failures += 1;
            }
        }
        Ok(_) => {
            check(
                "stale identity → rejected",
                false,
                "process was killed prematurely",
            );
            failures += 1;
        }
        Err(e) => {
            check(
                "stale identity → rejected",
                false,
                &format!("wrong error: {e}"),
            );
            failures += 1;
        }
    }
    // Confirm the process really is still alive.
    let mut c = ProcCollector::new();
    let _ = c.collect(1.0);
    let (rows, _) = c.collect(1.0);
    let alive = rows.iter().any(|r| r.pid == pid);
    if !check("after rejection the process is still alive", alive, "") {
        failures += 1;
    }
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();

    // --- 3. Non-existent PID ---
    let dead = core_pulse_core::model::ProcessKey {
        pid: 0xFFFF_FF00,
        start_time_unix_ms: 1,
    };
    let ok = matches!(
        action::terminate(&dead, "ghost.exe"),
        Err(ActionError::ProcessGone) | Err(ActionError::AccessDenied)
    );
    if !check("non-existent PID → rejected", ok, "") {
        failures += 1;
    }

    // --- 4. Critical process ---
    let system = core_pulse_core::model::ProcessKey {
        pid: 4,
        start_time_unix_ms: 0,
    };
    let ok = matches!(
        action::terminate(&system, "System"),
        Err(ActionError::CriticalProcess)
    );
    if !check("critical process (PID 4) → rejected", ok, "") {
        failures += 1;
    }

    // --- 5. A non-destructive action on our own process must not crash ---
    let me = core_pulse_core::model::ProcessKey {
        pid: std::process::id(),
        start_time_unix_ms: 0,
    };
    // The creation time is deliberately wrong, so this must be rejected with IdentityChanged.
    let ok = matches!(
        action::set_priority(&me, "core-pulse-cli.exe", action::Priority::Normal),
        Err(ActionError::IdentityChanged { .. })
    );
    if !check(
        "wrong identity rejected for a non-destructive action",
        ok,
        "",
    ) {
        failures += 1;
    }

    println!();
    if failures == 0 {
        println!("G2: PASS (all scenarios behaved as expected)");
        Ok(())
    } else {
        println!("G2: {failures} scenario(s) FAILED");
        anyhow::bail!("action selftest failed")
    }
}

fn list_services(limit: usize) -> anyhow::Result<()> {
    let (services, degraded) = core_pulse_core::sys::services::collect();
    println!("{} services", services.len());
    println!(
        "{:<28} {:<46} {:<10} {:<8} {:>7}",
        "NAME", "DISPLAY NAME", "STATE", "STARTUP", "PID"
    );
    for s in services.iter().take(limit) {
        println!(
            "{:<28} {:<46} {:<10} {:<8} {:>7}",
            truncate(&s.name, 28),
            truncate(&s.display_name, 46),
            s.state.label(),
            s.start_type.label(),
            if s.pid == 0 {
                "—".to_string()
            } else {
                s.pid.to_string()
            }
        );
    }
    for d in degraded {
        println!("degraded: {:?}/{:?}: {}", d.area, d.reason, d.detail);
    }
    Ok(())
}

fn list_startup() -> anyhow::Result<()> {
    let (entries, degraded) = core_pulse_core::sys::startup::collect();
    println!("{} startup entries", entries.len());
    println!("{:<46} {:<16} {:<6} TARGET", "NAME", "SOURCE", "32BIT");
    for e in &entries {
        println!(
            "{:<46} {:<16} {:<6} {}",
            truncate(&e.name, 46),
            e.source.label(),
            if e.is_32bit { "ya" } else { "—" },
            match e.target_exists {
                Some(true) => "present",
                Some(false) => "missing",
                None => "—",
            }
        );
    }
    for d in degraded {
        println!("degraded: {:?}/{:?}: {}", d.area, d.reason, d.detail);
    }
    Ok(())
}

fn list_users() -> anyhow::Result<()> {
    let mut collector = Collector::new();
    // Detail proses (termasuk nama pengguna) diisi bertahap: 40 proses per
    // sampel. Tiga sampel cukup untuk menemukan user tiap sesi.
    for _ in 0..4 {
        let _ = collector.sample();
        std::thread::sleep(Duration::from_millis(600));
    }
    let snap = collector.sample();
    println!("{} session(s)", snap.sessions.len());
    println!(
        "{:>5} {:<30} {:<12} {:>8} {:>8} {:>10}",
        "SESSION", "USER", "STATE", "PROCESSES", "CPU", "MEMORY"
    );
    for s in &snap.sessions {
        println!(
            "{:>5} {:<30} {:<12} {:>8} {:>7.1}% {:>10}",
            s.session_id,
            s.user.clone().unwrap_or_else(|| "—".into()),
            s.state,
            s.process_count,
            s.cpu_percent,
            human_bytes(s.mem_private_bytes)
        );
    }
    Ok(())
}

fn availability(count: usize, unit: &str) -> String {
    if count == 0 {
        "NOT available".to_string()
    } else if count == 1 {
        format!("available (1 {unit})")
    } else {
        format!("available ({count} {unit}s)")
    }
}

/// Tampilkan status semua provider sensor beserta bacaannya (Gate G3).
///
/// Ini alat verifikasi yang sama yang dipakai untuk memutuskan apakah fitur
/// "monitoring suhu" benar-benar berfungsi di sebuah mesin — bukan asumsi.
fn list_sensors(enable_bridge: bool, enable_helper: bool, samples: u32) -> anyhow::Result<()> {
    use core_pulse_sensors::Availability;

    let mut sensors = core_pulse_sensors::Sensors::with_options(enable_bridge, enable_helper);
    println!("core-pulse sensors (bridge: {enable_bridge}, helper: {enable_helper})");
    println!(
        "Provider: acpi-pdh, smart, gpu-vendor{}{}\n",
        if enable_bridge { ", lhm-bridge" } else { "" },
        if enable_helper { ", helper" } else { "" }
    );

    sensors.probe_all(true);

    // Poll pertama juga menggabungkan status helper. Status lokal saja tidak
    // cukup: provider SMART bisa "butuh administrator" padahal helper sudah
    // menyediakan datanya.
    let first_report = sensors.poll().clone();
    println!("Provider status:");
    let statuses: Vec<(String, String)> = first_report
        .statuses
        .iter()
        .map(|(id, a)| {
            let text = match a {
                Availability::Available => "available".to_string(),
                Availability::NeedsElevation => "needs administrator".to_string(),
                Availability::NotAvailable(why) => format!("unavailable: {why}"),
                Availability::Failed(e) => format!("failed: {e}"),
            };
            (id.clone(), text)
        })
        .collect();
    for (id, text) in &statuses {
        println!("  {id:<14} {text}");
    }

    for i in 0..samples.max(1) {
        if i > 0 {
            std::thread::sleep(Duration::from_secs(2));
        }
        let report = sensors.poll();
        println!("\nSample {}: {} reading(s)", i + 1, report.readings.len());
        if report.readings.is_empty() {
            println!("  (no readable sensor — see the status above)");
            continue;
        }
        println!(
            "  {:<28} {:>9} {:<9} {:<12} {:<12} ID",
            "LABEL", "VALUE", "UNIT", "SOURCE", "CONFIDENCE"
        );
        for r in &report.readings {
            println!(
                "  {:<28} {:>9.1} {:<9} {:<12} {:<12} {}",
                truncate(&r.label, 28),
                r.value,
                format!("{:?}", r.unit),
                format!("{:?}", r.source),
                format!("{:?}", r.confidence),
                r.id
            );
        }
    }

    // Verdict Gate G3, dihitung dari data yang benar-benar terbaca.
    let report = sensors.poll();
    let disk_temps: Vec<_> = report
        .readings
        .iter()
        .filter(|r| r.disk_index.is_some())
        .collect();
    let cpu_status = statuses
        .iter()
        .find(|(id, _)| id == "acpi-pdh")
        .map(|(_, t)| t.clone())
        .unwrap_or_default();

    println!("\nG3:");
    println!(
        "  Disk temperature    : {}",
        if disk_temps.is_empty() {
            "not read yet".to_string()
        } else {
            format!(
                "{} sensor(s) ({})",
                disk_temps.len(),
                disk_temps
                    .iter()
                    .map(|r| format!("{:.0} °C", r.value))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    );
    println!("  CPU temperature     : {cpu_status}");
    println!(
        "  note                : a missing CPU temperature is not a bug; this firmware \
         exposes no ACPI thermal zone (docs/ENVIRONMENT-AUDIT.md)"
    );
    Ok(())
}

/// Inspect the elevated helper over its named pipe. This command does not
/// require admin; it only reads what the helper already provides.
fn helper_probe() -> anyhow::Result<()> {
    use core_pulse_sensors::helper;

    println!(
        "expected helper: {}",
        helper::expected_helper_path().display()
    );

    let data = helper::fetch_once().map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "connected: version {} (PID {}), elevated: {}",
        data.info.version, data.info.pid, data.info.elevated
    );

    println!("\nProvider status inside the helper:");
    for status in &data.statuses {
        let text = match &status.availability {
            core_pulse_core::model::SensorAvailability::Available => "available".to_string(),
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
        println!("  {:<14} {text}", status.provider);
    }

    println!("\nReadings: {} sensor(s)", data.readings.len());
    for r in &data.readings {
        println!(
            "  {:<28} {:>7.1} {:<8} {:<12} {}",
            truncate(&r.label, 28),
            r.value,
            format!("{:?}", r.unit),
            format!("{:?}", r.source),
            r.id
        );
    }

    if !data.info.elevated {
        println!("\nWARNING: the helper is running without elevation; SMART stays unavailable.");
    }
    Ok(())
}

/// Uji helper end-to-end untuk Gate G4.
///
/// Yang diuji bukan hanya "happy path": frame >1 MiB, JSON tidak valid, dan
/// helper yang mati di tengah jalan. Setelah setiap frame rusak, helper harus
/// tetap melayani klien baru.
fn helper_selftest() -> anyhow::Result<()> {
    use core_pulse_ipc::pipe::PipeClient;
    use core_pulse_ipc::{FrameHeader, PIPE_NAME, PROTO_VERSION};
    use core_pulse_sensors::helper::{self, HelperError};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    struct KillGuard(std::process::Child);
    impl Drop for KillGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    let helper_path = helper::expected_helper_path();
    if !helper_path.is_file() {
        anyhow::bail!("helper not found at {}", helper_path.display());
    }

    if PipeClient::connect_timeout(PIPE_NAME, 50).is_ok() {
        println!("SKIP: a helper is already running; stop it first to run the self-test");
        return Ok(());
    }

    println!("spawn: {}", helper_path.display());
    let child = Command::new(&helper_path)
        .arg("--console")
        .env("CORE_PULSE_LOG", "error")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn helper: {e}"))?;
    let mut guard = KillGuard(child);

    // Tunggu pipe siap, maksimum 5 detik.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut ready = false;
    while Instant::now() < deadline {
        if PipeClient::connect_timeout(PIPE_NAME, 50).is_ok() {
            ready = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if !ready {
        anyhow::bail!("the helper did not open its pipe within 5 seconds");
    }
    println!("OK   pipe ready");

    let first = helper::fetch_once().map_err(|e| anyhow::anyhow!("first fetch: {e}"))?;
    println!(
        "OK   hello + sensors: version {} elevated {} ({} readings)",
        first.info.version,
        first.info.elevated,
        first.readings.len()
    );

    // Frame dengan panjang 2 MB: harus ditolak sebelum alokasi.
    {
        let client = PipeClient::connect_timeout(PIPE_NAME, 500)
            .map_err(|e| anyhow::anyhow!("connect (oversized): {e}"))?;
        let header = FrameHeader {
            length: 2_000_000,
            proto_version: PROTO_VERSION,
        };
        client
            .write_raw(&header.encode(), 500)
            .map_err(|e| anyhow::anyhow!("write oversized frame: {e}"))?;
    }
    std::thread::sleep(Duration::from_millis(300));
    helper::fetch_once()
        .map_err(|e| anyhow::anyhow!("helper died after the oversized frame: {e}"))?;
    println!("OK   frame >1 MiB rejected, helper still alive");

    // JSON tidak valid dengan header yang sah.
    {
        let client = PipeClient::connect_timeout(PIPE_NAME, 500)
            .map_err(|e| anyhow::anyhow!("connect (invalid json): {e}"))?;
        let payload = b"not json";
        let header = FrameHeader {
            length: payload.len() as u32,
            proto_version: PROTO_VERSION,
        };
        let mut frame = Vec::from(header.encode());
        frame.extend_from_slice(payload);
        client
            .write_raw(&frame, 500)
            .map_err(|e| anyhow::anyhow!("write invalid frame: {e}"))?;
    }
    std::thread::sleep(Duration::from_millis(300));
    helper::fetch_once().map_err(|e| anyhow::anyhow!("helper died after the invalid JSON: {e}"))?;
    println!("OK   invalid JSON rejected, helper still alive");

    // Matikan helper: klien harus gagal cepat dengan NotRunning.
    let _ = guard.0.kill();
    let _ = guard.0.wait();

    let start = Instant::now();
    let error = helper::fetch_once().expect_err("helper is dead, fetch must fail");
    let elapsed = start.elapsed();
    match error {
        HelperError::NotRunning(_) => {}
        other => anyhow::bail!("wrong error after the helper died: {other}"),
    }
    if elapsed > Duration::from_secs(2) {
        anyhow::bail!("helper-death detection too slow: {elapsed:?}");
    }
    println!(
        "OK   helper death detected in {:.0} ms (NotRunning)",
        elapsed.as_secs_f64() * 1000.0
    );

    println!("\nG4 helper-selftest: PASS");
    Ok(())
}

/// Byte → "1.2 GB" untuk keluaran CLI.
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

fn ok(v: bool) -> &'static str {
    if v { "available" } else { "NOT available" }
}

fn has_per_core(snap: &Snapshot) -> bool {
    !snap.cpu.per_core_percent.is_empty()
}

fn bench(seconds: u64) -> anyhow::Result<()> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    fn self_cpu_100ns() -> u64 {
        let mut c = FILETIME::default();
        let mut e = FILETIME::default();
        let mut k = FILETIME::default();
        let mut u = FILETIME::default();
        // SAFETY: pseudo-handle valid; keempat out-param valid.
        if unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) }.is_err()
        {
            return 0;
        }
        let k = ((k.dwHighDateTime as u64) << 32) | k.dwLowDateTime as u64;
        let u = ((u.dwHighDateTime as u64) << 32) | u.dwLowDateTime as u64;
        k + u
    }

    /// (private_bytes, working_set_bytes)
    fn self_memory() -> (u64, u64) {
        let mut pmc = PROCESS_MEMORY_COUNTERS_EX {
            cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            ..Default::default()
        };
        // SAFETY: cb diisi, struct valid.
        let _ = unsafe {
            K32GetProcessMemoryInfo(
                GetCurrentProcess(),
                &mut pmc as *mut _ as *mut _,
                std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
            )
        };
        (pmc.PrivateUsage as u64, pmc.WorkingSetSize as u64)
    }

    let mut collector = Collector::new();
    let _ = collector.sample();
    let ncpu = collector_host_cores();

    let deadline = Instant::now() + Duration::from_secs(seconds.max(1));
    let mut samples = 0u64;
    let mut enumerate_ms = Vec::new();
    let mut build_ms = Vec::new();
    let cpu_start = self_cpu_100ns();
    let wall_start = Instant::now();
    let mut peak_private = 0u64;
    let mut peak_ws = 0u64;

    while Instant::now() < deadline {
        let t0 = Instant::now();
        let snap = collector.sample();
        build_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        samples += 1;

        let (p, ws) = self_memory();
        peak_private = peak_private.max(p);
        peak_ws = peak_ws.max(ws);

        // Enumerasi tanpa cache (cold) diukur lebih jarang: biayanya realistis
        // untuk startup, bukan untuk steady state.
        if samples % 5 == 1 {
            let t1 = Instant::now();
            let mut fresh = Collector::new();
            let _ = fresh.sample();
            enumerate_ms.push(t1.elapsed().as_secs_f64() * 1000.0);
        }
        drop(snap);
        std::thread::sleep(Duration::from_secs(1));
    }

    let wall = wall_start.elapsed().as_secs_f64();
    let cpu_used = (self_cpu_100ns() - cpu_start) as f64 / 10_000_000.0;
    let cpu_one_core = if wall > 0.0 {
        cpu_used / wall * 100.0
    } else {
        0.0
    };
    let cpu_system = cpu_one_core / ncpu as f64;

    fn stats(v: &[f64]) -> (f64, f64, f64) {
        if v.is_empty() {
            return (0.0, 0.0, 0.0);
        }
        let avg = v.iter().sum::<f64>() / v.len() as f64;
        let max = v.iter().cloned().fold(f64::MIN, f64::max);
        let mut sorted = v.to_vec();
        sorted.sort_by(f64::total_cmp);
        let p95 = sorted[((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1)];
        (avg, p95, max)
    }

    let (b_avg, b_p95, b_max) = stats(&build_ms);
    let (e_avg, e_p95, e_max) = stats(&enumerate_ms);

    println!("samples: {samples}  (interval 1 s)");
    println!(
        "cpu_self: {cpu_one_core:.2}% dari satu core  |  {cpu_system:.2}% dari sistem ({ncpu} core)"
    );
    println!(
        "private_bytes: {:.1} MB   working_set: {:.1} MB",
        peak_private as f64 / 1e6,
        peak_ws as f64 / 1e6
    );
    println!("snapshot_build_ms: avg {b_avg:.2}  p95 {b_p95:.2}  max {b_max:.2}");
    println!("cold_collector_ms: avg {e_avg:.2}  p95 {e_p95:.2}  max {e_max:.2}");

    // Budget dari docs/PERF.md.
    let mut failed = false;
    if cpu_system > 1.0 {
        println!("WARN: CPU sistem melebihi budget 1%");
        failed = true;
    }
    if peak_private > 100_000_000 {
        println!("WARN: private bytes melebihi budget 100 MB");
        failed = true;
    }
    if b_p95 > 100.0 {
        println!("WARN: snapshot build p95 melebihi budget 100 ms");
        failed = true;
    }
    if !failed {
        println!("BUDGET: PASS");
    }
    Ok(())
}

fn collector_host_cores() -> u32 {
    core_pulse_core::sys::cpu::logical_cpu_count().max(1)
}

fn verify(seconds: u64, io_size_mb: u64) -> anyhow::Result<()> {
    use core_pulse_core::sys::pdh::Query;

    // Query independen: sengaja dibuat terpisah dari collector supaya bug
    // parsing/agregasi di collector tidak ikut "terverifikasi" oleh dirinya sendiri.
    let mut ref_query = Query::new().ok_or_else(|| anyhow::anyhow!("PDH unavailable"))?;
    let ref_read = ref_query.add(r"\PhysicalDisk(*)\Disk Read Bytes/sec");
    let ref_write = ref_query.add(r"\PhysicalDisk(*)\Disk Write Bytes/sec");
    let ref_idle = ref_query.add(r"\PhysicalDisk(*)\% Idle Time");
    let ref_gpu = ref_query.add(r"\GPU Engine(*)\Utilization Percentage");
    let ref_net = ref_query.add(r"\Network Interface(*)\Bytes Received/sec");

    let mut collector = Collector::new();
    let _ = collector.sample();
    // Prime query referensi: counter rate butuh dua sampel sebelum punya nilai.
    ref_query.collect();

    let mut disk_diffs: Vec<f64> = Vec::new();
    let mut gpu_diffs: Vec<f64> = Vec::new();
    let mut net_diffs: Vec<f64> = Vec::new();

    println!(
        "{:<4} {:<6} {:>12} {:>12} {:>8}   {:>10} {:>10} {:>8}",
        "t", "objek", "kita", "referensi", "selisih", "", "", ""
    );

    for t in 1..=seconds.max(1) {
        std::thread::sleep(Duration::from_secs(1));
        let snap = collector.sample();
        ref_query.collect();

        // --- Disk: bandingkan per indeks ---
        let r_read: std::collections::HashMap<u32, f64> = ref_read
            .as_ref()
            .map(|c| c.array())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(n, v)| {
                Some((core_pulse_core::sys::pdh::parse_physical_disk_index(&n)?, v))
            })
            .collect();
        let r_write: std::collections::HashMap<u32, f64> = ref_write
            .as_ref()
            .map(|c| c.array())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(n, v)| {
                Some((core_pulse_core::sys::pdh::parse_physical_disk_index(&n)?, v))
            })
            .collect();
        let r_idle: std::collections::HashMap<u32, f64> = ref_idle
            .as_ref()
            .map(|c| c.array())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(n, v)| {
                Some((core_pulse_core::sys::pdh::parse_physical_disk_index(&n)?, v))
            })
            .collect();

        for d in &snap.disks {
            let rr = r_read.get(&d.index).copied().unwrap_or(0.0).max(0.0);
            let rw = r_write.get(&d.index).copied().unwrap_or(0.0).max(0.0);
            let ri = r_idle.get(&d.index).copied().unwrap_or(0.0);
            let ours = d.read_bps as f64 + d.write_bps as f64;
            let theirs = rr + rw;
            let diff = relative_diff(ours, theirs, 32.0 * 1024.0);
            disk_diffs.push(diff);
            println!(
                "{t:<4} disk#{} {:<6} {:>12} {:>12} {:>7.1}%",
                d.index,
                "",
                human_bytes(ours as u64),
                human_bytes(theirs as u64),
                diff * 100.0
            );
            let busy_ref = (100.0 - ri).clamp(0.0, 100.0);
            let busy_diff = (d.busy_percent as f64 - busy_ref).abs();
            println!(
                "{:<4} {:<6} {:>12} {:>12} {:>7.1}%",
                "",
                format!("  busy#{}", d.index),
                format!("{:.1}%", d.busy_percent),
                format!("{busy_ref:.1}%"),
                busy_diff
            );
        }

        // --- GPU: maksimum engine per adapter ---
        let ref_engines = ref_gpu.as_ref().map(|c| c.array()).unwrap_or_default();
        let mut ref_max_by_luid: std::collections::HashMap<(u32, u32), f64> =
            std::collections::HashMap::new();
        for (instance, value) in &ref_engines {
            if let Some(luid) = core_pulse_core::sys::pdh::parse_gpu_adapter_luid(instance) {
                let slot = ref_max_by_luid.entry(luid).or_insert(0.0);
                *slot = slot.max(*value);
            }
        }
        let ref_gpu_max = ref_max_by_luid.values().copied().fold(0.0f64, f64::max);
        let our_gpu_max = snap
            .gpus
            .iter()
            .filter_map(|g| g.utilization_percent)
            .fold(0.0f32, f32::max) as f64;
        let gdiff = relative_diff(our_gpu_max, ref_gpu_max, 1.0);
        if ref_gpu_max > 0.0 || our_gpu_max > 0.0 {
            gpu_diffs.push(gdiff);
            println!(
                "{:<4} {:<6} {:>12} {:>12} {:>7.1}%",
                "",
                "gpu",
                format!("{our_gpu_max:.1}%"),
                format!("{ref_gpu_max:.1}%"),
                gdiff * 100.0
            );
        }

        // --- Jaringan: cocokkan deskripsi adapter dengan instance PDH ---
        let ref_net_map: std::collections::HashMap<String, f64> = ref_net
            .as_ref()
            .map(|c| c.array())
            .unwrap_or_default()
            .into_iter()
            .map(|(n, v)| (n.to_lowercase(), v.max(0.0)))
            .collect();
        for a in &snap.net {
            let key = a.description.to_lowercase();
            if let Some(theirs) = ref_net_map.get(&key) {
                let diff = relative_diff(a.rx_bps as f64, *theirs, 4.0 * 1024.0);
                net_diffs.push(diff);
                println!(
                    "{:<4} {:<6} {:>12} {:>12} {:>7.1}%",
                    "",
                    truncate(&a.name, 14),
                    human_bytes(a.rx_bps),
                    human_bytes(*theirs as u64),
                    diff * 100.0
                );
            }
        }
    }

    println!();
    report("disk throughput", &disk_diffs, 0.05);
    report("GPU utilisation", &gpu_diffs, 0.05);
    report("network rx", &net_diffs, 0.10);

    let mut io_ok = true;
    if io_size_mb > 0 {
        io_ok = verify_real_io(io_size_mb)?;
    }

    let all_ok =
        check(&disk_diffs, 0.05) && check(&gpu_diffs, 0.05) && check(&net_diffs, 0.10) && io_ok;
    if all_ok {
        println!("\nG1: PASS");
    } else {
        println!("\nG1: ADA SELISIH DI LUAR AMBANG");
    }
    Ok(())
}

/// Tulis file sebesar `size_mb` sambil mengakumulasi angka write yang dilaporkan
/// collector. Ini satu-satunya cara memverifikasi bahwa angka disk benar-benar
/// mengukur I/O nyata, bukan hanya konsisten dengan PDH.
///
/// Penulisan dilakukan di thread terpisah karena `sync_all()` bisa memakan
/// waktu jauh lebih lama daripada `write()` (data tertahan di page cache).
/// Sampling harus mencakup seluruh jendela, termasuk flush ke disk.
fn verify_real_io(size_mb: u64) -> anyhow::Result<bool> {
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    let path = std::env::temp_dir().join("core-pulse-verify-io.bin");
    let total_bytes = size_mb * 1024 * 1024;
    let written = Arc::new(AtomicU64::new(0));
    let done = Arc::new(AtomicBool::new(false));

    let mut collector = Collector::new();
    let _ = collector.sample();
    std::thread::sleep(Duration::from_millis(1100));
    let _ = collector.sample();

    let writer_path = path.clone();
    let writer_written = Arc::clone(&written);
    let writer_done = Arc::clone(&done);
    let handle = std::thread::spawn(move || -> std::io::Result<()> {
        let chunk = vec![0xA5u8; 1024 * 1024];
        let mut file = std::fs::File::create(&writer_path)?;
        let mut total = 0u64;
        while total < total_bytes {
            file.write_all(&chunk)?;
            total += chunk.len() as u64;
            writer_written.store(total, Ordering::Relaxed);
        }
        file.flush()?;
        file.sync_all()?;
        writer_done.store(true, Ordering::Relaxed);
        Ok(())
    });

    let start = Instant::now();
    let mut reported = 0f64;
    let mut samples = 0u32;
    let mut peak: u64 = 0;

    // Sampling berlanjut sampai penulis selesai + 400 ms ekor, supaya flush
    // akhir ikut terukur.
    let mut idle_since: Option<Instant> = None;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let snap = collector.sample();
        let rate: u64 = snap.disks.iter().map(|d| d.write_bps).sum();
        let secs = snap.interval_ms.max(1) as f64 / 1000.0;
        reported += rate as f64 * secs;
        peak = peak.max(rate);
        samples += 1;

        if done.load(Ordering::Relaxed) {
            let since = *idle_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(400) {
                break;
            }
        }
        if start.elapsed() > Duration::from_secs(60) {
            break;
        }
    }
    let elapsed = start.elapsed();
    let _ = handle.join();
    let total_written = written.load(Ordering::Relaxed);
    let _ = std::fs::remove_file(&path);

    if samples == 0 || total_written == 0 {
        println!("\nReal I/O: skipped (no samples)");
        return Ok(true);
    }

    let ratio = reported / total_written as f64;
    println!(
        "\nI/O nyata: {:.0} MB ditulis dalam {:.2} s ({} sampel, puncak {}/s)",
        total_written as f64 / 1e6,
        elapsed.as_secs_f64(),
        samples,
        human_bytes(peak)
    );
    println!(
        "  terakumulasi dari collector: {:.0} MB  (rasio {:.2}× terhadap byte ditulis)",
        reported / 1e6,
        ratio
    );
    // Rasio wajar 0.5–2.0: PDH menghitung semua I/O disk pada jendela itu
    // (termasuk tulisan lain di sistem), jadi sedikit di atas 1 itu normal.
    // Di bawah 0.5 berarti kita melewatkan I/O; jauh di atas 2 berarti kita
    // menjumlahkan instance ganda (mis. _Total).
    let ok = (0.5..=2.0).contains(&ratio);
    println!("  {}", if ok { "PASS" } else { "FAIL" });
    Ok(ok)
}

/// Selisih ternormalisasi dengan lantai noise.
///
/// Dua pengukuran yang diambil beberapa milidetik berbeda akan selalu berbeda
/// ketika nilainya kecil (mis. 0 B/s vs 800 B/s). Karena itu selisih absolut di
/// bawah `floor` dianggap nol, dan di atasnya dinormalkan terhadap nilai terbesar.
fn relative_diff(ours: f64, theirs: f64, floor: f64) -> f64 {
    let abs = (ours - theirs).abs();
    if abs <= floor {
        return 0.0;
    }
    let denom = ours.abs().max(theirs.abs()).max(floor);
    abs / denom
}

fn check(values: &[f64], threshold: f64) -> bool {
    values.iter().all(|d| *d <= threshold)
}

fn report(label: &str, values: &[f64], threshold: f64) {
    if values.is_empty() {
        println!("{label:<18} no samples");
        return;
    }
    let max = values.iter().cloned().fold(0.0f64, f64::max);
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    let status = if max <= threshold { "PASS" } else { "FAIL" };
    println!(
        "{label:<18} selisih rata-rata {:>5.2}%  maksimum {:>5.2}%  ambang {:>4.0}%  {status}",
        avg * 100.0,
        max * 100.0,
        threshold * 100.0
    );
}

fn compare(seconds: u64) -> anyhow::Result<()> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;

    fn system_times() -> Option<(u64, u64, u64)> {
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: ketiga out-param valid dan ditulis penuh oleh API.
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_err() {
            return None;
        }
        let f = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        Some((f(idle), f(kernel), f(user)))
    }

    let mut collector = Collector::new();
    let _ = collector.sample();
    let mut prev = system_times().ok_or_else(|| anyhow::anyhow!("GetSystemTimes failed"))?;

    let mut diffs = Vec::new();
    println!(
        "{:<6} {:>10} {:>10} {:>8}",
        "t", "ours", "GetSystemTimes", "diff"
    );
    for i in 1..=seconds.max(1) {
        std::thread::sleep(Duration::from_secs(1));
        let snap = collector.sample();
        let now = system_times().ok_or_else(|| anyhow::anyhow!("GetSystemTimes failed"))?;

        let d_idle = now.0.saturating_sub(prev.0) as f64;
        let d_kernel = now.1.saturating_sub(prev.1) as f64;
        let d_user = now.2.saturating_sub(prev.2) as f64;
        // GetSystemTimes: kernel sudah termasuk idle.
        let d_total = d_kernel + d_user;
        let busy = if d_total > 0.0 {
            (1.0 - d_idle / d_total) * 100.0
        } else {
            0.0
        };
        prev = now;

        let ours = snap.cpu.total_percent as f64;
        let diff = ours - busy;
        diffs.push(diff.abs());
        println!("{i:<6} {ours:>10.2} {busy:>10.2} {diff:>8.2}");
    }

    let avg = diffs.iter().sum::<f64>() / diffs.len().max(1) as f64;
    let max = diffs.iter().cloned().fold(0.0f64, f64::max);
    println!("\nselisih rata-rata: {avg:.2}%  maksimum: {max:.2}%");
    if max <= 2.0 {
        println!("G0 CPU: PASS (<= 2%)");
    } else if max <= 5.0 {
        println!("G0 CPU: marginal (2-5%), perlu investigasi");
    } else {
        println!("G0 CPU: FAIL (> 5%)");
    }
    Ok(())
}

fn record(out: &str, samples: u32) -> anyhow::Result<()> {
    let mut collector = Collector::new();
    let mut all = Vec::new();
    for _ in 0..samples.max(1) {
        all.push(collector.sample());
        std::thread::sleep(Duration::from_millis(1000));
    }
    let json = serde_json::to_string_pretty(&all)?;
    std::fs::write(out, json)?;
    println!("{} snapshot ditulis ke {out}", all.len());
    Ok(())
}
