//! Protokol named pipe antara UI dan helper.
//!
//! Protokol sengaja **baca-saja**. Lihat docs/ADR/0004-helper-process.md dan
//! docs/THREAT-MODEL.md (T2). Tidak ada pesan yang menyebabkan helper
//! mengeksekusi perintah, menulis file, atau membuka path yang ditentukan klien.
//!
//! Framing: `u32 length` (LE) + `u16 proto_version` (LE) + payload JSON.
//! Panjang > 1 MiB ditolak **sebelum** dialokasikan.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod pipe;

use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Versi protokol. Naikkan kalau ada perubahan yang memutus kompatibilitas.
pub const PROTO_VERSION: u16 = 1;

/// Batas ukuran satu pesan. Pesan lebih besar ditolak sebelum dialokasikan.
pub const MAX_MESSAGE_BYTES: u32 = 1024 * 1024;

pub const PIPE_NAME: &str = r"\\.\pipe\core-pulse-v1";

/// Nama Windows Service untuk helper. Dipakai saat instalasi service dan saat
/// klien memverifikasi pipe milik service LocalSystem (lihat `pipe::service_process`).
pub const HELPER_SERVICE_NAME: &str = "core-pulse-helper";

/// Nama pipe tanpa prefix, untuk `CreateNamedPipeW`.
pub const PIPE_PATH: &str = r"\\.\pipe\core-pulse-v1";

/// Timeout baca per pesan. Klien yang menggantung tidak boleh memblokir helper.
pub const READ_TIMEOUT_MS: u32 = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Request {
    /// Negosiasi versi. Wajib jadi pesan pertama.
    Hello {
        proto_version: u16,
    },
    GetSensors,
    GetSmart {
        disk_index: u32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Response {
    Hello {
        proto_version: u16,
        /// Versi helper, untuk ditampilkan saat versi tidak cocok.
        helper_version: String,
        /// Apakah helper berjalan elevated (harus `true` untuk build normal).
        elevated: bool,
    },
    Sensors {
        readings: Vec<core_pulse_core::model::SensorReading>,
        degraded: Vec<core_pulse_core::model::Degradation>,
        statuses: Vec<core_pulse_core::model::SensorStatus>,
    },
    Smart {
        disks: Vec<core_pulse_core::model::SensorReading>,
    },
    Error {
        code: u16,
        message: String,
    },
}

/// Kode error protokol.
pub mod codes {
    pub const OK: u16 = 0;
    pub const BAD_VERSION: u16 = 1;
    pub const MESSAGE_TOO_LARGE: u16 = 2;
    pub const MALFORMED: u16 = 3;
    pub const ACCESS_DENIED: u16 = 4;
    pub const UNSUPPORTED: u16 = 5;
    pub const INTERNAL: u16 = 6;
}

/// Header biner di awal setiap pesan.
#[derive(Debug, Clone, Copy)]
pub struct FrameHeader {
    pub length: u32,
    pub proto_version: u16,
}

impl FrameHeader {
    pub const SIZE: usize = 6;

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.length == 0 {
            return Err("panjang pesan nol");
        }
        if self.length > MAX_MESSAGE_BYTES {
            return Err("pesan melebihi batas 1 MiB");
        }
        if self.proto_version != PROTO_VERSION {
            return Err("versi protokol tidak cocok");
        }
        Ok(())
    }

    pub fn encode(&self) -> [u8; Self::SIZE] {
        let mut out = [0u8; Self::SIZE];
        out[0..4].copy_from_slice(&self.length.to_le_bytes());
        out[4..6].copy_from_slice(&self.proto_version.to_le_bytes());
        out
    }

    pub fn decode(buf: &[u8; Self::SIZE]) -> Self {
        Self {
            length: u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]),
            proto_version: u16::from_le_bytes([buf[4], buf[5]]),
        }
    }
}

/// Error saat membaca/menulis frame.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame tidak valid: {0}")]
    Invalid(&'static str),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Tulis satu pesan. `length` dihitung dari payload JSON.
pub fn write_message<W: Write, T: Serialize>(w: &mut W, msg: &T) -> Result<(), FrameError> {
    let payload = serde_json::to_vec(msg)?;
    if payload.len() > MAX_MESSAGE_BYTES as usize {
        return Err(FrameError::Invalid("payload melebihi batas 1 MiB"));
    }
    let header = FrameHeader {
        length: payload.len() as u32,
        proto_version: PROTO_VERSION,
    };
    w.write_all(&header.encode())?;
    w.write_all(&payload)?;
    w.flush()?;
    Ok(())
}

/// Baca satu pesan. Header divalidasi **sebelum** alokasi payload, jadi klien
/// jahat tidak bisa memaksa helper mengalokasikan 500 MB.
pub fn read_message<R: Read, T: for<'de> Deserialize<'de>>(r: &mut R) -> Result<T, FrameError> {
    let mut header_buf = [0u8; FrameHeader::SIZE];
    r.read_exact(&mut header_buf)?;
    let header = FrameHeader::decode(&header_buf);
    header.validate().map_err(FrameError::Invalid)?;

    let mut payload = vec![0u8; header.length as usize];
    r.read_exact(&mut payload)?;
    Ok(serde_json::from_slice(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_oversized_message() {
        let h = FrameHeader {
            length: MAX_MESSAGE_BYTES + 1,
            proto_version: PROTO_VERSION,
        };
        assert!(h.validate().is_err());
    }

    #[test]
    fn rejects_zero_length() {
        let h = FrameHeader {
            length: 0,
            proto_version: PROTO_VERSION,
        };
        assert!(h.validate().is_err());
    }

    #[test]
    fn rejects_wrong_version() {
        let h = FrameHeader {
            length: 10,
            proto_version: PROTO_VERSION + 1,
        };
        assert!(h.validate().is_err());
    }

    #[test]
    fn accepts_normal_frame() {
        let h = FrameHeader {
            length: 512,
            proto_version: PROTO_VERSION,
        };
        assert!(h.validate().is_ok());
    }

    #[test]
    fn header_roundtrip() {
        let h = FrameHeader {
            length: 1234,
            proto_version: PROTO_VERSION,
        };
        let decoded = FrameHeader::decode(&h.encode());
        assert_eq!(decoded.length, 1234);
        assert_eq!(decoded.proto_version, PROTO_VERSION);
    }

    #[test]
    fn request_roundtrip_through_pipe() {
        let mut buf = Vec::new();
        write_message(&mut buf, &Request::GetSensors).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let req: Request = read_message(&mut cursor).unwrap();
        assert!(matches!(req, Request::GetSensors));
    }

    #[test]
    fn response_roundtrip_through_pipe() {
        let mut buf = Vec::new();
        write_message(
            &mut buf,
            &Response::Hello {
                proto_version: PROTO_VERSION,
                helper_version: "0.1.0".into(),
                elevated: true,
            },
        )
        .unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let resp: Response = read_message(&mut cursor).unwrap();
        match resp {
            Response::Hello { elevated, .. } => assert!(elevated),
            other => panic!("respons salah: {other:?}"),
        }
    }

    #[test]
    fn truncated_frame_is_rejected() {
        // Header menyatakan 100 byte, payload hanya 10.
        let mut buf = Vec::new();
        let h = FrameHeader {
            length: 100,
            proto_version: PROTO_VERSION,
        };
        buf.extend_from_slice(&h.encode());
        buf.extend_from_slice(&[0u8; 10]);
        let mut cursor = std::io::Cursor::new(buf);
        let result: Result<Request, _> = read_message(&mut cursor);
        assert!(
            result.is_err(),
            "frame terpotong harus gagal, bukan menggantung"
        );
    }

    #[test]
    fn invalid_json_is_rejected_without_panic() {
        let payload = b"not json";
        let h = FrameHeader {
            length: payload.len() as u32,
            proto_version: PROTO_VERSION,
        };
        let mut buf = Vec::new();
        buf.extend_from_slice(&h.encode());
        buf.extend_from_slice(payload);
        let mut cursor = std::io::Cursor::new(buf);
        let result: Result<Request, _> = read_message(&mut cursor);
        assert!(matches!(result, Err(FrameError::Json(_))));
    }

    #[test]
    fn oversized_header_is_rejected_before_allocating() {
        // Header menyatakan 4 GiB. `read_message` harus gagal di validasi,
        // bukan mencoba mengalokasikan.
        let mut buf = Vec::new();
        buf.extend_from_slice(&u32::MAX.to_le_bytes());
        buf.extend_from_slice(&PROTO_VERSION.to_le_bytes());
        let mut cursor = std::io::Cursor::new(buf);
        let result: Result<Request, _> = read_message(&mut cursor);
        assert!(matches!(result, Err(FrameError::Invalid(_))));
    }
}
