//! Format angka untuk UI. Semua fungsi di sini murni dan bisa diuji.

/// Byte → "1.2 KB" / "345 MB" / "1.1 GB". Basis 1024, satu desimal.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Persentase dengan satu desimal.
pub fn percent(v: f32) -> String {
    format!("{v:.1}%")
}

/// Suhu: `None` → "—" (tidak diketahui, bukan nol).
pub fn temp_c(v: Option<f32>) -> String {
    match v {
        Some(t) => format!("{t:.0} °C"),
        None => "—".to_string(),
    }
}

/// Durasi detik → "3h 12m" / "12m 04s" / "45s".
pub fn uptime(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3_600;
    let m = (secs % 3_600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{d}h {h:02}m")
    } else if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// Potong string ke lebar karakter, dengan elipsis.
pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_scales() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.0 KB");
        assert_eq!(bytes(1536), "1.5 KB");
        assert_eq!(bytes(1024 * 1024), "1.0 MB");
        assert_eq!(bytes(1024 * 1024 * 1024), "1.0 GB");
    }

    #[test]
    fn unknown_temp_is_dash() {
        assert_eq!(temp_c(None), "—");
        assert_eq!(temp_c(Some(0.0)), "0 °C");
        assert_eq!(temp_c(Some(62.4)), "62 °C");
    }

    #[test]
    fn uptime_formats() {
        assert_eq!(uptime(45), "45s");
        assert_eq!(uptime(125), "2m 05s");
        assert_eq!(uptime(3_600), "1h 00m");
        assert_eq!(uptime(90_000), "1h 01m");
    }

    #[test]
    fn truncate_respects_chars_not_bytes() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
        // Multi-byte aman.
        assert_eq!(truncate("日本語テスト", 3), "日本…");
    }
}
