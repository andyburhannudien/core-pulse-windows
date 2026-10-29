# ADR-0002 — Tiering sensor dan penolakan driver ring0

**Status:** Diterima
**Tanggal:** 2026-10-29

## Konteks

Fitur pembeda aplikasi ini adalah monitoring suhu. Kenyataannya:

1. Windows tidak menyediakan API user-mode untuk suhu CPU.
2. Jalur ACPI sering kosong. Di mesin pengembangan, `MSAcpi_ThermalZoneTemperature`
   punya **0 instance** dan PDH `Thermal Zone Information` juga **0 instance**.
3. Membaca MSR (`IA32_PACKAGE_THERM_STATUS` 0x1B1, `IA32_THERM_STATUS` 0x19C)
   membutuhkan kode ring0.
4. `WinRing0.sys` — jalur yang dipakai OpenHardwareMonitor dan versi lama
   LibreHardwareMonitor — punya CVE-2020-14979, ada di vulnerable driver blocklist
   Microsoft, dan ditolak HVCI di Windows 11 24H2+.

## Keputusan

Pakai **tiering** dengan degradasi eksplisit, dan **tolak** driver kernel di v1:

| Tier | Sumber | Hak | Peran |
|---|---|---|---|
| 0 | ACPI / PDH thermal zone | user | dicoba pertama, sering kosong |
| 1 | SMART disk (NVMe/ATA) | admin | suhu disk, port dari `disk-pulse` |
| 2 | GPU vendor (NVML/IGCL/ADLX) | user | suhu GPU |
| 3 | Bridge LHM/HWiNFO | user | opt-in, ditandai pihak ketiga |
| 4 | Driver ring0 sendiri | admin | **tidak dipakai di v1** |

Ketika tidak ada sumber, UI menampilkan `—` + alasan, bukan angka.

## Alasan

1. **Distribusi.** Mengemas WinRing0 berarti aplikasi di-flag antivirus dan gagal
   di mesin dengan keamanan default. Itu bukan risiko teoretis, itu kegagalan
   pemasangan.
2. **Biaya driver.** Driver kernel yang benar butuh: sertifikat EV, attestation
   signing Microsoft, pengujian di banyak build Windows, dan pemeliharaan setiap
   kali kebijakan berubah. Itu proyek 3–6 bulan tersendiri.
3. **Anti-cheat.** Banyak game memblokir driver monitoring. Task manager yang
   tidak bisa dijalankan sambil bermain game adalah cacat serius.
4. **Kejujuran lebih baik daripada angka palsu.** Pengguna yang tahu "suhu CPU
   tidak tersedia di mesin ini" bisa mengambil tindakan (pakai LHM, ganti tool).
   Pengguna yang melihat `0 °C` akan menyimpulkan aplikasinya rusak.
5. **Bridge Tier 3 memberi jalan keluar** tanpa kita menanggung risiko driver:
   pengguna yang sudah menjalankan LHM mendapat suhu CPU, dan kita hanya membaca
   HTTP loopback.

## Alternatif yang ditolak

- **Kemas WinRing0 / RTCore64 / driver serupa**: diblokir, berisiko hukum dan
  reputasi, dan merusak distribusi.
- **Intel Power Gadget**: dihentikan, driver-nya bermasalah di Windows baru.
- **Vendor WMI (HP/Dell/Lenovo)**: hanya ada di sebagian model, tidak portabel,
  dan di mesin ini namespace `root\HP` tidak mengekspos kelas thermal.
- **Driver sendiri sejak v1**: terlalu mahal untuk fitur pendukung.
- **Menampilkan suhu estimasi dari beban CPU**: menyesatkan. Ditolak keras.

## Konsekuensi

Positif:

- Aplikasi bisa dipasang di mana saja tanpa flag antivirus.
- Tidak ada driver yang perlu diperbarui saat Windows berubah.
- Arsitektur `SensorProvider` membuat penambahan sumber baru murah.

Negatif:

- Di mesin tanpa ACPI thermal zone dan tanpa GPU diskrit, satu-satunya suhu
  adalah suhu disk — dan itu butuh helper.
- Fitur "monitoring suhu" harus dipasarkan dengan ekspektasi yang tepat.

## Pemicu untuk meninjau ulang

- Kalau Microsoft menyediakan API suhu resmi (mis. lewat `Windows.Devices.Sensors`
  atau ETW thermal yang terdokumentasi).
- Kalau aplikasi menjadi produk komersial dengan sumber daya untuk driver
  ber-sertifikat.
- Kalau ada cukup pengguna yang membutuhkan suhu CPU dan menolak bridge pihak
  ketiga — itu sinyal untuk ADR baru, bukan penambahan diam-diam.
