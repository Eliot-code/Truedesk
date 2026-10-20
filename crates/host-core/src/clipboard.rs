use protocol::{
    clipboard::{ClipboardImageMsg, ClipboardMsg, MAX_CLIPBOARD, MAX_CLIPBOARD_IMAGE},
    datamsg::DataMsg,
};

/// An image on the clipboard, as raw RGBA8 rows (`width * height * 4` bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Largest image we will decode from a peer, in bytes of RGBA. A small PNG can
/// expand to an enormous bitmap, so cap the decoded size, not just the encoded
/// bytes, to avoid a decompression bomb.
const MAX_DECODED_IMAGE: usize = 64 * 1024 * 1024;

/// The OS clipboard operations needed by [`ClipboardSync`]. The image methods
/// default to "no images", so a text-only backend needs only the first two.
pub trait ClipboardBackend {
    fn read(&self) -> Option<String>;
    fn write(&self, text: &str);
    fn read_image(&self) -> Option<ClipboardImage> {
        None
    }
    fn write_image(&self, _image: &ClipboardImage) {}
}

/// Clipboard backend backed by the system clipboard through `arboard`.
///
/// The `arboard::Clipboard` handle is kept alive for the backend's whole
/// lifetime. On X11/Wayland the clipboard selection is served by the process
/// that owns the live handle; a per-operation handle that drops immediately
/// after `set_text` leaves the selection unowned and the pasted text vanishes.
/// The backend lives entirely on the dedicated clipboard thread, so the
/// non-`Send`, single-thread-affine handle stays on one thread; `RefCell`
/// gives the `&self` trait methods the interior mutability arboard needs.
#[cfg(not(target_os = "android"))]
pub struct ArboardBackend {
    clipboard: std::cell::RefCell<arboard::Clipboard>,
}

#[cfg(not(target_os = "android"))]
impl ArboardBackend {
    pub fn new() -> Option<Self> {
        arboard::Clipboard::new()
            .ok()
            .map(|clipboard| Self {
                clipboard: std::cell::RefCell::new(clipboard),
            })
    }
}

#[cfg(not(target_os = "android"))]
impl ClipboardBackend for ArboardBackend {
    fn read(&self) -> Option<String> {
        self.clipboard.borrow_mut().get_text().ok()
    }

    fn write(&self, text: &str) {
        let _ = self.clipboard.borrow_mut().set_text(text.to_owned());
    }

    fn read_image(&self) -> Option<ClipboardImage> {
        let image = self.clipboard.borrow_mut().get_image().ok()?;
        Some(ClipboardImage { width: image.width, height: image.height, rgba: image.bytes.into_owned() })
    }

    fn write_image(&self, image: &ClipboardImage) {
        let data = arboard::ImageData {
            width: image.width,
            height: image.height,
            bytes: std::borrow::Cow::Borrowed(&image.rgba),
        };
        let _ = self.clipboard.borrow_mut().set_image(data);
    }
}

/// Encodes an RGBA image to PNG for transport. `None` if the dimensions and
/// byte length don't match, or encoding fails.
pub fn encode_png(image: &ClipboardImage) -> Option<Vec<u8>> {
    let width = u32::try_from(image.width).ok()?;
    let height = u32::try_from(image.height).ok()?;
    if image.rgba.len() != image.width.checked_mul(image.height)?.checked_mul(4)? {
        return None;
    }
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&image.rgba).ok()?;
    }
    Some(out)
}

/// Decodes a PNG from a peer back to RGBA. Accepts only 8-bit RGBA (what we
/// send) and refuses anything that would decode larger than
/// [`MAX_DECODED_IMAGE`].
pub fn decode_png(bytes: &[u8]) -> Option<ClipboardImage> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let (color, depth, width, height) = {
        let info = reader.info();
        (info.color_type, info.bit_depth, info.width as usize, info.height as usize)
    };
    if color != png::ColorType::Rgba || depth != png::BitDepth::Eight {
        return None;
    }
    if width.checked_mul(height)?.checked_mul(4)? > MAX_DECODED_IMAGE {
        return None;
    }
    let mut rgba = vec![0u8; reader.output_buffer_size()?];
    reader.next_frame(&mut rgba).ok()?;
    // Our PNGs are single-frame RGBA8, so the image is exactly this many bytes.
    rgba.truncate(width.saturating_mul(height).saturating_mul(4));
    Some(ClipboardImage { width, height, rgba })
}

/// Applies remote clipboard updates and detects genuine local changes, for
/// both text and images.
pub struct ClipboardSync<B: ClipboardBackend> {
    backend: B,
    last_seen: Option<String>,
    last_hash: Option<u64>,
    next_seq: u64,
    last_seen_image: Option<u64>,
    last_image_hash: Option<u64>,
    next_image_seq: u64,
}

impl<B: ClipboardBackend> ClipboardSync<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            last_seen: None,
            last_hash: None,
            next_seq: 0,
            last_seen_image: None,
            last_image_hash: None,
            next_image_seq: 0,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Applies a remote text update and remembers its hash to suppress the echo.
    pub fn apply(&mut self, text: &str) {
        self.backend.write(text);
        self.last_hash = Some(hash(text));
    }

    /// Applies a remote image update (PNG bytes) and suppresses its echo.
    pub fn apply_image(&mut self, png: &[u8]) {
        let Some(image) = decode_png(png) else { return };
        let image_hash = hash_bytes(&image.rgba);
        self.backend.write_image(&image);
        self.last_image_hash = Some(image_hash);
        // So the next poll reads it back as "unchanged" rather than as new.
        self.last_seen_image = Some(image_hash);
    }

    /// Polls for a local clipboard change.
    ///
    /// The first successful read establishes a baseline and does not emit a
    /// message. Oversize reads are remembered as seen but skipped.
    pub fn poll(&mut self) -> Option<ClipboardMsg> {
        let text = self.backend.read()?;
        if self.last_seen.is_none() {
            self.last_seen = Some(text);
            return None;
        }
        if self.last_seen.as_deref() == Some(text.as_str()) {
            return None;
        }
        self.last_seen = Some(text.clone());

        let text_hash = hash(&text);
        if self.last_hash == Some(text_hash) || text.len() > MAX_CLIPBOARD {
            return None;
        }

        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        Some(ClipboardMsg { seq, text })
    }

    /// Polls for a local clipboard image change, encoding it to PNG. Like
    /// [`poll`](Self::poll): the first read is a baseline, echoes and oversize
    /// images are skipped.
    pub fn poll_image(&mut self) -> Option<ClipboardImageMsg> {
        let image = self.backend.read_image()?;
        let image_hash = hash_bytes(&image.rgba);
        if self.last_seen_image.is_none() {
            self.last_seen_image = Some(image_hash);
            return None;
        }
        if self.last_seen_image == Some(image_hash) {
            return None;
        }
        self.last_seen_image = Some(image_hash);
        if self.last_image_hash == Some(image_hash) {
            return None;
        }
        let png = encode_png(&image)?;
        if png.len() > MAX_CLIPBOARD_IMAGE {
            return None;
        }
        let seq = self.next_image_seq;
        self.next_image_seq = self.next_image_seq.wrapping_add(1);
        Some(ClipboardImageMsg { seq, image: png })
    }

    /// Poll and wrap a locally generated text update for the Data channel.
    pub fn poll_data(&mut self) -> Option<DataMsg> {
        self.poll().map(DataMsg::Clipboard)
    }

    /// Poll and wrap a locally generated image update for the Data channel.
    pub fn poll_image_data(&mut self) -> Option<DataMsg> {
        self.poll_image().map(DataMsg::ClipboardImage)
    }
}

fn hash(text: &str) -> u64 {
    hash_bytes(text.as_bytes())
}

fn hash_bytes(bytes: &[u8]) -> u64 {
    // FNV-1a is small, deterministic, and sufficient for echo detection.
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// In-memory backend useful for tests and callers that need a deterministic backend.
#[derive(Debug, Default)]
pub struct FakeBackend {
    text: std::cell::RefCell<Option<String>>,
    image: std::cell::RefCell<Option<ClipboardImage>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        *self.text.get_mut() = Some(text.into());
    }

    pub fn set_image(&mut self, image: ClipboardImage) {
        *self.image.get_mut() = Some(image);
    }
}

impl ClipboardBackend for FakeBackend {
    fn read(&self) -> Option<String> {
        self.text.borrow().clone()
    }

    fn write(&self, text: &str) {
        *self.text.borrow_mut() = Some(text.to_owned());
    }

    fn read_image(&self) -> Option<ClipboardImage> {
        self.image.borrow().clone()
    }

    fn write_image(&self, image: &ClipboardImage) {
        *self.image.borrow_mut() = Some(image.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(fill: u8) -> ClipboardImage {
        ClipboardImage { width: 2, height: 2, rgba: vec![fill; 2 * 2 * 4] }
    }

    #[test]
    fn echo_is_suppressed() {
        let mut sync = ClipboardSync::new(FakeBackend::new());
        sync.apply("hello");
        sync.backend_mut().set_text("hello");
        assert_eq!(sync.poll(), None);
    }

    #[test]
    fn local_change_is_detected() {
        let mut backend = FakeBackend::new();
        backend.set_text("hello");
        let mut sync = ClipboardSync::new(backend);
        assert_eq!(sync.poll(), None);
        sync.backend_mut().set_text("world");
        assert_eq!(sync.poll(), Some(ClipboardMsg { seq: 0, text: "world".into() }));
    }

    #[test]
    fn repeated_polls_do_not_emit() {
        let mut backend = FakeBackend::new();
        backend.set_text("initial");
        let mut sync = ClipboardSync::new(backend);
        assert_eq!(sync.poll(), None);
        assert_eq!(sync.poll(), None);
    }

    #[test]
    fn oversize_text_is_skipped_without_panicking() {
        let mut backend = FakeBackend::new();
        backend.set_text("x".repeat(MAX_CLIPBOARD + 1));
        let mut sync = ClipboardSync::new(backend);
        assert_eq!(sync.poll(), None);
    }

    #[test]
    fn sequence_increments_across_distinct_sends() {
        let mut backend = FakeBackend::new();
        backend.set_text("one");
        let mut sync = ClipboardSync::new(backend);
        assert_eq!(sync.poll(), None);
        sync.backend_mut().set_text("two");
        assert_eq!(sync.poll().map(|message| message.seq), Some(0));
        sync.backend_mut().set_text("three");
        assert_eq!(sync.poll().map(|message| message.seq), Some(1));
    }

    #[test]
    fn images_round_trip_through_png() {
        let original = ClipboardImage { width: 2, height: 1, rgba: vec![10, 20, 30, 255, 40, 50, 60, 128] };
        let png = encode_png(&original).unwrap();
        assert_eq!(decode_png(&png), Some(original));
    }

    #[test]
    fn a_local_image_change_is_detected_then_the_echo_suppressed() {
        let mut backend = FakeBackend::new();
        backend.set_image(image(1));
        let mut sync = ClipboardSync::new(backend);
        // First read is the baseline.
        assert!(sync.poll_image().is_none());
        // A different image is emitted once.
        sync.backend_mut().set_image(image(2));
        let message = sync.poll_image().expect("image change should emit");
        assert_eq!(message.seq, 0);
        assert!(sync.poll_image().is_none());
        // Applying a remote image and reading it back does not echo.
        let remote = encode_png(&image(9)).unwrap();
        sync.apply_image(&remote);
        assert!(sync.poll_image().is_none());
    }

    #[test]
    fn decode_rejects_non_png_bytes() {
        assert_eq!(decode_png(b"not a png"), None);
    }
}
