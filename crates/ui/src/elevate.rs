//! Launching the helper with elevation **at the user's request**.
//!
//! There is no auto-elevation at startup: every UAC prompt is the result of a
//! click. A helper already installed as a service needs no UAC at all.

use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};
use windows::core::PCWSTR;

/// Run the helper elevated with the given arguments.
///
/// `args` is `--serve` (this session only) or `--install-service`
/// (auto-start, one UAC prompt). `visible` is for debugging only.
pub fn launch_helper(args: &str, visible: bool) -> Result<(), String> {
    let path = core_pulse_sensors::helper::expected_helper_path();
    if !path.is_file() {
        return Err(format!(
            "helper not found at {}; build core-pulse-helper or set CORE_PULSE_HELPER_PATH",
            path.display()
        ));
    }

    let operation = wide("runas");
    let file = wide(&path.to_string_lossy());
    let parameters = wide(args);

    // SAFETY: every string is NUL-terminated and lives for the call. `runas`
    // raises the UAC prompt; the helper does not inherit the UI's handles.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR(parameters.as_ptr()),
            PCWSTR::null(),
            if visible { SW_SHOWNORMAL } else { SW_HIDE },
        )
    };

    // ShellExecuteW returns a value > 32 on success.
    let code = result.0 as isize;
    if code <= 32 {
        Err(format!(
            "ShellExecuteW failed (code {code}); the UAC prompt may have been cancelled"
        ))
    } else {
        Ok(())
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
