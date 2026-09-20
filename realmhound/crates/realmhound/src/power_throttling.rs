//! Opt the process out of Windows background throttling so the capture/worker/
//! audio threads keep running at full speed while RealmHound is minimised,
//! otherwise EcoQoS delays loot notifications until restore.

use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, ProcessPowerThrottling, SetProcessInformation,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE,
};

/// Best-effort: opt out of execution-speed and timer-resolution throttling.
/// `StateMask = 0` forces the masked policies OFF.
pub fn disable_background_throttling() {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED
            | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION,
        StateMask: 0,
    };

    let ok = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const core::ffi::c_void,
            core::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };

    if ok != 0 {
        tracing::info!("[POWER] Disabled background execution-speed throttling");
    } else {
        let err = unsafe { GetLastError() };
        tracing::warn!("[POWER] Failed to disable background throttling (error {err}, non-fatal)");
    }
}
