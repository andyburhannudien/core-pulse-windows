//! Protokol named pipe antara UI dan helper.
//!
//! Protokol sengaja baca-saja. Lihat docs/ADR/0004-helper-process.md dan
//! docs/THREAT-MODEL.md (T2).

#![deny(unsafe_op_in_unsafe_fn)]

use serde::{Deserialize, Serialize};

/// Versi protokol. Naikkan kalau ada perubahan yang memutus kompatibilitas.
pub const PROTO_VERSION: u16 = 1;

/// Batas ukuran satu pesan. Pesan lebih besar ditolak sebelum dialokasikan.
pub const MAX_MESSAGE_BYTES: u32 = 1024 * 1024;

pub const PIPE_NAME: &str = r"\\.\pipe\core-pulse-v1";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Request {
    Hello { proto_version: u16 },
    GetSensors,
    GetSmart { disk_index: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Response {
    Hello {
        proto_version: u16,
    },
    Sensors {
        readings: Vec<core_pulse_core::model::SensorReading>,
    },
    Smart {
        disks: Vec<core_pulse_core::model::DiskData>,
    },
    Error {
        code: u16,
        message: String,
    },
}

/// Header biner di awal setiap pesan: `u32` panjang payload + `u16` versi.
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
}
