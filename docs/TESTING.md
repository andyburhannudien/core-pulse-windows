# Strategi testing

Prinsip: **logika yang bisa diuji tanpa Windows harus diuji tanpa Windows**, dan
logika yang butuh Windows harus diuji dengan fixture yang bisa diputar ulang.

Alasan praktis: mesin pengembangan punya 2 HDD SATA, tanpa NVMe, tanpa GPU
diskrit, tanpa thermal zone ACPI, dan tanpa hak admin. Kalau test bergantung pada
hardware itu, separuh kode tidak akan pernah teruji.

## 1. Piramida

```
        /\
       /  \      manual checklist (paritas vs Task Manager)
      /────\     integration live (opt-in, butuh mesin nyata)
     /      \    fixture replay (snapshot & payload rekaman)
    /────────\   unit test (parser, kalkulasi, state machine)
   /──────────\  compile-time (type system, clippy, deny)
```

Target kasar: 70% unit, 20% fixture, 10% live/manual. Bukan angka keramat, tapi
arahnya: jangan mengandalkan test manual untuk hal yang bisa diuji murni.

## 2. Unit test

Yang **wajib** punya unit test:

| Modul | Kasus |
|---|---|
| `sys::ntapi` parser | buffer terpotong, `NextEntryOffset` 0, nama proses non-UTF8, PID 0 |
| CPU% kalkulasi | delta waktu 0 (jangan bagi nol), counter wrap, core offline, interval tidak seragam |
| Memory pressure | batas 0%, 100%, commit > limit, nilai negatif dari API |
| SMART NVMe | health log 512 byte valid, temperature 0 K, byte terpotong |
| SMART ATA | atribut 0xC2/0xBE, threshold hilang, nilai 0 (sensor tidak ada) |
| Signature | path tidak ada, file tanpa signature, signature invalid, cache hit |
| `Snapshot` serialisasi | round-trip JSON, `None` tetap `null`, `schema` ikut |
| PID reuse guard | creation time beda 1 ms, 1 detik, sama persis |
| Alert engine | dedup, cooldown, flapping (naik-turun cepat), nilai kembali normal |
| Format angka | 1023 B → "1.0 KB", 1 GiB, pembulatan persen, locale |
| Riwayat SQLite | insert batch, query rentang, retensi, migrasi skema |

Aturan: setiap bug yang ditemukan di lapangan **wajib** menghasilkan satu unit
test regresi sebelum diperbaiki. Tanpa itu, bug akan kembali.

## 3. Fixture replay

Inti strategi ini. Setiap data dari OS direkam ke file, lalu collector diuji
dengan memutar rekaman itu.

```
tests/fixtures/
├── snapshot/
│   ├── win11-26100-idle.json          # 400 proses, mesin ini
│   ├── win11-26100-load.json          # saat build Rust berjalan
│   └── win10-19045-legacy.json
├── smart/
│   ├── nvme-health-samsung-980.bin    # 512 byte log page 0x02
│   ├── nvme-health-wd-sn770.bin
│   ├── ata-attrs-st1000lm049.bin
│   └── ata-attrs-corrupt.bin          # sengaja rusak, untuk fuzz seed
└── ipc/
    ├── sensor-update-v1.json
    └── malformed/                      # pesan cacat untuk test penolakan
```

Cara merekam: `core-pulse-cli record --out tests/fixtures/snapshot/x.json`.
Perintah ini ada di CLI sejak P1, supaya merekam fixture tidak pernah jadi
pekerjaan manual yang ditunda.

Aturan fixture:

- **Anonimisasi**: nama pengguna diganti `user`, nama komputer `PC`, path
  personal diganti `C:\Users\user\...`. Ada test yang memeriksa fixture tidak
  memuat nama asli.
- **Kecil**: satu snapshot ~400 proses ≈ 200 KB JSON; kompres dengan `zstd` kalau
  perlu, tapi jangan lebih dari 2 MB per file.
- **Berversi**: fixture menyimpan `schema` sehingga migrasi bisa diuji.

## 4. Test live (opt-in)

Test yang menyentuh mesin sungguhan diberi atribut `#[ignore]` atau digerbangi
variabel lingkungan, supaya `cargo test` biasa tetap deterministik:

```powershell
$env:CORE_PULSE_LIVE = "1"; cargo test --features live -- --ignored
```

Yang diuji live:

- Enumerasi proses mengembalikan > 0 dan mengandung PID 4.
- Total CPU% dalam rentang 0–100 × jumlah core.
- Suhu disk (kalau ada disk yang bisa dibaca) dalam rentang -10..90 °C.
- Helper: install → connect → data → uninstall, tanpa sisa service.
- UI smoke test: buka, tunggu 3 detik, tutup, exit code 0.

## 5. Fuzz

Target `cargo fuzz`:

| Target | Input | Tujuan |
|---|---|---|
| `nvme_log` | byte acak | tidak panic, tidak OOB |
| `ata_attrs` | byte acak | sama |
| `ipc_message` | byte acak | ditolak dengan error, bukan panic |
| `lhm_json` | JSON acak | parser tidak panik, tidak alokasi liar |

Jalankan 10 menit di CI nightly, dan simpan corpus yang ditemukan ke
`tests/fixtures/fuzz-corpus/` agar regresi tertangkap.

## 6. Checklist manual (paritas vs Task Manager)

Dijalankan sebelum setiap gate P1 dan sebelum rilis. Ini satu-satunya cara
memverifikasi "angkanya benar".

| # | Langkah | Ekspektasi |
|---|---|---|
| 1 | Buka Task Manager dan core-pulse berdampingan, tab Processes | jumlah proses sama |
| 2 | Urutkan CPU descending, diamkan 60 detik | urutan 5 teratas sama, selisih < 5% |
| 3 | Buka 5 tab Chrome baru | chrome.exe naik di keduanya, selisih < 10% |
| 4 | Copy file 2 GB | kolom Disk dan tab Performance mengikuti |
| 5 | Unduh file besar | kolom Network mengikuti |
| 6 | Putar video 4K | kolom GPU muncul dan masuk akal |
| 7 | Buka tab Performance → Memory | in use/standby/free cocok |
| 8 | End task Notepad | hilang dari keduanya |
| 9 | Jalankan proses sebagai admin | kolom elevated benar |
| 10 | Cabut/colok USB drive | volume muncul/hilang tanpa crash |
| 11 | Biarkan 30 menit idle | CPU < 1%, RAM stabil (tidak naik terus) |
| 12 | Matikan helper saat berjalan | UI tidak hang, status berubah |
| 13 | Ubah DPI ke 150% | layout tidak terpotong |
| 14 | Ganti tema terang/gelap | kontras teks tetap terbaca |
| 15 | Sleep lalu resume | sampling pulih, tidak ada lonjakan palsu |

Poin 15 penting: setelah resume, delta waktu bisa besar dan menghasilkan lonjakan
CPU% palsu. Collector wajib mendeteksi lompatan `interval_ms` > 5 detik dan
membuang sampel pertama.

## 7. CI

```yaml
# .github/workflows/ci.yml (ringkas)
jobs:
  check:      # windows-latest: fmt, clippy -D warnings, nextest, deny
  bench:      # windows-latest: criterion, bandingkan dengan baseline
  coverage:   # tarpaulin/llvm-cov, target 65% untuk core+sensors
  fuzz:       # nightly, 10 menit per target
  build:      # release build, ukur ukuran exe, upload artifact
```

Gate merge: `check` hijau + tidak ada regresi benchmark > 20% + coverage tidak
turun lebih dari 2 poin.

## 8. Kebijakan test flaky

Test flaky lebih buruk daripada tidak ada test, karena melatih orang mengabaikan
kegagalan.

- Test yang gagal intermiten diberi label `flaky` dan **dikarantina** dalam 48 jam.
- Kalau penyebabnya timing, ganti dengan sinkronisasi eksplisit, bukan `sleep`.
- Kalau penyebabnya hardware, pindahkan ke kategori live (opt-in).
- Test yang dikarantina lebih dari 2 minggu dihapus atau diperbaiki — tidak
  dibiarkan menggantung.
