use protocol::input::InputEvent;

pub fn norm_to_pixel(coord: i32, screen: u32) -> i32 {
    let max_pixel = i64::from(screen).saturating_sub(1);
    ((i64::from(coord).clamp(0, 65_535) * max_pixel + 32_767) / 65_535) as i32
}

/// A display's rectangle in the global desktop space the pointer moves in:
/// points on macOS (origin at the main display's top-left, negative for
/// displays left of or above it), virtual-screen pixels on Windows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Maps protocol pointer coordinates (0..=65535 across the shared display)
/// onto `bounds`, clamped to the display's last point.
///
/// The viewer normalizes against the video, whose pixel size (Retina-native
/// or scaled to the stream size) says nothing about the units the OS moves
/// the pointer in, so only the display's own bounds give the right spot.
pub fn map_to_bounds(x: i32, y: i32, bounds: DisplayBounds) -> (i32, i32) {
    let axis = |coord: i32, origin: f64, size: f64| {
        let fraction = f64::from(coord.clamp(0, 65_535)) / 65_535.0;
        let last = (origin + size - 1.0).max(origin);
        (origin + fraction * size).clamp(origin, last).floor() as i32
    };
    (axis(x, bounds.x, bounds.width), axis(y, bounds.y, bounds.height))
}

pub trait InputInjector {
    fn inject(&mut self, ev: &InputEvent, screen_w: u32, screen_h: u32);

    /// The shared display changed: `Some(id)` is a monitor id from the
    /// capturer's list, `None` the main display. Pointer moves land on it.
    fn select_display(&mut self, _display: Option<u32>) {}
}

impl<T: InputInjector + ?Sized> InputInjector for Box<T> {
    fn inject(&mut self, ev: &InputEvent, screen_w: u32, screen_h: u32) {
        (**self).inject(ev, screen_w, screen_h);
    }

    fn select_display(&mut self, display: Option<u32>) {
        (**self).select_display(display);
    }
}

// Desktop input goes through enigo. Phones inject input outside this crate
// (the Android app) through their own InputInjector.
#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(not(target_os = "android"))]
pub use desktop::EnigoInjector;

#[derive(Default)]
pub struct NoopInjector {
    count: usize,
}

impl NoopInjector {
    pub fn count(&self) -> usize {
        self.count
    }
}

impl InputInjector for NoopInjector {
    fn inject(&mut self, _ev: &InputEvent, _screen_w: u32, _screen_h: u32) {
        self.count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_to_pixel_maps_endpoints() {
        assert_eq!(norm_to_pixel(0, 1920), 0);
        assert_eq!(norm_to_pixel(65535, 1920), 1919);
        assert_eq!(norm_to_pixel(32768, 1000), 500);
    }

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> DisplayBounds {
        DisplayBounds { x, y, width, height }
    }

    #[test]
    fn map_to_bounds_uses_points_not_capture_pixels() {
        // A Retina MacBook: 2880x1800 pixels, 1440x900 points. The video may
        // be 2880 or 1920 wide; the pointer still moves in points.
        let retina = bounds(0.0, 0.0, 1440.0, 900.0);
        assert_eq!(map_to_bounds(0, 0, retina), (0, 0));
        assert_eq!(map_to_bounds(32_768, 32_768, retina), (720, 450));
        assert_eq!(map_to_bounds(65_535, 65_535, retina), (1439, 899));
    }

    #[test]
    fn map_to_bounds_offsets_secondary_displays() {
        // Right of the main display.
        let right = bounds(1440.0, 0.0, 1920.0, 1080.0);
        assert_eq!(map_to_bounds(0, 0, right), (1440, 0));
        assert_eq!(map_to_bounds(65_535, 65_535, right), (3359, 1079));
        // Left of and above the main display: negative origin.
        let left = bounds(-1920.0, -1080.0, 1920.0, 1080.0);
        assert_eq!(map_to_bounds(0, 0, left), (-1920, -1080));
        assert_eq!(map_to_bounds(32_768, 32_768, left), (-960, -540));
        assert_eq!(map_to_bounds(65_535, 65_535, left), (-1, -1));
    }

    #[test]
    fn map_to_bounds_clamps_out_of_range_input() {
        let left = bounds(-1280.0, 0.0, 1280.0, 800.0);
        assert_eq!(map_to_bounds(-5, -5, left), (-1280, 0));
        assert_eq!(map_to_bounds(70_000, i32::MAX, left), (-1, 799));
        assert_eq!(map_to_bounds(i32::MIN, 65_535, left), (-1280, 799));
    }

    #[test]
    fn map_to_bounds_handles_fractional_bounds() {
        // Scaled display modes can report non-integral point sizes.
        let scaled = bounds(0.0, 0.0, 1496.5, 967.5);
        assert_eq!(map_to_bounds(65_535, 65_535, scaled), (1495, 966));
        assert_eq!(map_to_bounds(0, 0, scaled), (0, 0));
    }

    #[test]
    fn noop_injector_records_events() {
        let mut inj = NoopInjector::default();
        inj.inject(
            &protocol::input::InputEvent::MouseButton {
                button: 1,
                down: true,
            },
            1920,
            1080,
        );
        assert_eq!(inj.count(), 1);
    }

}
