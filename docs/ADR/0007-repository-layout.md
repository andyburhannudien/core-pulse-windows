# ADR-0007 — Repository terpisah dari `disk-pulse`

**Status:** Diterima (dengan satu asumsi yang perlu dikonfirmasi)
**Tanggal:** 2026-10-29

## Konteks

`D:\Projects\disk-pulse` sudah ada: pemeriksa kesehatan disk berbasis WPF + service
C#, dengan parser SMART NVMe/ATA yang sudah terverifikasi unit test. Aplikasi baru
(`core-pulse`) butuh sebagian logika itu (SMART, alert engine, pola service+pipe)
tapi dibangun dengan Rust dan egui.

Pilihan:

1. Satu repository: `core-pulse` sebagai crate di dalam `disk-pulse`.
2. Repository baru: `D:\Projects\core-pulse`, kode SMART di-port.
3. Repository baru + `disk-pulse` sebagai dependensi/library bersama.

## Keputusan

**Repository baru** di `D:\Projects\core-pulse`, sibling terhadap `disk-pulse`,
dengan port kode (bukan referensi silang).

## Alasan

1. **Toolchain berbeda.** `disk-pulse` adalah .NET/WPF; `core-pulse` adalah Rust.
   Menggabungkan dua toolchain dalam satu repo memaksa CI, `.gitignore`, dan
   konvensi commit yang berbeda hidup bersama tanpa manfaat.
2. **Siklus rilis berbeda.** `disk-pulse` stabil; `core-pulse` akan berubah cepat
   selama P0–P4. Menyatukan keduanya membuat perubahan besar pada satu proyek
   mengganggu yang lain.
3. **Port lebih murah daripada binding.** Logika yang dibutuhkan dari `disk-pulse`
   adalah parser byte-offset dan scoring — beberapa ratus baris yang justru lebih
   mudah ditulis ulang dalam Rust dengan test yang sudah ada sebagai spesifikasi.
   Membuat FFI .NET↔Rust untuk itu menambah kompleksitas yang tidak sepadan.
4. **Opsi masa depan tetap terbuka.** Kalau nanti ingin satu paket ("pulse suite"),
   kedua repo bisa digabung lewat submodule atau installer bersama. Memisah lebih
   mudah daripada memisahkan yang sudah menyatu.

## Asumsi yang perlu dikonfirmasi

Keputusan ini mengasumsikan `core-pulse` adalah **produk terpisah**, bukan fitur
baru di dalam `disk-pulse`. Kalau tujuan sebenarnya adalah "disk-pulse versi
lengkap dengan CPU/memori", maka opsi 1 atau 3 lebih tepat, dan ADR ini harus
diganti.

Lihat [OPEN-QUESTIONS.md](../OPEN-QUESTIONS.md).

## Konsekuensi

Positif:

- Kebebasan memilih toolchain, CI, dan ritme rilis.
- Port SMART menjadi kesempatan memperbaiki desain (Rust, test-first).
- Risiko: dua basis kode yang mirip bisa menyimpang.

Negatif:

- Perbaikan parser SMART tidak otomatis berlaku di kedua proyek. Mitigasi:
  fixture dan test korpus yang sama disalin ke kedua repo; kalau menyimpang,
  itu terdeteksi karena test memakai data identik.
- Tidak ada kode bersama sejak awal. Diterima: nilainya kecil, biaya koordinasinya
  lebih besar.
