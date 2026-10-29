# ADR-0001 — egui/eframe sebagai stack UI

**Status:** Diterima
**Tanggal:** 2026-10-29

## Konteks

Aplikasi ini menampilkan data tabular padat (ratusan proses, 11 kolom, refresh
1 Hz) plus beberapa grafik real-time. Target mesin kelas terendah adalah
i5-9500T dengan **8 GB RAM** (lihat [ENVIRONMENT-AUDIT](../ENVIRONMENT-AUDIT.md)).
Aplikasi monitoring yang memakan 300 MB akan dituduh sebagai bagian dari masalah.

Kandidat:

| Stack | Versi saat evaluasi | Model memori | Catatan |
|---|---|---|---|
| egui/eframe | 0.37.0 | satu proses, satu window, GPU via wgpu/glow | murni Rust |
| Tauri 2 | 2.12.5 | proses UI + 2–4 proses WebView2 | butuh Node saat build |
| Slint | 1.18.1 | satu proses | lisensi perlu diputuskan |
| iced | — | satu proses | tabel virtualized belum matang |

## Keputusan

Pakai **egui/eframe 0.37** dengan `egui_extras 0.37` (tabel) dan `egui_plot 0.38`
(kompatibel dengan egui 0.37 — diverifikasi dari metadata dependensi crates.io).

## Alasan

1. **RAM.** egui tidak butuh WebView2. Perkiraan 60–90 MB vs 150–300 MB untuk
   Tauri. Pada mesin 8 GB, ini bukan detail.
2. **Satu toolchain.** `cargo build` cukup. Tidak ada Node, tidak ada bundler,
   tidak ada IPC lintas bahasa untuk data per-proses 1 Hz.
3. **Immediate mode cocok untuk data streaming.** Tidak ada diffing virtual DOM;
   snapshot baru langsung dirender. Sorting dan filter 400 baris selesai dalam
   satu frame.
4. **Tabel virtualized siap pakai.** `TableBuilder::body.rows()` hanya merender
   baris yang terlihat — persis kebutuhan kita.
5. **Kontrol penuh atas perilaku Windows.** Tray, DPI, single instance, dan
   akses Win32 dilakukan langsung, tanpa lapisan framework yang menyembunyikan.

## Alternatif yang ditolak

- **Tauri 2**: UI paling menarik secara visual, dan ekosistem web kaya. Ditolak
  karena 3+ proses WebView2, biaya RAM, dan IPC JSON untuk 400 baris × 1 Hz
  menambah latensi serta kompleksitas. Kalau suatu hari prioritas berubah menjadi
  "tampilan paling cantik", ini pilihan pertama untuk dievaluasi ulang.
- **Slint**: bagus dan ringan, tapi lisensinya (GPL / komersial / atribusi)
  menambah keputusan non-teknis yang tidak perlu di v1.
- **iced**: arsitektur Elm menarik, tapi widget tabel dengan virtualisasi dan
  sorting belum setara `egui_extras` saat evaluasi.

## Konsekuensi

Positif:

- Biner kecil (< 12 MB), startup cepat, RAM rendah.
- Iterasi cepat: satu bahasa, satu build.
- Grafik dan tabel berbagi satu render pass.

Negatif:

- Tampilan tidak native Win11 (tidak ada Mica, tidak ada kontrol Fluent).
  Diterima: ini alat teknis, bukan aplikasi konsumen.
- Aksesibilitas terbatas (screen reader kurang baik). Diterima untuk v1, dicatat
  sebagai batasan di [UI-SPEC §10](../UI-SPEC.md#10-aksesibilitas-dan-dpi).
- Text input IME/CJK di egui perlu perhatian khusus saat implementasi.

## Pemicu untuk meninjau ulang

- Kalau muncul kebutuhan distribusi ke pengguna non-teknis yang menuntut UI native.
- Kalau egui mengalami regresi performa pada tabel besar.
- Kalau aksesibilitas menjadi syarat (mis. pengadaan enterprise).
