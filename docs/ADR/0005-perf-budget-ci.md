# ADR-0005 — Budget performa sebagai gate CI

**Status:** Diterima
**Tanggal:** 2026-10-29

## Konteks

Aplikasi monitoring punya masalah khas: ia mengukur beban sistem sambil menjadi
beban sistem. Tanpa batas yang diukur otomatis, regresi performa masuk perlahan
dan baru terasa setelah rilis ("kenapa core-pulse pakai 4% CPU idle?").

Target mesin terendah: i5-9500T, 6 core, 8 GB RAM.

## Keputusan

Tetapkan budget numerik di [PERF.md](../PERF.md) dan **jadikan gate CI**, bukan
sekadar pedoman:

| Metrik | Batas keras |
|---|---|
| CPU idle | 1,0% |
| Private bytes UI | 100 MB |
| Enumerasi 400 proses | 50 ms |
| Satu siklus collector | 100 ms |
| Serialisasi snapshot | 10 ms |
| Ukuran exe | 20 MB |

Implementasi:

1. `core-pulse-cli bench --seconds 60` mencetak metrik yang sama dengan yang
   dipakai CI.
2. `criterion` untuk fungsi murni, dengan baseline tersimpan.
3. CI gagal kalau regresi > 20% dibanding baseline `main`, atau kalau batas keras
   terlampaui.

## Alasan

1. **Budget tanpa pengukuran adalah harapan.** Angka yang tidak diukur akan
   dilanggar tanpa ada yang sadar.
2. **Regresi performa bersifat akumulatif.** Satu `clone()` per baris tidak
   terasa; lima puluh di jalur render membuat aplikasi terasa berat.
3. **Konteks mesin rendah memaksa disiplin.** Kalau berjalan baik di 8 GB / 6 core,
   akan berjalan baik di mana saja.
4. **Metrik yang sama dipakai untuk dukungan.** Pengguna bisa menjalankan `bench`
   dan mengirim hasilnya saat melaporkan "aplikasi ini berat".

## Alternatif yang ditolak

- **Hanya mengandalkan review manual**: tidak konsisten, dan reviewer tidak bisa
  merasakan 20 ms tambahan per frame.
- **Benchmark absolut tanpa baseline**: berisik di VM CI; yang penting adalah tren.
- **Menetapkan budget lebih longgar supaya CI tidak merah**: ini awal dari
  penurunan kualitas. Batas keras tidak dinegosiasikan; kalau terlampaui, kode
  yang diperbaiki, bukan angkanya.

## Konsekuensi

Positif:

- Regresi tertangkap di PR, bukan di tangan pengguna.
- Keputusan optimasi punya dasar angka.
- `--bench` menjadi alat diagnostik yang berguna di luar CI.

Negatif:

- CI lebih lambat dan lebih kompleks.
- Runner Windows lebih bervariasi; ambang di CI dilonggarkan 2× dan hanya tren
  yang digerbangi ketat, sementara batas keras diukur pada rilis manual.
- Perlu disiplin menjaga baseline tetap diperbarui saat optimasi sengaja
  dilakukan.
