# ADR-0008 — `Option` = tidak diketahui, tanpa angka palsu

**Status:** Diterima
**Tanggal:** 2026-10-29

## Konteks

Hampir semua sumber data di aplikasi ini bisa gagal atau tidak ada:

- thermal zone kosong (terbukti di mesin pengembangan),
- SMART butuh admin,
- NVML tidak ada kalau tidak ada GPU NVIDIA,
- ETW butuh admin,
- proses terproteksi menolak dibaca,
- counter PDH bisa kosong di VM.

Ada tiga cara menanganinya di kode dan di UI:

1. Pakai `0` sebagai pengganti "tidak diketahui" (paling umum di aplikasi serupa).
2. Sembunyikan fitur sepenuhnya kalau tidak tersedia.
3. Bedakan secara eksplisit antara "nol" dan "tidak diketahui", dan tampilkan
   alasan.

## Keputusan

Opsi 3, sebagai kontrak di seluruh lapisan:

- Model: `Option<T>` untuk setiap nilai yang mungkin tidak diketahui.
- JSON: `null` untuk tidak diketahui, `0` hanya untuk nol yang benar.
- SQLite: `NULL` untuk kolom yang tidak terukur.
- UI: `—` + alasan, bukan `0`.
- Log: `Degradation { area, reason, detail }` pada setiap snapshot.

## Alasan

1. **Kesalahan medis.** `0 °C` dan `0 B/s` bukan "tidak ada data" — itu nilai
   yang salah dan bisa memicu kesimpulan salah ("disk tidak aktif" padahal
   kolomnya tidak terukur).
2. **Alert palsu.** Alert berbasis ambang akan berperilaku aneh kalau `0` dipakai
   sebagai sentinel: suhu 0 °C bisa memicu alert "terlalu dingin", atau menutupi
   kegagalan sensor.
3. **Debugging.** `Degradation` dengan alasan spesifik (`NeedsElevation` vs
   `NotAvailable`) langsung memberi tahu pengguna tindakan apa yang mungkin.
4. **Kontrak API.** `core-pulse-cli --json` dipakai skrip. Konsumen harus bisa
   membedakan "nol" dari "tidak diukur"; kalau tidak, skrip akan salah hitung.
5. **Menghindari `unwrap()` yang menyamar.** Banyak bug berasal dari `unwrap_or(0)`
   yang menyembunyikan kegagalan. Tipe `Option` memaksa pemanggil memutuskan.

## Alternatif yang ditolak

- **`0` sebagai sentinel**: alasan di atas. Ditolak.
- **Menyembunyikan fitur**: pengguna tidak tahu bahwa fitur ada dan bisa
  diaktifkan. Ditolak; UI menampilkan baris dengan alasan.
- **`f32::NAN` sebagai sentinel**: menyebar ke perhitungan dan sulit di-debug.
  Ditolak.
- **`Result` di setiap field**: terlalu berat untuk data yang sebagian besar
  berhasil; `Option` + `Degradation` agregat sudah cukup.

## Konsekuensi

Positif:

- UI jujur; tidak ada klaim yang tidak bisa dipertanggungjawabkan.
- Bug sumber data terlihat jelas, bukan tersamar sebagai nol.
- Konsumen JSON bisa menulis logika yang benar.

Negatif:

- Kode lebih verbose (`Option` di mana-mana, `match`/`?`).
- Perlu disiplin di UI: setiap `Option` harus punya representasi visual, bukan
  `unwrap_or_default()`.
- Beberapa pengguna akan menganggap `—` sebagai bug. Dijawab dengan tombol
  "Kenapa?" yang menjelaskan kondisi mesin.

## Cara menegakkan

- Clippy lint `unwrap_used` dan `expect_used` diaktifkan untuk `core` dan
  `sensors` (diizinkan hanya dengan `#[allow]` beralasan).
- Review checklist: setiap field `Option` baru harus punya baris di tabel
  degradasi ([ARCHITECTURE §7](../ARCHITECTURE.md#7-jalur-degradasi)).
- Test: fixture snapshot dengan bagian yang hilang harus tetap menghasilkan UI
  yang valid (snapshot test pada model).
