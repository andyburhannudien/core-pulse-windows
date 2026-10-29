# Spesifikasi UI

UI memakai `egui/eframe` 0.37 dengan `egui_extras` (tabel) dan `egui_plot`
(grafik). Dokumen ini mendefinisikan apa yang dilihat pengguna, supaya
implementasi tidak mengarang sendiri.

## 1. Prinsip

1. **Padat, bukan lapang.** Ini alat diagnostik; informasi per piksel lebih
   berharga daripada whitespace.
2. **Jujur.** Nilai yang tidak diketahui tampil sebagai `—` dengan alasan, bukan
   `0`.
3. **Cepat.** Semua angka bisa disortir dan dicari tanpa jeda terasa.
4. **Tidak mengejutkan.** Aksi destruktif selalu dikonfirmasi dan selalu
   menyebut nama + PID target.

## 2. Kerangka window

```
+--------------------------------------------------------------------------+
| [Proses] [Kinerja] [Sensor] [Riwayat] [Mulai] [Layanan] [Pengguna]        |  nav bar
+--------------------------------------------------------------------------+
| cari: [_______________]   [x] tampilkan proses sistem     400 proses      |  toolbar
+--------------------------------------------------------------------------+
| Nama            PID    CPU   Memori   Disk   Jaringan   GPU   Status      |  header
| chrome.exe     4820   4.2%   512 MB    0 B/s    12 KB/s  1.8%  Berjalan   |
|  ...                                                                     |
|                                                                          |
+--------------------------------------------------------------------------+
| CPU 12%  Mem 41%  Disk 3%  Net 0.4 MB/s  GPU 2%  CPU 62C*  Disk 38C       |  status bar
+--------------------------------------------------------------------------+
```

`*` pada suhu menandakan `confidence != Direct` (lihat [SENSORS.md](SENSORS.md)).
Kalau tidak ada sensor sama sekali, bagian suhu di status bar menampilkan
`Suhu: tidak tersedia` dan bisa diklik untuk membuka penjelasan.

Ukuran default: 1200 × 780, minimum 900 × 560. Posisi dan ukuran window disimpan
per pengguna.

## 3. Tab Proses

### Kolom (default)

| Kolom | Lebar | Isi | Sortable |
|---|---|---|---|
| Nama | fleksibel (min 200) | nama + ikon 16 px | ya |
| PID | 70 | — | ya |
| Status | 100 | Berjalan / Ditangguhkan / Tidak merespons | ya |
| CPU | 80 | % dari total sistem | ya |
| Memori | 100 | private working set | ya |
| Disk | 100 | byte/detik | ya |
| Jaringan | 110 | byte/detik (butuh ETW) | ya |
| GPU | 70 | % (butuh PDH GPU) | ya |
| Pengguna | 140 | `DOMAIN\user` | ya |
| Integritas | 90 | Sedang / Tinggi / Sistem | ya |
| Tertanda tangan | 90 | ikon ✓ / ✗ / ? | ya |

Kolom bisa disembunyikan lewat menu klik kanan header; pilihan disimpan.
Kolom yang sumbernya tidak tersedia (mis. Jaringan tanpa admin) **disembunyikan
otomatis** pada run pertama, dan bisa dimunculkan manual dengan status "butuh
hak administrator" di header-nya.

### Perilaku

- Virtualized: hanya baris yang terlihat yang dirender (`TableBuilder::body.rows`).
- Sorting: klik header untuk toggle asc/desc/none. Default CPU desc.
- Search: substring case-insensitive pada nama, path, dan PID. Debounce 120 ms.
- Tree: toggle "Tampilkan hierarki" mengelompokkan proses anak di bawah induk;
  induk bisa di-collapse. Agregat CPU/RAM induk = proses itu sendiri + anak yang
  ter-collapse (diberi penanda `+`).
- Selection: satu atau banyak baris (Ctrl/Shift). Aksi berlaku ke semua yang dipilih.
- Klik kanan: menu kontekstual (lihat §7).
- Double click: buka dialog properti proses.
- Proses yang baru muncul diberi highlight halus selama 3 detik; yang hilang
  langsung dihapus (tanpa animasi yang mengganggu sorting).

### Keadaan kosong dan terdegradasi

| Keadaan | Tampilan |
|---|---|
| Snapshot pertama belum ada | spinner + "Membaca sistem…" (maks 2 detik) |
| Enumerasi proses gagal | panel error dengan tombol "Coba lagi" + kode error |
| Kolom GPU kosong (VM) | kolom disembunyikan, catatan di tooltip |
| Proses sistem disembunyikan | baris footer "12 proses sistem disembunyikan" |

## 4. Tab Kinerja

Grid dua kolom berisi kartu:

| Kartu | Isi |
|---|---|
| CPU | total %, 6 grafik per-core mini, frekuensi, queue, context switch |
| Memori | donut komposisi (in use / modified / standby / free) + commit |
| Disk | per disk: busy %, throughput, response time, queue |
| Jaringan | per adapter: rx/tx, link speed, status |
| GPU | per adapter: utilization, memori khusus/bersama, suhu |
| Suhu | kartu ringkas semua sensor suhu + sparkline 60 detik |

Grafik: 60 titik × 1 Hz, garis + area semi-transparan. Skala Y auto dengan
minimum 100% untuk CPU. Saat window tidak fokus, redraw turun ke 1 fps.

## 5. Tab Sensor

Tampilan pohon ala LibreHardwareMonitor, tapi hanya untuk sumber yang benar-benar
ada:

```
CPU
  └─ (tidak tersedia — firmware tidak mengekspos thermal zone)   [Kenapa?]
Disk
  ├─ ST1000LM049  38 °C   SMART
  └─ ST500LT012   36 °C   SMART
GPU
  └─ Intel UHD 630  52 °C   IGCL (pihak ketiga)
```

Setiap baris menampilkan nilai sekarang, min, max, rata-rata sesi, dan sumber.
Tombol "Rekam CSV" untuk ekspor. Baris yang tidak tersedia tetap ditampilkan
(dengan alasan) — menyembunyikannya membuat pengguna bingung kenapa tidak ada
suhu CPU.

## 6. Tab lain

- **Riwayat**: grafik 24 jam / 7 hari / 30 hari dari SQLite; tabel "pemakan CPU
  teratas per jam"; daftar alert.
- **Mulai (Startup)**: entri Registry Run + folder Startup + scheduled task,
  dengan status aktif/nonaktif dan dampak (estimasi dari riwayat).
- **Layanan**: daftar service, status, start type, tombol start/stop (butuh
  elevasi; kalau tidak ada, tombol disabled dengan tooltip).
- **Pengguna**: sesi aktif, proses per sesi, konsumsi agregat.

## 7. Menu kontekstual proses

| Item | Butuh elevasi | Konfirmasi |
|---|---|---|
| Akhiri tugas | kadang | ya (nama + PID) |
| Akhiri pohon proses | kadang | ya (jumlah proses anak) |
| Tangguhkan / Lanjutkan | ya | tidak |
| Prioritas (submenu) | ya untuk naik | tidak |
| Afinitas CPU | ya | tidak |
| Mode efisiensi | tidak | tidak |
| Buka lokasi file | tidak | tidak |
| Properti | tidak | tidak |
| Salin (nama / PID / path / command line) | tidak | tidak |

Item yang butuh elevasi dan tidak tersedia: disabled + tooltip "Butuh hak
administrator". Bukan hilang, supaya pengguna tahu fitur itu ada.

## 8. Keyboard

| Shortcut | Aksi |
|---|---|
| `Ctrl+F` | fokus ke kolom pencarian |
| `Ctrl+C` | salin baris terpilih (TSV) |
| `Delete` | akhiri tugas (dengan konfirmasi) |
| `F5` | paksa refresh snapshot |
| `Ctrl+1..7` | pindah tab |
| `Ctrl+,` | pengaturan |
| `Ctrl+Shift+E` | buka log folder |
| `Esc` | tutup dialog / bersihkan selection |
| `Ctrl+Q` | keluar (atau ke tray kalau "close to tray" aktif) |

## 9. Tema dan tipografi

Token warna (dipakai konsisten, tidak ada hardcode di view):

| Token | Gelap | Terang |
|---|---|---|
| `bg_base` | `#1a1a1c` | `#f7f7f8` |
| `bg_panel` | `#232326` | `#ffffff` |
| `bg_row_alt` | `#1f1f22` | `#f2f2f4` |
| `border` | `#33333a` | `#dcdce0` |
| `text_primary` | `#e8e8ea` | `#1a1a1c` |
| `text_muted` | `#9a9aa2` | `#6b6b72` |
| `accent` | `#4a9eff` | `#0a6cd8` |
| `warn` | `#e0a030` | `#a86a00` |
| `crit` | `#e05a5a` | `#c03030` |
| `ok` | `#4ac07a` | `#1a7a45` |

Font: Segoe UI Variable (sistem) dengan fallback Segoe UI; ukuran dasar 13 px,
tabel 12,5 px, angka memakai tabular figures supaya kolom tidak bergoyang.
Ikuti tema sistem secara default, dengan override manual.

## 10. Aksesibilitas dan DPI

- Per-monitor DPI aware v2; layout diuji di 100/125/150/200%.
- Semua aksi bisa dicapai lewat keyboard.
- Kontras teks minimal 4.5:1 pada kedua tema.
- Status tidak hanya dibedakan warna: ikon/tanda pada alert dan status proses.
- Tooltip muncul setelah 400 ms, tidak menutupi baris yang sedang dibaca.

## 12. Catatan implementasi (egui 0.37)

Beberapa detail berubah dibanding versi egui sebelumnya. Ini catatan untuk
siapa pun yang menyentuh `crates/ui`:

- `eframe::App` sekarang punya `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame)`.
  Tidak ada lagi `fn update(&mut self, ctx, frame)`; panel dibuat dari `Ui`, bukan
  dari `Context`.
- Panel atas/bawah: `egui::Panel::top(id)` / `egui::Panel::bottom(id)`.
  `TopBottomPanel` dan `SidePanel` sudah tidak ada.
- `egui::CentralPanel::default().show(ui, ...)` menerima `&mut Ui`.
- Tabel memakai `.id_salt("process_table")` supaya state kolom tidak bertabrakan
  dengan tabel lain di masa depan.
- Tabel diset `resizable(false)` dan kolom nama `clip(true)` — dengan
  `resizable(true)` kolom bisa melebar melewati lebar window dan header
  terpotong.
- Sparkline digambar manual (`spark_points` + `egui::Shape::line`), bukan
  `egui_plot`. Untuk 60 titik hasilnya identik dan tidak menambah dependensi.
- `--tab performance` (argumen CLI) membuka langsung di tab Kinerja, dipakai untuk
  screenshot otomatis dan uji manual.
- Renderer: fitur `glow`. Lihat [COMPATIBILITY §5](COMPATIBILITY.md#5-catatan-build-dan-renderer).

Status implementasi per bagian spesifikasi ini:

| Bagian | Status |
|---|---|
| §2 kerangka window (nav, toolbar, status bar) | **Selesai** |
| §3 tab Proses (kolom, sort, search, virtualized) | **Selesai** (kolom Jaringan/GPU/Disk menyusul di P1) |
| §4 tab Kinerja (CPU, memori, grafik) | **Sebagian** — disk/net/GPU menyusul di P1 |
| §5 tab Sensor | Belum (P3) |
| §6 tab lain | Belum (P2) |
| §7 menu kontekstual | Belum (P2) |
| §8 keyboard | Belum (P2) |
| §9 tema | Sebagian — tema gelap tetap; token belum dipusatkan |
| §10 aksesibilitas | Belum diuji |
| §11 tray | Belum (P4) |

## 11. Tray

Menu: Tampilkan, Sensor ringkas (submenu), Mode senyap (matikan notifikasi),
Mulai saat login (checkbox), Keluar. Klik ganda membuka window. Ikon berubah
warna kalau ada alert aktif — satu-satunya indikasi visual di luar window.
