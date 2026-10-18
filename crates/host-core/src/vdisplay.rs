//! Virtual displays: screens that exist only in software, so a headless host
//! has something to share and a viewer can get an extra monitor.
//!
//! [`VirtualDisplays`] is the platform backend. A session owns its displays
//! through [`SessionDisplays`], which removes them when the session ends.
//! On macOS leaving the process removes them too: the OS ties them to the
//! process that made them. Windows keeps them, so the app calls
//! [`remove_owned`] on the way out.

use protocol::control::{self, MonitorInfo};
use std::fmt;
use std::sync::{Arc, OnceLock};

/// What to create. Sizes are in pixels; with `hidpi` the display is half
/// that size in points. (macOS starts such a display at 1x of its point
/// size and offers the 2x mode in System Settings, so text comes out at
/// the viewer's usual size either way.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DisplaySpec {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub hidpi: bool,
}

pub const MIN_WIDTH: u32 = 640;
pub const MIN_HEIGHT: u32 = 480;
pub const MAX_WIDTH: u32 = 3840;
pub const MAX_HEIGHT: u32 = 2160;

impl DisplaySpec {
    /// A spec for a viewer's request: kept within 640x480..3840x2160 with
    /// the aspect ratio preserved where it can be, and even on both sides
    /// (the encoder needs even sizes; HiDPI needs whole points).
    pub fn fitted(width: u32, height: u32, hidpi: bool) -> Self {
        let (mut width, mut height) = (f64::from(width.max(1)), f64::from(height.max(1)));
        let shrink = (f64::from(MAX_WIDTH) / width).min(f64::from(MAX_HEIGHT) / height).min(1.0);
        width *= shrink;
        height *= shrink;
        let even = |value: f64, min: u32, max: u32| ((value as u32).clamp(min, max)) & !1;
        let (width, height) = (even(width, MIN_WIDTH, MAX_WIDTH), even(height, MIN_HEIGHT, MAX_HEIGHT));
        // At 2x each side must be a whole, even number of points, and no
        // smaller than the minimum (macOS never brings up smaller modes).
        let hidpi = hidpi && width % 4 == 0 && height % 4 == 0 && width / 2 >= MIN_WIDTH && height / 2 >= MIN_HEIGHT;
        Self { width, height, refresh_hz: 60, hidpi }
    }

    /// What a host with no screen gets.
    pub fn headless() -> Self { Self { width: 1920, height: 1080, refresh_hz: 60, hidpi: false } }
}

/// A display a backend made: its id (see [`VirtualDisplays`]) and the size
/// it really got, which differs from the request where the platform only
/// offers fixed modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddedDisplay {
    pub id: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VirtualDisplayError {
    /// This OS (or OS version) has no virtual displays Truedesk can drive,
    /// or their driver isn't installed.
    Unsupported,
    /// The computer has as many virtual displays as it can take.
    Limit,
    /// Only the newest virtual display can go (Windows' driver removes
    /// displays last in, first out).
    NotNewest,
    /// The platform refused or failed; the text is for logs.
    Failed(String),
}

impl fmt::Display for VirtualDisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("virtual displays are not supported on this computer"),
            Self::Limit => f.write_str("this computer has as many virtual displays as it can take"),
            Self::NotNewest => f.write_str("only the newest virtual display can be removed"),
            Self::Failed(reason) => write!(f, "could not create a virtual display: {reason}"),
        }
    }
}

impl std::error::Error for VirtualDisplayError {}

/// A platform's virtual displays.
///
/// `create` returns once the display exists, with the id the screen
/// capturer and input mapping use for it (a `CGDirectDisplayID` on macOS,
/// an `HMONITOR` on Windows). A backend that drives an external driver
/// translates the driver's handle into that id and keeps the handle
/// itself. Backends only remove displays they created. macOS has one, and
/// so does Windows with Amyuni's driver installed; elsewhere [`platform`]
/// is [`Unsupported`].
pub trait VirtualDisplays: Send + Sync {
    /// Whether `create` can work here. Cheap, and must not prompt the user.
    fn supported(&self) -> bool;

    fn create(&self, spec: &DisplaySpec) -> Result<AddedDisplay, VirtualDisplayError>;

    /// Whether `remove` would take this display away now. `Ok` for ids the
    /// backend doesn't know.
    fn removable(&self, _id: u32) -> Result<(), VirtualDisplayError> { Ok(()) }

    /// Removes a display this backend created; unknown ids are ignored.
    fn remove(&self, id: u32) -> Result<(), VirtualDisplayError>;

    /// The displays this backend created and still holds.
    fn owned(&self) -> Vec<u32>;
}

/// The backend for platforms without virtual displays.
#[derive(Debug, Default)]
pub struct Unsupported;

impl VirtualDisplays for Unsupported {
    fn supported(&self) -> bool { false }

    fn create(&self, _spec: &DisplaySpec) -> Result<AddedDisplay, VirtualDisplayError> { Err(VirtualDisplayError::Unsupported) }

    fn remove(&self, _id: u32) -> Result<(), VirtualDisplayError> { Ok(()) }

    fn owned(&self) -> Vec<u32> { Vec::new() }
}

/// This computer's backend, shared by every session of the process.
pub fn platform() -> Arc<dyn VirtualDisplays> {
    static PLATFORM: OnceLock<Arc<dyn VirtualDisplays>> = OnceLock::new();
    Arc::clone(PLATFORM.get_or_init(|| {
        #[cfg(target_os = "macos")]
        return Arc::new(macos::CgVirtualDisplays::new());
        #[cfg(windows)]
        return Arc::new(win32::AmyuniDisplays::new());
        #[cfg(not(any(target_os = "macos", windows)))]
        return Arc::new(Unsupported);
    }))
}

/// Removes every display this process still holds, newest first. For app
/// exit; sessions remove their own when they end.
pub fn remove_owned() {
    let backend = platform();
    for id in backend.owned().into_iter().rev() {
        if let Err(error) = backend.remove(id) {
            log::warn!("left virtual display {id} in place: {error}");
        }
    }
}

/// Whether this computer has a screen to share: `Some(false)` when it has
/// none (a headless PC or Mac), `None` where it can't tell. Needs no
/// permissions.
#[cfg(target_os = "macos")]
pub fn displays_present() -> Option<bool> { Some(!macos::online_displays().is_empty()) }

/// Whether this computer has a screen to share (see the macOS version).
#[cfg(target_os = "windows")]
pub fn displays_present() -> Option<bool> {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CMONITORS};
    // SAFETY: a plain metrics query.
    Some(unsafe { GetSystemMetrics(SM_CMONITORS) } > 0)
}

/// Whether this computer has a screen to share (see the macOS version).
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn displays_present() -> Option<bool> { None }

/// Most virtual displays a viewer may add in one session.
pub const MAX_PER_SESSION: usize = 2;

/// Why a session could not add or remove a display, as a
/// `Control::VirtualDisplayRefused` code.
pub fn refusal_code(error: &VirtualDisplayError) -> &'static str {
    match error {
        VirtualDisplayError::Unsupported => control::VIRTUAL_DISPLAY_NOT_ALLOWED,
        VirtualDisplayError::Limit => control::VIRTUAL_DISPLAY_LIMIT,
        VirtualDisplayError::NotNewest => control::VIRTUAL_DISPLAY_NOT_NEWEST,
        VirtualDisplayError::Failed(_) => control::VIRTUAL_DISPLAY_FAILED,
    }
}

/// The virtual displays one session made. Dropping it removes them.
pub struct SessionDisplays {
    backend: Arc<dyn VirtualDisplays>,
    /// Whether the viewer may add and remove displays.
    viewer_may_add: bool,
    /// Added at the viewer's request, oldest first; the viewer may remove
    /// these.
    added: Vec<AddedDisplay>,
    /// Made because the host had no screen; kept for the whole session.
    headless: Option<AddedDisplay>,
}

impl SessionDisplays {
    pub fn new(backend: Arc<dyn VirtualDisplays>, viewer_may_add: bool) -> Self {
        Self { backend, viewer_may_add, added: Vec::new(), headless: None }
    }

    /// How many displays the viewer may add, or `None` when it may not add
    /// any (the host setting is off, or the platform can't).
    pub fn viewer_limit(&self) -> Option<u32> {
        (self.viewer_may_add && self.backend.supported()).then_some(MAX_PER_SESSION as u32)
    }

    /// Gives a host with no screen one to share, if the host allows virtual
    /// displays and the platform has them.
    pub fn add_headless(&mut self) -> Result<u32, VirtualDisplayError> {
        if !self.viewer_may_add || !self.backend.supported() {
            return Err(VirtualDisplayError::Unsupported);
        }
        let display = self.backend.create(&DisplaySpec::headless())?;
        self.headless = Some(display);
        Ok(display.id)
    }

    /// Adds a display the viewer asked for. `Err` is a refusal code.
    pub fn add(&mut self, spec: &DisplaySpec) -> Result<u32, &'static str> {
        if self.viewer_limit().is_none() {
            return Err(control::VIRTUAL_DISPLAY_NOT_ALLOWED);
        }
        if self.added.len() >= MAX_PER_SESSION {
            return Err(control::VIRTUAL_DISPLAY_LIMIT);
        }
        let display = self.backend.create(spec).map_err(|error| {
            log::warn!("{error}");
            refusal_code(&error)
        })?;
        self.added.push(display);
        Ok(display.id)
    }

    /// Whether [`remove`](Self::remove) would succeed now. `Err` is a
    /// refusal code.
    pub fn check_remove(&self, id: u32) -> Result<(), &'static str> {
        if !self.added.iter().any(|added| added.id == id) {
            return Err(control::VIRTUAL_DISPLAY_UNKNOWN);
        }
        self.backend.removable(id).map_err(|error| refusal_code(&error))
    }

    /// Removes a display the viewer added. `Err` is a refusal code.
    pub fn remove(&mut self, id: u32) -> Result<(), &'static str> {
        let Some(index) = self.added.iter().position(|added| added.id == id) else {
            return Err(control::VIRTUAL_DISPLAY_UNKNOWN);
        };
        self.backend.remove(id).map_err(|error| {
            log::warn!("{error}");
            refusal_code(&error)
        })?;
        self.added.remove(index);
        Ok(())
    }

    /// Marks the displays the viewer added, which it may remove, and fills
    /// in the size of each display the session made where capture didn't
    /// report one (so the viewer sees the size Windows really gave it).
    pub fn annotate(&self, monitors: &mut [MonitorInfo]) {
        for monitor in monitors {
            monitor.is_virtual = self.added.iter().any(|added| added.id == monitor.id);
            let made = self.added.iter().chain(&self.headless).find(|made| made.id == monitor.id);
            if let Some(made) = made.filter(|_| monitor.width == 0 || monitor.height == 0) {
                (monitor.width, monitor.height) = (made.width, made.height);
            }
        }
    }

    pub fn is_empty(&self) -> bool { self.added.is_empty() && self.headless.is_none() }
}

impl Drop for SessionDisplays {
    /// Newest first: Windows' driver can only take away its newest display.
    fn drop(&mut self) {
        for display in self.added.drain(..).rev().chain(self.headless.take()) {
            if let Err(error) = self.backend.remove(display.id) {
                log::warn!("left virtual display {} in place: {error}", display.id);
            }
        }
    }
}

pub mod amyuni;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
pub mod win32;

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::Mutex;

    /// An in-memory backend that hands out ids from 100 up.
    #[derive(Default)]
    pub struct FakeDisplays {
        pub state: Mutex<FakeState>,
    }

    #[derive(Default)]
    pub struct FakeState {
        pub unsupported: bool,
        pub fail: bool,
        pub next: u32,
        pub live: Vec<(u32, DisplaySpec)>,
        pub removed: Vec<u32>,
        /// Like Windows' driver: only the newest display can go.
        pub newest_only: bool,
    }

    impl FakeDisplays {
        pub fn unsupported() -> Self {
            let fake = Self::default();
            fake.state.lock().unwrap().unsupported = true;
            fake
        }

        pub fn live(&self) -> Vec<(u32, DisplaySpec)> { self.state.lock().unwrap().live.clone() }
    }

    impl VirtualDisplays for FakeDisplays {
        fn supported(&self) -> bool { !self.state.lock().unwrap().unsupported }

        fn create(&self, spec: &DisplaySpec) -> Result<AddedDisplay, VirtualDisplayError> {
            let mut state = self.state.lock().unwrap();
            if state.unsupported {
                return Err(VirtualDisplayError::Unsupported);
            }
            if state.fail {
                return Err(VirtualDisplayError::Failed("scripted".into()));
            }
            let id = 100 + state.next;
            state.next += 1;
            state.live.push((id, *spec));
            Ok(AddedDisplay { id, width: spec.width, height: spec.height })
        }

        fn removable(&self, id: u32) -> Result<(), VirtualDisplayError> {
            let state = self.state.lock().unwrap();
            let newest = state.live.last().map(|(live, _)| *live);
            if state.newest_only && state.live.iter().any(|(live, _)| *live == id) && newest != Some(id) {
                return Err(VirtualDisplayError::NotNewest);
            }
            Ok(())
        }

        fn remove(&self, id: u32) -> Result<(), VirtualDisplayError> {
            self.removable(id)?;
            let mut state = self.state.lock().unwrap();
            if let Some(index) = state.live.iter().position(|(live, _)| *live == id) {
                state.live.remove(index);
                state.removed.push(id);
            }
            Ok(())
        }

        fn owned(&self) -> Vec<u32> { self.state.lock().unwrap().live.iter().map(|(id, _)| *id).collect() }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeDisplays;
    use super::*;

    fn monitor(id: u32) -> MonitorInfo {
        MonitorInfo { id, name: format!("D{id}"), width: 0, height: 0, is_primary: false, is_virtual: false }
    }

    #[test]
    fn fitted_sizes_are_even_capped_and_keep_the_aspect() {
        assert_eq!(DisplaySpec::fitted(1281, 721, false), DisplaySpec { width: 1280, height: 720, refresh_hz: 60, hidpi: false });
        // A 5K window shrinks to fit 3840x2160 at the same 16:9 shape.
        let big = DisplaySpec::fitted(5120, 2880, true);
        assert_eq!((big.width, big.height, big.hidpi), (3840, 2160, true));
        // Ultra-wide: limited by width.
        let wide = DisplaySpec::fitted(6880, 2880, false);
        assert_eq!((wide.width, wide.height), (3840, 1606));
        assert_eq!(DisplaySpec::fitted(10, 10, false).width, MIN_WIDTH);
        assert_eq!(DisplaySpec::fitted(0, 0, false).height, MIN_HEIGHT);
        // 2x needs sizes divisible by four and at least 640x480 points.
        assert!(!DisplaySpec::fitted(1282, 1000, true).hidpi);
        assert!(!DisplaySpec::fitted(1280, 720, true).hidpi);
        assert!(DisplaySpec::fitted(2880, 1800, true).hidpi);
    }

    #[test]
    fn viewers_add_up_to_the_limit_and_only_remove_their_own() {
        let backend = Arc::new(FakeDisplays::default());
        let mut displays = SessionDisplays::new(backend.clone(), true);
        assert_eq!(displays.viewer_limit(), Some(MAX_PER_SESSION as u32));
        let spec = DisplaySpec::fitted(1280, 720, false);
        let first = displays.add(&spec).unwrap();
        let second = displays.add(&spec).unwrap();
        assert_eq!(displays.add(&spec), Err(control::VIRTUAL_DISPLAY_LIMIT));
        assert_eq!(displays.remove(7), Err(control::VIRTUAL_DISPLAY_UNKNOWN));
        displays.remove(first).unwrap();
        assert_eq!(displays.check_remove(first), Err(control::VIRTUAL_DISPLAY_UNKNOWN));
        assert_eq!(backend.owned(), vec![second]);
        // Freed a slot.
        displays.add(&spec).unwrap();
        assert_eq!(backend.owned().len(), 2);
    }

    #[test]
    fn ending_the_session_removes_every_display_it_made() {
        let backend = Arc::new(FakeDisplays::default());
        let mut displays = SessionDisplays::new(backend.clone(), true);
        let headless = displays.add_headless().unwrap();
        let added = displays.add(&DisplaySpec::fitted(1280, 720, false)).unwrap();
        // The headless display isn't the viewer's to remove.
        assert_eq!(displays.remove(headless), Err(control::VIRTUAL_DISPLAY_UNKNOWN));
        drop(displays);
        assert!(backend.owned().is_empty());
        let removed = backend.state.lock().unwrap().removed.clone();
        assert_eq!(removed, vec![added, headless]);
    }

    #[test]
    fn the_host_setting_and_platform_gate_everything() {
        let backend = Arc::new(FakeDisplays::default());
        let mut off = SessionDisplays::new(backend.clone(), false);
        assert_eq!(off.viewer_limit(), None);
        assert_eq!(off.add(&DisplaySpec::headless()), Err(control::VIRTUAL_DISPLAY_NOT_ALLOWED));
        assert_eq!(off.add_headless(), Err(VirtualDisplayError::Unsupported));

        let mut unsupported = SessionDisplays::new(Arc::new(Unsupported), true);
        assert_eq!(unsupported.viewer_limit(), None);
        assert_eq!(unsupported.add(&DisplaySpec::headless()), Err(control::VIRTUAL_DISPLAY_NOT_ALLOWED));

        backend.state.lock().unwrap().fail = true;
        let mut failing = SessionDisplays::new(backend.clone(), true);
        assert_eq!(failing.add(&DisplaySpec::headless()), Err(control::VIRTUAL_DISPLAY_FAILED));
        assert!(failing.is_empty());
    }

    #[test]
    fn only_viewer_added_displays_are_marked_virtual() {
        let backend = Arc::new(FakeDisplays::default());
        let mut displays = SessionDisplays::new(backend, true);
        let headless = displays.add_headless().unwrap();
        let added = displays.add(&DisplaySpec::headless()).unwrap();
        let mut list = vec![monitor(1), monitor(headless), monitor(added)];
        displays.annotate(&mut list);
        assert_eq!(list.iter().map(|monitor| monitor.is_virtual).collect::<Vec<_>>(), [false, false, true]);
    }

    #[test]
    fn annotated_displays_carry_the_size_they_really_got() {
        let mut displays = SessionDisplays::new(Arc::new(FakeDisplays::default()), true);
        let headless = displays.add_headless().unwrap();
        let added = displays.add(&DisplaySpec::fitted(1280, 720, false)).unwrap();
        let reported = MonitorInfo { width: 1600, height: 900, ..monitor(added) };
        let mut list = vec![monitor(1), monitor(headless), reported];
        displays.annotate(&mut list);
        let sizes: Vec<_> = list.iter().map(|monitor| (monitor.width, monitor.height)).collect();
        // Capture's own size wins; unknown sizes stay unknown for real screens.
        assert_eq!(sizes, [(0, 0), (1920, 1080), (1600, 900)]);
    }

    #[test]
    fn newest_first_backends_refuse_out_of_order_removal_and_clean_up_in_order() {
        let backend = Arc::new(FakeDisplays::default());
        backend.state.lock().unwrap().newest_only = true;
        let mut displays = SessionDisplays::new(backend.clone(), true);
        let headless = displays.add_headless().unwrap();
        let spec = DisplaySpec::fitted(1280, 720, false);
        let first = displays.add(&spec).unwrap();
        let second = displays.add(&spec).unwrap();
        assert_eq!(displays.check_remove(first), Err(control::VIRTUAL_DISPLAY_NOT_NEWEST));
        assert_eq!(displays.remove(first), Err(control::VIRTUAL_DISPLAY_NOT_NEWEST));
        assert_eq!(backend.owned(), vec![headless, first, second], "a refused removal leaves everything in place");
        assert_eq!(displays.check_remove(second), Ok(()));
        displays.remove(second).unwrap();
        assert_eq!(displays.check_remove(first), Ok(()));
        let third = displays.add(&spec).unwrap();
        drop(displays);
        assert!(backend.owned().is_empty());
        assert_eq!(backend.state.lock().unwrap().removed, vec![second, third, first, headless]);
    }

    #[test]
    fn platform_limits_become_refusal_codes() {
        assert_eq!(refusal_code(&VirtualDisplayError::Limit), control::VIRTUAL_DISPLAY_LIMIT);
        assert_eq!(refusal_code(&VirtualDisplayError::NotNewest), control::VIRTUAL_DISPLAY_NOT_NEWEST);
    }

    #[test]
    fn unsupported_platforms_say_so() {
        let error = Unsupported.create(&DisplaySpec::headless()).unwrap_err();
        assert_eq!(error, VirtualDisplayError::Unsupported);
        assert!(error.to_string().contains("not supported"));
    }
}
