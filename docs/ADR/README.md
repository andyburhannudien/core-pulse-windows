# Architecture Decision Records

Catatan keputusan arsitektur. Format ringkas: konteks, keputusan, alternatif,
konsekuensi. ADR tidak pernah dihapus — kalau keputusan berubah, tulis ADR baru
yang men-supersede dan tandai yang lama.

| # | Judul | Status |
|---|---|---|
| [0001](0001-ui-stack.md) | egui/eframe sebagai stack UI | Diterima |
| [0002](0002-sensor-tiering.md) | Tiering sensor dan penolakan driver ring0 | Diterima |
| [0003](0003-snapshot-arcswap.md) | Snapshot immutable + `ArcSwap`, satu sampler thread | Diterima |
| [0004](0004-helper-process.md) | Helper terpisah dengan protokol baca-saja | Diterima |
| [0005](0005-perf-budget-ci.md) | Budget performa sebagai gate CI | Diterima |
| [0006](0006-history-sqlite.md) | Riwayat SQLite dengan `rusqlite` bundled | Diterima |
| [0007](0007-repository-layout.md) | Repository terpisah dari `disk-pulse` | Diterima |
| [0008](0008-honest-degradation.md) | `Option` = tidak diketahui, tanpa angka palsu | Diterima |

## Kapan menulis ADR baru

Tulis ADR kalau keputusan:

- sulit dibalik (pilihan UI toolkit, format data, protokol IPC),
- menghabiskan > 1 hari kerja untuk diubah,
- menghalangi opsi lain di masa depan,
- atau pernah diperdebatkan lebih dari sekali.

Tidak perlu ADR untuk: nama variabel, pilihan `Vec` vs `SmallVec` di satu fungsi,
atau hal yang bisa diubah dalam 10 menit.
