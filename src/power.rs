// Tells Windows this is a background app, so it can run it as cheaply as possible.
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Threading::{
    GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
    SetProcessInformation,
};

// Opts into EcoQoS ("efficiency mode" in Task Manager), which runs the process on efficient cores at low clock speeds
// and lets its timers be coalesced with others.
pub fn enable_efficiency_mode() {
    let flags = PROCESS_POWER_THROTTLING_EXECUTION_SPEED | PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION;
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: flags,
        StateMask: flags,
    };

    let result = unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const _ as *const _,
            size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    if let Err(error) = result {
        eprintln!("Failed to enable efficiency mode: {error}");
    }
}

// Releases memory that was only needed during startup. Pages that are still in use come back as they are touched.
pub fn trim_working_set() {
    let _ = unsafe { K32EmptyWorkingSet(GetCurrentProcess()) };
}
