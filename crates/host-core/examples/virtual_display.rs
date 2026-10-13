//! Manual check of the macOS virtual display backend: adds a 1280x720
//! display and then a 2560x1440 HiDPI one, two seconds each, removes
//! them, and says whether screen capture listed them.
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
            Ok(id) => id,
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
        displays.remove(id);
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

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("virtual displays are only implemented on macOS");
}
