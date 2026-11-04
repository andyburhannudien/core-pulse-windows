//! Uji apakah SMART bisa dibaca **tanpa** `GENERIC_READ`.
//!
//! Ini sengaja dijadikan contoh (example) terpisah, bukan test: jawabannya
//! bergantung pada driver dan kebijakan mesin, dan kita ingin bisa
//! membandingkannya dengan mudah di mesin lain.

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Ioctl::{
    IOCTL_STORAGE_QUERY_PROPERTY, ProtocolTypeNvme, STORAGE_PROPERTY_QUERY,
    STORAGE_PROTOCOL_SPECIFIC_DATA, StorageDeviceProtocolSpecificProperty,
};
use windows::core::PCWSTR;

#[repr(C)]
#[derive(Clone, Copy)]
struct QueryWithProtocol {
    query: STORAGE_PROPERTY_QUERY,
    protocol: STORAGE_PROTOCOL_SPECIFIC_DATA,
}

fn try_access(index: u32, access: u32, label: &str) {
    let path = format!(r"\\.\PhysicalDrive{index}");
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: path NUL-terminated.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    };

    let handle = match handle {
        Ok(h) => h,
        Err(e) => {
            println!(
                "  {label:<16} open gagal: 0x{:04X}",
                (e.code().0 as u32) & 0xFFFF
            );
            return;
        }
    };

    // NVMe health log lewat protocol specific property.
    let input = QueryWithProtocol {
        query: STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProtocolSpecificProperty,
            QueryType: windows::Win32::System::Ioctl::PropertyStandardQuery,
            AdditionalParameters: [0],
        },
        protocol: STORAGE_PROTOCOL_SPECIFIC_DATA {
            ProtocolType: ProtocolTypeNvme,
            DataType: 0,
            ProtocolDataRequestValue: 0x02,
            ProtocolDataRequestSubValue: 0,
            ProtocolDataOffset: std::mem::size_of::<STORAGE_PROTOCOL_SPECIFIC_DATA>() as u32,
            ProtocolDataLength: 512,
            FixedProtocolReturnData: 0,
            ProtocolDataRequestSubValue2: 0,
            ProtocolDataRequestSubValue3: 0,
            ProtocolDataRequestSubValue4: 0,
        },
    };

    let input_bytes = unsafe {
        std::slice::from_raw_parts(
            &input as *const _ as *const u8,
            std::mem::size_of::<QueryWithProtocol>(),
        )
    };

    let out_len = std::mem::size_of::<QueryWithProtocol>() + 512;
    let mut out = vec![0u64; out_len.div_ceil(8)];
    let mut returned = 0u32;

    // SAFETY: buffer cukup besar dan selaras.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(input_bytes.as_ptr() as *const _),
            input_bytes.len() as u32,
            Some(out.as_mut_ptr() as *mut _),
            out_len as u32,
            Some(&mut returned),
            None,
        )
    };

    match ok {
        Ok(()) => {
            let offset = std::mem::size_of::<QueryWithProtocol>();
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    (out.as_ptr() as *const u8).add(offset),
                    512.min(returned as usize),
                )
            };
            if bytes.len() >= 3 {
                let kelvin = u16::from_le_bytes([bytes[1], bytes[2]]);
                println!(
                    "  {label:<16} IOCTL OK, returned={returned}, suhu={:.1} °C",
                    kelvin as f32 - 273.15
                );
            } else {
                println!("  {label:<16} IOCTL OK tapi payload pendek ({returned})");
            }
        }
        Err(e) => {
            // SCSI/ATA pass-through mungkin jalur yang benar untuk disk ini.
            println!(
                "  {label:<16} IOCTL gagal: 0x{:04X}",
                (e.code().0 as u32) & 0xFFFF
            );
        }
    }

    // SAFETY: handle dibuka di atas.
    unsafe {
        let _ = CloseHandle(handle);
    }
}

fn main() {
    println!("Menguji akses SMART per tingkat hak akses (tanpa admin vs GENERIC_READ)");
    for index in 0..2u32 {
        println!("\nPhysicalDrive{index}:");
        try_access(index, 0, "akses 0");
        try_access(index, 0x8000_0000, "GENERIC_READ");
        // 0x0012_0080 = GENERIC_READ|GENERIC_WRITE tanpa hak khusus
        try_access(index, 0x0012_0080, "R|W");
    }
}
