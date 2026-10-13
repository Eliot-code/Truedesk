//! Small Windows-only hooks used by the SYSTEM-launched host.

use super::secure_attention::SecureAttentionError;

/// The service marks its child with this variable. Environment variables are
/// inherited by the child and avoid treating an ordinary signed-in host as a
/// secure-desktop host.
pub fn helper_active() -> bool {
    std::env::var_os("OPENDESK_SERVICE_HOST").is_some_and(|value| value == "1")
}

/// Ask Windows' secure-attention broker to show the security screen.
/// `SendSAS` is intentionally loaded dynamically: it is an OS service export,
/// not a normal application API and is absent on older Windows versions.
pub fn send_secure_attention() -> Result<(), SecureAttentionError> {
    use std::ffi::c_void;
    use windows::core::PCSTR;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

    unsafe {
        let library = LoadLibraryA(PCSTR(b"sas.dll\0".as_ptr()));
        let Ok(library) = library else { return Err(SecureAttentionError::Unsupported) };
        let Some(address) = GetProcAddress(library, PCSTR(b"SendSAS\0".as_ptr())) else {
            return Err(SecureAttentionError::Unsupported);
        };
        let send_sas: extern "system" fn(bool) = std::mem::transmute::<*const c_void, _>(address);
        send_sas(true);
    }
    Ok(())
}
