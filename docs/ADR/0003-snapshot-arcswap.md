# ADR-0003 — Snapshot immutable + `ArcSwap`, satu sampler thread

**Status:** Diterima
**Tanggal:** 2026-10-29

## Konteks

Data harus mengalir dari syscall (mahal, blocking, bisa gagal) ke render loop
(60 fps, tidak boleh blocking). Pilihan sinkronisasi menentukan apakah UI terasa
mulus dan apakah kode tetap bisa dipahami.

Kandidat:

1. `Mutex<State>` — render mengunci state setiap frame.
2. Channel per bagian (`mpsc`) — UI mengumpulkan dari banyak channel.
3. `ArcSwap<Snapshot>` — sampler menaruh nilai immutable, UI `load()`.

## Keputusan

Gunakan **`ArcSwap<Snapshot>`** dengan **satu sampler thread** sebagai satu-satunya
penulis. Snapshot immutable, dibagikan lewat `Arc`. Grafik menyimpan ring buffer
sendiri di UI, diisi saat `snapshot.seq` berubah.

## Alasan

1. **Render tidak pernah menunggu.** `load()` adalah atomic load + refcount, bukan
   lock. Tidak ada kemungkinan frame tertahan karena sampler sedang enumerasi
   proses (yang bisa 3–30 ms).
2. **Tidak ada data robek (torn read).** Snapshot selalu konsisten secara internal:
   total CPU dan per-core berasal dari sampel yang sama.
3. **Satu penulis menghilangkan kontensi.** Tidak ada dua thread yang saling
   menimpa field.
4. **Mudah diuji.** Collector menghasilkan `Snapshot` murni; test bisa
   membandingkan dua snapshot tanpa menyentuh thread.
5. **CLI memakai jalur yang sama.** `core-pulse-cli` juga membaca `ArcSwap` — jadi
   UI dan CLI tidak bisa "melihat" data yang berbeda.

## Alternatif yang ditolak

- **`Mutex<State>`**: sederhana, tapi render bisa tertahan oleh sampler. Dengan
  400 proses dan PDH, ini terasa sebagai micro-stutter. Juga memaksa setiap view
  memegang lock selama menggambar.
- **Channel per bagian**: UI harus menyatukan 8 sumber dengan timing berbeda, dan
  harus menangani bagian yang belum datang. Menambah kompleksitas tanpa manfaat.
- **`RwLock`**: memperbaiki pembacaan bersamaan tapi tidak masalah utama (latensi
  tulis), dan menambah risiko writer starvation.

## Konsekuensi

Positif:

- UI selalu responsif; angka boleh tertinggal 1 detik, frame tidak boleh tertinggal.
- Model data sederhana: `Snapshot` masuk, `Snapshot` keluar.
- Alokasi bisa dikendalikan: sampler membangun `Snapshot` baru dari buffer yang
  dipakai ulang, lalu membuang yang lama (hanya satu yang hidup).

Negatif:

- Setiap snapshot baru mengalokasikan `Vec<ProcessRow>` (dengan string). Untuk 400
  proses × 1 Hz ini masih kecil, tapi harus diukur (lihat
  [PERF.md](../PERF.md)). Kalau jadi masalah, langkah berikutnya adalah arena +
  string interning per nama executable.
- Ring buffer grafik harus dikelola UI, karena `core` tidak menyimpan histori.
  Ini disengaja: histori tampilan bukan tanggung jawab collector.

## Catatan implementasi

```rust
// sampler
let snapshot = build_snapshot(&mut buffers, seq, interval)?;
shared.store(Arc::new(snapshot));

// render
let snap = shared.load();        // Guard<Arc<Snapshot>>
if snap.seq != last_seen { /* update ring buffer */ }
```

`ArcSwap` dipilih daripada `ArcSwapOption` karena snapshot pertama selalu ada
sebelum UI menggambar (UI menunggu maksimal 2 detik, menampilkan spinner).
