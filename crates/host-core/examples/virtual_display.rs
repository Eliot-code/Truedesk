//! Manual check of the virtual display backend. macOS: adds a 1280x720
//! display and then a 2560x1440 HiDPI one, two seconds each, removes
//! them, and says whether screen capture listed them. Windows: see below.
//!
//! `cargo run -p host-core --example virtual_display`

#[cfg(target_os = "macos")]
fn main() {
    use host_core::vdisplay::{platform, DisplaySpec};
    use std::time::{Duration, Instant};

    #[repr(C)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGDisplayPixelsWide(display: u32) -> usize;
        fn CGDisplayPixelsHigh(display: u32) -> usize;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }

    fn listed_for_capture(id: u32) -> bool {
        // Listing without Screen Recording permission panics in scap.
        scap::has_permission() && scap::get_all_targets().iter().any(|target| matches!(target, scap::Target::Display(display) if display.id == id))
    }

    let displays = platform();
    if !displays.supported() {
        eprintln!("failed: CGVirtualDisplay is not available");
        std::process::exit(1);
    }
    for hidpi in [false, true] {
        let (width, height) = if hidpi { (2560, 1440) } else { (1280, 720) };
        let spec = DisplaySpec { width, height, refresh_hz: 60, hidpi };
        let id = match displays.create(&spec) {
            Ok(display) => display.id,
            Err(error) => {
                eprintln!("failed: {error}");
                std::process::exit(1);
            }
        };
        let started = Instant::now();
        let listed = loop {
            if listed_for_capture(id) {
                break Some(started.elapsed());
            }
            if started.elapsed() > Duration::from_secs(3) {
                break None;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        std::thread::sleep(Duration::from_secs(2));
        // SAFETY: plain CoreGraphics queries.
        let (pixels, bounds) = unsafe { ((CGDisplayPixelsWide(id), CGDisplayPixelsHigh(id)), CGDisplayBounds(id)) };
        println!(
            "hidpi={hidpi}: display {id} is {}x{} pixels, {}x{} points; capture lists it: {}",
            pixels.0,
            pixels.1,
            bounds.width,
            bounds.height,
            listed.map_or("no (is Screen Recording allowed?)".to_owned(), |after| format!("after {} ms", after.as_millis())),
        );
        // Remove and go straight on to the next one.
        let _ = displays.remove(id);
    }
    std::thread::sleep(Duration::from_secs(1));
    match displays.owned().as_slice() {
        [] => println!("ok: every display was removed"),
        left => {
            eprintln!("failed: still holding {left:?}");
            std::process::exit(1);
        }
    }
}

/// Windows: needs Amyuni's driver (Home → "Install virtual display
/// driver"). Adds a 1280x720 display and a 2560x1440 one, checks capture
/// lists them, that the older can't go first, and removes both.
#[cfg(windows)]
fn main() {
    use host_core::vdisplay::{platform, DisplaySpec, VirtualDisplayError};
    use std::time::Duration;

    let fail = |message: String| -> ! {
        eprintln!("failed: {message}");
        std::process::exit(1);
    };
    let displays = platform();
    if !displays.supported() {
        fail("the virtual display driver isn't installed".into());
    }
    let mut added = Vec::new();
    for (width, height) in [(1280, 720), (2560, 1440)] {
        let display = displays.create(&DisplaySpec { width, height, refresh_hz: 60, hidpi: false }).unwrap_or_else(|error| fail(error.to_string()));
        std::thread::sleep(Duration::from_secs(2));
        let listed = scap::get_all_targets().iter().any(|target| matches!(target, scap::Target::Display(shown) if shown.id == display.id));
        println!("asked for {width}x{height}: display {} is {}x{}; capture lists it: {listed}", display.id, display.width, display.height);
        added.push(display.id);
    }
    match displays.remove(added[0]) {
        Err(VirtualDisplayError::NotNewest) => println!("ok: the older display can't go first"),
        other => fail(format!("removing the older display first gave {other:?}")),
    }
    for id in added.into_iter().rev() {
        displays.remove(id).unwrap_or_else(|error| fail(error.to_string()));
        std::thread::sleep(Duration::from_secs(1));
    }
    match displays.owned().as_slice() {
        [] => println!("ok: every display was removed"),
        left => fail(format!("still holding {left:?}")),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn main() {
    eprintln!("virtual displays are only implemented on macOS and Windows");
}
