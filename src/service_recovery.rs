// Restarts the Windows media session service (NPSMSvc) when it hangs.
// It runs as a per-user service under the user's own account, so its process can be ended without admin rights,
// and Windows then restarts it automatically from the service's recovery settings.
use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{EqualSid, GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Services::{
    CloseServiceHandle, ENUM_SERVICE_STATUS_PROCESSW, ENUM_SERVICE_TYPE, EnumServicesStatusExW,
    OpenSCManagerW, SC_ENUM_PROCESS_INFO, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_ACTIVE,
    SERVICE_USER_SHARE_PROCESS, SERVICE_WIN32,
};
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_TERMINATE, TerminateProcess,
};

// Per-user service instances are named after the template service plus a per-logon suffix, e.g. "NPSMSvc_a4d98d".
const SERVICE_NAME_PREFIX: &str = "NPSMSvc_";
// Per-user service instances carry this type flag on top of their regular service type.
const USER_SERVICE_INSTANCE: u32 = 0x80;

// Ends the current user's media session service process so Windows restarts it.
pub fn restart_media_service() -> Result<(), String> {
    let pid = find_media_service_process().ok_or("Media session service process not found")?;

    unsafe {
        let process = OpenProcess(PROCESS_TERMINATE, false, pid).map_err(|error| error.to_string())?;
        let result = TerminateProcess(process, 1);
        let _ = CloseHandle(process);
        result.map_err(|error| error.to_string())
    }
}

// Finds the process hosting this user's instance of the service, skipping other signed-in users' instances.
fn find_media_service_process() -> Option<u32> {
    let current_user = process_token_user(unsafe { GetCurrentProcess() })?;

    running_service_process_ids(SERVICE_NAME_PREFIX)
        .into_iter()
        .find(|&pid| {
            let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
                return false;
            };
            let owner = process_token_user(process);
            unsafe {
                let _ = CloseHandle(process);
            }
            owner.is_some_and(|owner| same_user(&owner, &current_user))
        })
}

// Returns the process IDs of running services whose names start with the given prefix.
fn running_service_process_ids(prefix: &str) -> Vec<u32> {
    let mut pids = Vec::new();
    let service_type = ENUM_SERVICE_TYPE(SERVICE_WIN32.0 | SERVICE_USER_SHARE_PROCESS.0 | USER_SERVICE_INSTANCE);

    unsafe {
        let Ok(manager) = OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_ENUMERATE_SERVICE) else {
            return pids;
        };

        let mut needed = 0u32;
        let mut returned = 0u32;
        let _ = EnumServicesStatusExW(
            manager, SC_ENUM_PROCESS_INFO, service_type, SERVICE_ACTIVE, None,
            &mut needed, &mut returned, None, PCWSTR::null(),
        );

        // Leave room for services that start between the two calls; u64 keeps the entries' pointers aligned.
        let mut buffer = vec![0u64; (needed as usize + 4096).div_ceil(8)];
        let bytes = std::slice::from_raw_parts_mut(buffer.as_mut_ptr().cast::<u8>(), buffer.len() * 8);
        if EnumServicesStatusExW(
            manager, SC_ENUM_PROCESS_INFO, service_type, SERVICE_ACTIVE, Some(bytes),
            &mut needed, &mut returned, None, PCWSTR::null(),
        )
        .is_ok()
        {
            let services = std::slice::from_raw_parts(
                buffer.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(),
                returned as usize,
            );
            for service in services {
                let name = service.lpServiceName.to_string().unwrap_or_default();
                let pid = service.ServiceStatusProcess.dwProcessId;
                if name.starts_with(prefix) && pid != 0 {
                    pids.push(pid);
                }
            }
        }

        let _ = CloseServiceHandle(manager);
    }

    pids
}

// Reads the TOKEN_USER of a process, stored in a u64 buffer so the structure inside stays aligned.
fn process_token_user(process: HANDLE) -> Option<Vec<u64>> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;

        let mut length = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
        let mut buffer = vec![0u64; (length as usize).div_ceil(8)];
        let result = GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), length, &mut length);
        let _ = CloseHandle(token);

        result.ok()?;
        Some(buffer)
    }
}

fn same_user(first: &[u64], second: &[u64]) -> bool {
    unsafe {
        let first = &*first.as_ptr().cast::<TOKEN_USER>();
        let second = &*second.as_ptr().cast::<TOKEN_USER>();
        EqualSid(first.User.Sid, second.User.Sid).is_ok()
    }
}
