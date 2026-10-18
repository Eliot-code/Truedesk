//! Privacy mode: during a session the host ignores its local keyboard and
//! mouse and blanks its physical screen, so someone at the host machine cannot
//! interfere or watch. The remote viewer still sees and controls the real
//! desktop, because the host's injected input and the captured frames are
//! unaffected.
//!
//! Safety: an active mode is a guard whose `Drop` restores everything, so a
//! session that ends for any reason (including a panic) never leaves the host
//! locked out or blanked.
//!
//! Only Windows is supported for now. `BlockInput` suppresses local input while
//! injected input keeps working, and a top-most black window marked
//! `WDA_EXCLUDEFROMCAPTURE` blanks the display locally without changing what the
//! capturer (and thus the viewer) sees. macOS and Linux have no equally clean
//! mechanism yet, so the feature is advertised off there.

/// Whether this platform can enter privacy mode.
pub fn available() -> bool {
    cfg!(windows)
}

/// An active privacy mode. Dropping it restores local input and the screen.
pub struct PrivacyMode {
    #[cfg(windows)]
    _inner: windows_impl::Active,
}

/// Turn privacy mode on. Returns a guard that restores on drop. On platforms
/// that do not support it this is an error and nothing changes.
pub fn enable() -> Result<PrivacyMode, String> {
    #[cfg(windows)]
    {
        Ok(PrivacyMode { _inner: windows_impl::Active::enter()? })
    }
    #[cfg(not(windows))]
    {
        Err("privacy mode is not supported on this platform".into())
    }
}

#[cfg(windows)]
mod windows_impl {
    use std::sync::mpsc;
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM, COLORREF};
    use windows::Win32::Graphics::Gdi::{CreateSolidBrush, GetStockObject, BLACK_BRUSH, HBRUSH};
    use windows::Win32::UI::Input::KeyboardAndMouse::BlockInput;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, GetSystemMetrics,
        PostMessageW, RegisterClassW, SetWindowDisplayAffinity, ShowWindow, TranslateMessage,
        HWND_TOPMOST, MSG, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        SW_SHOW, WDA_EXCLUDEFROMCAPTURE, WM_CLOSE, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
        WS_EX_TOPMOST, WS_POPUP,
    };
    use windows::core::{w, PCWSTR};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    /// An active privacy mode on Windows: local input blocked and a black,
    /// capture-excluded cover shown. Dropping it undoes both.
    pub(super) struct Active {
        /// The thread running the cover window's message loop, and the window to
        /// close when tearing down.
        cover: Option<Cover>,
    }

    struct Cover {
        hwnd: isize,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Active {
        pub(super) fn enter() -> Result<Self, String> {
            // Block local input first: this is the essential, reliable part.
            // SAFETY: a plain Win32 call; injected input (SendInput) is
            // unaffected, and input unblocks if this process exits.
            unsafe {
                BlockInput(true).map_err(|error| format!("could not block local input: {error}"))?;
            }
            // Best-effort cover: if it fails, input stays blocked and we log.
            let cover = match Cover::show() {
                Ok(cover) => Some(cover),
                Err(error) => {
                    log::warn!("privacy mode: screen cover failed ({error}); local input is still blocked");
                    None
                }
            };
            Ok(Self { cover })
        }
    }

    impl Drop for Active {
        fn drop(&mut self) {
            if let Some(cover) = self.cover.take() {
                cover.close();
            }
            // SAFETY: restores local input; always runs, even on a panic.
            unsafe {
                let _ = BlockInput(false);
            }
        }
    }

    impl Cover {
        fn show() -> Result<Self, String> {
            let (ready_tx, ready_rx) = mpsc::channel::<Result<isize, String>>();
            let thread = std::thread::Builder::new()
                .name("opendesk-privacy-cover".into())
                .spawn(move || cover_thread(&ready_tx))
                .map_err(|error| error.to_string())?;
            match ready_rx.recv() {
                Ok(Ok(hwnd)) => Ok(Self { hwnd, thread: Some(thread) }),
                Ok(Err(error)) => { let _ = thread.join(); Err(error) }
                Err(_) => { let _ = thread.join(); Err("privacy cover thread stopped".into()) }
            }
        }

        fn close(mut self) {
            // SAFETY: ask the cover's message loop to close its window; the
            // thread then returns and the handle joins.
            unsafe {
                let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn cover_thread(ready: &mpsc::Sender<Result<isize, String>>) {
        // SAFETY: standard window creation. The window owns its own message
        // loop on this thread; it is destroyed on WM_CLOSE from close().
        unsafe {
            let instance = match GetModuleHandleW(PCWSTR::null()) {
                Ok(instance) => instance,
                Err(error) => { let _ = ready.send(Err(error.to_string())); return; }
            };
            let class_name = w!("OpenDeskPrivacyCover");
            let brush: HBRUSH = {
                let solid = CreateSolidBrush(COLORREF(0));
                if solid.is_invalid() { HBRUSH(GetStockObject(BLACK_BRUSH).0) } else { solid }
            };
            let class = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class_name,
                hbrBackground: brush,
                ..Default::default()
            };
            // Ignore a duplicate-class error from a previous mode on this run.
            let _atom = RegisterClassW(&class);
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let hwnd = match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
                class_name,
                w!("OpenDesk privacy"),
                WS_POPUP,
                x, y, width.max(1), height.max(1),
                None, None, Some(instance.into()), None,
            ) {
                Ok(hwnd) => hwnd,
                Err(error) => { let _ = ready.send(Err(error.to_string())); return; }
            };
            // Exclude the cover from capture so the viewer still sees the real
            // desktop while the local screen is black. If this fails the cover
            // would hide the viewer's view too, so bail and restore.
            if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_err() {
                let _ = DestroyWindow(hwnd);
                let _ = ready.send(Err("could not exclude the cover from capture".into()));
                return;
            }
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = set_topmost(hwnd);
            let _ = ready.send(Ok(hwnd.0 as isize));

            let mut message = MSG::default();
            while GetMessageW(&mut message, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            let _ = DestroyWindow(hwnd);
        }
    }

    // Keep the cover above other top-most windows without pulling in extra
    // imports at the call site.
    unsafe fn set_topmost(hwnd: HWND) -> windows::core::Result<()> {
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE};
        SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)
    }
}
