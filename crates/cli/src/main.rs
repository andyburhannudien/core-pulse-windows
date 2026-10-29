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

    // Sampel pertama hanya untuk mengisi baseline delta.
    let first = collector.sample();
    if json && watch.is_none() {
        print_json(&first)?;
        return Ok(());
    }

    if let Some(interval_str) = watch {
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
            if interval > Duration::ZERO && snap.seq > 0 && at_exit_requested() {
                break;
            }
        }
    } else {
        std::thread::sleep(Duration::from_millis(500));
        let snap = collector.sample();
        if json {
            print_json(&snap)?;
        } else {
            print_table(&snap, limit, sort);
        }
    }
    Ok(())
}

fn at_exit_requested() -> bool {
    false
}

fn print_json(snap: &Snapshot) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(snap)?);
    Ok(())
}

fn print_table(snap: &Snapshot, limit: usize, sort: &str) {
    let mut rows: Vec<&core_pulse_core::ProcessRow> = snap.processes.iter().collect();
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
        "{:<28} {:>7} {:>7} {:>12} {:>7} {:>7}",
        "NAME", "PID", "CPU%", "MEM(MB)", "THREADS", "HANDLES"
    );
    for r in rows.into_iter().take(limit) {
        println!(
            "{:<28} {:>7} {:>7.1} {:>12.1} {:>7} {:>7}",
            truncate(&r.name, 28),
            r.pid,
            r.cpu_percent,
            r.mem_private_bytes as f64 / 1e6,
            r.threads,
            r.handles
        );
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
    Ok(())
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
