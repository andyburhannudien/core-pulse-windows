# Pertanyaan terbuka

Keputusan yang belum dijawab. Saya mencatat asumsi yang dipakai sementara, supaya
kalau jawabannya berbeda, dampaknya jelas dan bisa diperbaiki tanpa membongkar
banyak hal.

## Q1 — Target distribusi

**Pertanyaan:** internal/personal saja, atau rilis publik (OSS/komersial)?

**Asumsi sementara:** internal dulu, tapi dokumentasi ditulis supaya layak publik.

| Kalau internal | Kalau publik |
|---|---|
| Tanpa code signing | Butuh sertifikat (OV ~$200–400/tahun, atau EV untuk SmartScreen instan) |
| Installer NSIS sederhana | Installer + update channel bertanda tangan |
| Threat model cukup untuk diri sendiri | Threat model formal + kebijakan disclosure |
| Tanpa LICENSE | Perlu pilih lisensi (MIT/Apache-2.0 paling aman untuk Rust) |
| P5 cukup 2–3 hari | P5 menjadi ~1 minggu |

**Dampak kalau berubah:** menambah pekerjaan di P5, tidak mengubah arsitektur.

## Q2 — Hubungan dengan `disk-pulse`

**Pertanyaan:** repo terpisah, atau digabung dengan `disk-pulse`?

**Asumsi sementara:** repo terpisah di `D:\Projects\core-pulse` (ADR-0007).

**Dampak kalau berubah:** kalau harus digabung, struktur workspace dan CI perlu
disusun ulang sebelum P1. Ini satu-satunya pertanyaan yang berdampak besar pada
struktur, jadi sebaiknya dijawab sebelum skeleton dibuat.

## Q3 — Waktu helper elevated

**Pertanyaan:** helper dibuat di P4 sesuai roadmap, atau ditunda sampai P1–P2
terbukti stabil?

**Asumsi sementara:** P4, sesuai roadmap.

| Dibuat di P4 | Ditunda |
|---|---|
| Suhu disk tersedia lebih awal | Suhu disk baru ada setelah v1.0 |
| Permukaan serangan ada lebih awal | Lebih lama tanpa fitur elevated |
| Lebih banyak hal untuk diuji bersamaan | Rilis lebih cepat, risiko lebih kecil |

**Dampak kalau berubah:** hanya urutan kerja; trait `SensorProvider` sudah
menyediakan tempat untuk backend helper.

## Q4 — Cakupan sensor Tier 3

**Pertanyaan:** bridge ke LibreHardwareMonitor/HWiNFO masuk v1 atau tidak?

**Asumsi sementara:** masuk v1, nonaktif default.

Kalau tidak: di mesin tanpa ACPI thermal zone (termasuk mesin pengembangan ini),
tidak ada satu pun sumber suhu CPU. Fitur "monitoring suhu" praktis hanya berarti
suhu disk. Itu melemahkan nilai jual utama aplikasi.

## Q5 — Bahasa UI

**Pertanyaan:** Indonesia, Inggris, atau keduanya?

**Asumsi sementara:** Inggris untuk istilah teknis (nama kolom, status proses),
Indonesia untuk teks penjelas dan dialog.

**Dampak:** kalau perlu dua bahasa penuh, perlu lapisan i18n sejak P1 (lebih murah
daripada retrofit di P5). `egui` tidak punya i18n bawaan; pendekatan termurah
adalah tabel `&'static str` per bahasa yang dipilih saat startup.

## Q6 — Perilaku saat menutup window

**Pertanyaan:** tutup = keluar, atau minimize ke tray?

**Asumsi sementara:** tutup = minimize ke tray, dengan opsi "keluar saat tutup"
di Settings. Alasan: aplikasi monitoring biasanya diharapkan tetap merekam riwayat.

**Dampak:** kecil, tapi memengaruhi persepsi "aplikasi ini boros" kalau pengguna
tidak sadar masih berjalan. Status bar dan tray harus jelas.
