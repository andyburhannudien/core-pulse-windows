//! Collector dan model data untuk core-pulse.
//!
//! Crate ini tidak tahu apa-apa tentang UI. Semua yang ada di sini bisa dipakai
//! oleh `core-pulse-cli` maupun `core-pulse.exe`.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod collector;
pub mod model;
pub mod sampler;
pub mod sys;

pub use collector::Collector;
pub use model::*;

/// Versi skema model data. Lihat docs/DATA-MODEL.md.
pub const SCHEMA_VERSION: u16 = 1;
