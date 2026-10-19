//! A host that needs no desktop: it shares a test pattern and prints the
//! input it receives, for trying viewers (the browser viewer, the phone app)
//! where there is no screen to share, such as a server or a container.
//!
//! ```text
//! cargo run -p relay                                  # in another terminal
//! cargo run -p host-core --example demo_host -- [relay URL] [ID] [password]
//! ```
//!
//! Defaults: `ws://127.0.0.1:8080/ws`, `123456789`, `demo-password`. A
//! crosshair follows the viewer's pointer and turns white while a button is
//! down, so input can be checked by eye as well.

use host_core::{
    capture::ScreenCapturer,
    encode::{H264Encoder, PixelFormat, RawFrame},
    inject::InputInjector,
    relay_client,
    roles::run_host_session,
};
use protocol::input::InputEvent;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const WIDTH: u16 = 1280;
const HEIGHT: u16 = 720;

/// Where the viewer's pointer is, in pixels, and whether a button is down.
#[derive(Default)]
struct Pointer {
    x: u32,
    y: u32,
    down: bool,
}

/// Colour bars with a band that moves, so a frozen picture is easy to spot.
struct Pattern {
    pointer: Arc<Mutex<Pointer>>,
    started: Instant,
}

impl Pattern {
    fn draw(&self) -> Vec<u8> {
        const BARS: [[u8; 3]; 7] = [[192, 192, 192], [0, 192, 192], [192, 192, 0], [0, 192, 0], [192, 0, 192], [0, 0, 192], [192, 0, 0]];
        let (width, height) = (usize::from(WIDTH), usize::from(HEIGHT));
        let band = (self.started.elapsed().as_millis() as usize / 8) % width;
        let pointer = self.pointer.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let cross = if pointer.down { [255, 255, 255] } else { [0, 0, 0] };
        let mut data = vec![0u8; width * height * 4];
        for (y, row) in data.chunks_exact_mut(width * 4).enumerate() {
            for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let [r, g, b] = if x.abs_diff(pointer.x as usize) < 2 || y.abs_diff(pointer.y as usize) < 2 {
                    cross
                } else if x.abs_diff(band) < 6 {
                    [255, 255, 255]
                } else {
                    BARS[x * BARS.len() / width]
                };
                *pixel = [b, g, r, 255];
            }
        }
        data
    }
}

impl ScreenCapturer for Pattern {
    fn next_frame(&mut self) -> Option<RawFrame> {
        Some(RawFrame { width: WIDTH, height: HEIGHT, data: self.draw(), format: PixelFormat::Bgra, timestamp_us: 0 })
    }

    fn wait_frame(&mut self, timeout: Duration) -> Option<RawFrame> {
        std::thread::sleep(timeout.min(Duration::from_millis(33)));
        self.next_frame()
    }
}

/// Prints each input event and moves the crosshair.
struct Printer(Arc<Mutex<Pointer>>);

impl InputInjector for Printer {
    fn inject(&mut self, event: &InputEvent, screen_w: u32, screen_h: u32) {
        println!("input: {event:?}");
        let mut pointer = self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        match *event {
            InputEvent::MouseMove { x, y } => {
                pointer.x = (x.clamp(0, 65535) as u32) * screen_w.max(1) / 65535;
                pointer.y = (y.clamp(0, 65535) as u32) * screen_h.max(1) / 65535;
            }
            InputEvent::MouseButton { down, .. } => pointer.down = down,
            _ => {}
        }
    }
}

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args.next().unwrap_or_else(|| "ws://127.0.0.1:8080/ws".into());
    let id = args.next().unwrap_or_else(|| "123456789".into());
    let password = args.next().unwrap_or_else(|| "demo-password".into());
    let token = "de".repeat(32);
    loop {
        let mut ws = match relay_client::connect(&url).await {
            Ok(ws) => ws,
            Err(error) => {
                eprintln!("can't reach the relay at {url}: {error}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        if let Err(error) = relay_client::register(&mut ws, &id, &token).await {
            eprintln!("the relay refused ID {id}: {error}");
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }
        println!("hosting ID {id} on {url}, password {password}");
        let pointer = Arc::new(Mutex::new(Pointer::default()));
        let pattern = Pattern { pointer: Arc::clone(&pointer), started: Instant::now() };
        let result = run_host_session(ws, &password, "s1", pattern, |width, height| H264Encoder::new(width, height, 30, 2_000_000), Printer(pointer)).await;
        println!("session ended: {}", result.map_or_else(|error| error.to_string(), |()| "ok".into()));
    }
}
