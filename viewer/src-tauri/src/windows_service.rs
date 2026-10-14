//! Opt-in Windows service controls and SYSTEM service entrypoint.

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ServiceStatus {
    pub supported: bool,
    pub installed: bool,
    pub running: bool,
}

#[cfg(windows)]
mod platform {
    use super::ServiceStatus;
    use std::{ffi::OsString, path::Path, sync::mpsc, time::Duration};
    use windows_service::{service::{ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus as WinServiceStatus, ServiceType}, service_control_handler::{self, ServiceControlHandlerResult}};

    pub const SERVICE_NAME: &str = "OpenDeskSignIn";
    pub const SERVICE_ARG: &str = "--opendesk-service";
    pub const SERVICE_HOST_ARG: &str = "--opendesk-service-host";
    const DATA_DIR_ARG: &str = "--opendesk-data-dir";

    pub fn args() -> Vec<OsString> { std::env::args_os().skip(1).collect() }
    pub fn is_service_host_process() -> bool { args().iter().any(|arg| arg == SERVICE_HOST_ARG) }
    pub fn data_dir() -> Option<std::path::PathBuf> {
        args().windows(2).find_map(|pair| (pair[0] == DATA_DIR_ARG).then(|| std::path::PathBuf::from(&pair[1])))
    }

    pub fn run_service_process() -> bool {
        if !args().iter().any(|arg| arg == SERVICE_ARG) { return false; }
        if let Err(error) = windows_service::service_dispatcher::start(SERVICE_NAME, ffi_service_main) { log::error!("could not start OpenDesk service dispatcher: {error}"); }
        true
    }

    pub fn status() -> ServiceStatus {
        use std::process::Command;
        let output = Command::new("sc.exe").args(["query", SERVICE_NAME]).output();
        let Ok(output) = output else { return ServiceStatus { supported: true, installed: false, running: false } };
        let text = String::from_utf8_lossy(&output.stdout);
        ServiceStatus { supported: true, installed: output.status.success(), running: text.contains("RUNNING") }
    }

    pub fn should_run_user_host(service_running: bool) -> bool { !service_running || is_service_host_process() }

    fn elevated_sc(args: &[String]) -> Result<(), String> {
        use std::process::Command;
        let joined = args.iter().map(|arg| format!("'{}'", arg.replace('\'', "''"))).collect::<Vec<_>>().join(",");
        let script = format!("Start-Process -FilePath 'sc.exe' -Verb RunAs -Wait -ArgumentList @({joined})");
        let status = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).status().map_err(|e| e.to_string())?;
        status.success().then_some(()).ok_or_else(|| "Windows did not approve the OpenDesk service change".into())
    }

    pub fn install(data_dir: &Path) -> Result<ServiceStatus, String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let bin_path = format!(r#""{}" {} --opendesk-data-dir "{}""#, exe.display(), SERVICE_ARG, data_dir.display());
        elevated_sc(&["create".into(), SERVICE_NAME.into(), "binPath=".into(), bin_path, "start=".into(), "auto".into(), "obj=".into(), "LocalSystem".into()])?;
        Ok(status())
    }

    pub fn start() -> Result<ServiceStatus, String> {
        elevated_sc(&["start".into(), SERVICE_NAME.into()])?;
        Ok(status())
    }

    pub fn uninstall() -> Result<ServiceStatus, String> {
        elevated_sc(&["stop".into(), SERVICE_NAME.into()]).ok();
        elevated_sc(&["delete".into(), SERVICE_NAME.into()])?;
        Ok(status())
    }

    windows_service::define_windows_service!(ffi_service_main, service_main);

    fn service_main(_arguments: Vec<OsString>) { if let Err(error) = service_loop() { log::error!("OpenDesk service stopped: {error}"); } }

    fn service_loop() -> Result<(), windows_service::Error> {
        let (stop_tx, stop_rx) = mpsc::channel();
        let handler = move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => { let _ = stop_tx.send(()); ServiceControlHandlerResult::NoError }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status_handle = service_control_handler::register(SERVICE_NAME, handler)?;
        status_handle.set_service_status(WinServiceStatus { service_type: ServiceType::OWN_PROCESS, current_state: ServiceState::Running, controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN, exit_code: ServiceExitCode::Win32(0), checkpoint: 0, wait_hint: Duration::ZERO, process_id: None })?;
        let data_dir = data_dir();
        if let Some(dir) = &data_dir { let _ = crate::unattended::promote_staged_machine_store(dir); }
        let mut child = None;
        loop {
            if stop_rx.recv_timeout(Duration::from_secs(2)).is_ok() { break; }
            if child.as_ref().is_some_and(|handle| unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(*handle, 0) } == 0) { child = None; }
            if child.is_none() { if let Some(dir) = &data_dir { child = launch_host(dir).ok(); } }
        }
        if let Some(handle) = child { unsafe { windows_sys::Win32::System::Threading::TerminateProcess(handle, 0); windows_sys::Win32::Foundation::CloseHandle(handle); } }
        status_handle.set_service_status(WinServiceStatus { service_type: ServiceType::OWN_PROCESS, current_state: ServiceState::Stopped, controls_accepted: ServiceControlAccept::empty(), exit_code: ServiceExitCode::Win32(0), checkpoint: 0, wait_hint: Duration::ZERO, process_id: None })?;
        Ok(())
    }

    fn launch_host(data_dir: &Path) -> Result<windows_sys::Win32::Foundation::HANDLE, String> {
        use windows_sys::{Win32::{Foundation::{CloseHandle, HANDLE}, Security::{DuplicateTokenEx, SetTokenInformation, SecurityImpersonation, TokenPrimary, TokenSessionId, TOKEN_ALL_ACCESS}, System::{RemoteDesktop::WTSGetActiveConsoleSessionId, Threading::{CreateProcessAsUserW, GetCurrentProcess, OpenProcessToken, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW}}}};
        let session = unsafe { WTSGetActiveConsoleSessionId() };
        if session == u32::MAX { return Err("no active console session".into()); }
        let mut source: HANDLE = std::ptr::null_mut();
        let mut token: HANDLE = std::ptr::null_mut();
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut source) } != 0;
        let duplicated = opened && unsafe { DuplicateTokenEx(source, TOKEN_ALL_ACCESS, std::ptr::null(), SecurityImpersonation, TokenPrimary, &mut token) } != 0;
        unsafe { if !source.is_null() { CloseHandle(source); } }
        if !duplicated { return Err("could not duplicate the SYSTEM token".into()); }
        if unsafe { SetTokenInformation(token, TokenSessionId, (&session as *const u32).cast(), std::mem::size_of::<u32>() as u32) } == 0 { unsafe { CloseHandle(token); } return Err("could not assign the console session".into()); }
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut command: Vec<u16> = format!("\"{}\" --opendesk-service-host --opendesk-data-dir \"{}\"", exe.display(), data_dir.display()).encode_utf16().chain(std::iter::once(0)).collect();
        let mut desktop: Vec<u16> = "winsta0\\default".encode_utf16().chain(std::iter::once(0)).collect();
        let startup = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, lpDesktop: desktop.as_mut_ptr(), ..Default::default() };
        let mut process = PROCESS_INFORMATION::default();
        let created = unsafe { CreateProcessAsUserW(token, std::ptr::null(), command.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, CREATE_UNICODE_ENVIRONMENT, std::ptr::null(), std::ptr::null(), &startup, &mut process) } != 0;
        unsafe { CloseHandle(token); }
        if created { unsafe { CloseHandle(process.hThread); } Ok(process.hProcess) } else { Err("could not launch the service host".into()) }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::ServiceStatus;
    use std::{ffi::OsString, path::Path};
    pub fn status() -> ServiceStatus { ServiceStatus { supported: false, installed: false, running: false } }
    pub fn should_run_user_host(service_running: bool) -> bool { !service_running }
    pub fn install(_data_dir: &Path) -> Result<ServiceStatus, String> { Err("the sign-in service is only available on Windows".into()) }
    pub fn uninstall() -> Result<ServiceStatus, String> { Err("the sign-in service is only available on Windows".into()) }
    pub fn start() -> Result<ServiceStatus, String> { Err("the sign-in service is only available on Windows".into()) }
    #[allow(dead_code)] pub fn args() -> Vec<OsString> { Vec::new() }
}

pub(crate) use platform::{install, should_run_user_host, start, status, uninstall};
#[cfg(windows)]
pub(crate) use platform::{data_dir, is_service_host_process, run_service_process};

#[cfg(windows)]
pub fn run_windows_service() -> bool { run_service_process() }

#[cfg(test)]
mod tests {
    use super::should_run_user_host;
    #[test]
    fn user_host_stands_down_for_service() {
        assert!(should_run_user_host(false));
        assert!(!should_run_user_host(true));
    }
}
