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
    // PID 0 adalah idle: menampilkannya sebagai "proses paling rakus CPU" hanya
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
        "core-pulse  seq {}  {:.1}% CPU  {:.1}/{:.1} GB RAM (tekanan {})  {} proses",
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
        println!("\ndisk:");
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
        println!("\ngpu:");
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
        println!("\njaringan:");
        for n in &snap.net {
            println!(
                "  {:<24} {:>9}/s turun  {:>9}/s naik  {}",
                truncate(&n.name, 24),
                human_bytes(n.rx_bps),
                human_bytes(n.tx_bps),
                if n.is_up { "up" } else { "down" }
            );
        }
    }
    if !snap.degraded.is_empty() {
        println!("\ndegradasi:");
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
    println!("  Mesin       : {}", snap.host.computer_name);
    println!("  CPU         : {}", snap.host.cpu_brand);
    println!(
        "  Core        : {} fisik / {} logis @ {} MHz",
        snap.host.physical_cores, snap.host.logical_cores, snap.host.base_clock_mhz
    );
    println!(
        "  RAM         : {:.1} GB",
        snap.host.total_ram_bytes as f64 / 1e9
    );
    println!("  Elevated    : {}", snap.host.is_elevated);
    println!("  Proses      : {}", snap.processes.len());
    println!(
        "  Memori      : in use {:.2} GB, standby {:.2} GB, free {:.2} GB, commit {:.2}/{:.2} GB",
        snap.memory.in_use_bytes as f64 / 1e9,
        snap.memory.standby_bytes as f64 / 1e9,
        snap.memory.free_bytes as f64 / 1e9,
        snap.memory.commit_total_bytes as f64 / 1e9,
        snap.memory.commit_limit_bytes as f64 / 1e9,
    );

    println!("\n  Sumber data:");
    println!("    CPU per-core      : {}", ok(has_per_core(&snap)));
    println!(
        "    Memori komposisi  : {}",
        ok(snap.memory.standby_bytes > 0)
    );
    println!(
        "    Disk (PDH)        : {}",
        availability(snap.disks.len(), "disk")
    );
    println!(
        "    Jaringan          : {}",
        availability(snap.net.len(), "adapter")
    );
    println!(
        "    GPU (PDH)         : {}",
        availability(snap.gpus.len(), "adapter")
    );
    println!(
        "    Suhu ACPI/PDH     : {}",
        match snap
            .degraded
            .iter()
            .find(|d| d.area == core_pulse_core::Area::Sensors)
        {
            Some(d) => format!("tidak tersedia ({:?})", d.reason),
            None => "belum diprobe".into(),
        }
    );
    println!("    Suhu disk SMART   : belum diimplementasikan (P3)");
    println!("    Suhu GPU          : belum diimplementasikan (P3)");

    if !snap.disks.is_empty() {
        println!("\n  Disk fisik:");
        for d in &snap.disks {
            println!(
                "    #{} {:<26} {:?}/{:?} {:>9}{}  busy {:>5.1}%  R {:>9}/s  W {:>9}/s  q {:.2}",
                d.index,
                d.model,
                d.bus,
                d.media,
                human_bytes(d.size_bytes),
                if d.size_is_estimate {
                    " (estimasi)"
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
        println!("\n  Volume:");
        for v in &snap.volumes {
            println!(
                "    {:<4} {:<18} {:<6} {:>9} bebas / {:>9}  disk {:?}",
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
        println!("\n  GPU:");
        for g in &snap.gpus {
            println!(
                "    {:<34} {:?}  util {:>6}  mem khusus {:>9}  bersama {:>9}",
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

fn availability(count: usize, unit: &str) -> String {
    if count == 0 {
        "TIDAK tersedia".to_string()
    } else {
        format!("tersedia ({count} {unit})")
    }
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
    if v { "tersedia" } else { "TIDAK tersedia" }
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
        println!("BUDGET: LULUS");
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
    let mut ref_query = Query::new().ok_or_else(|| anyhow::anyhow!("PDH tidak tersedia"))?;
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
        println!("\nG1: LULUS");
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
        println!("\nI/O nyata: dilewati (tidak ada sampel)");
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
    println!("  {}", if ok { "LULUS" } else { "GAGAL" });
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
        println!("{label:<18} tidak ada sampel");
        return;
    }
    let max = values.iter().cloned().fold(0.0f64, f64::max);
    let avg = values.iter().sum::<f64>() / values.len() as f64;
    let status = if max <= threshold { "LULUS" } else { "GAGAL" };
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
    let mut prev = system_times().ok_or_else(|| anyhow::anyhow!("GetSystemTimes gagal"))?;

    let mut diffs = Vec::new();
    println!(
        "{:<6} {:>10} {:>10} {:>8}",
        "t", "ours", "GetSystemTimes", "diff"
    );
    for i in 1..=seconds.max(1) {
        std::thread::sleep(Duration::from_secs(1));
        let snap = collector.sample();
        let now = system_times().ok_or_else(|| anyhow::anyhow!("GetSystemTimes gagal"))?;

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
        println!("G0 CPU: LULUS (<= 2%)");
    } else if max <= 5.0 {
        println!("G0 CPU: marginal (2-5%), perlu investigasi");
    } else {
        println!("G0 CPU: GAGAL (> 5%)");
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
