use protocol::{clipboard::{ClipboardMsg, MAX_CLIPBOARD}, datamsg::DataMsg};

/// The OS clipboard operations needed by [`ClipboardSync`].
pub trait ClipboardBackend {
    fn read(&self) -> Option<String>;
    fn write(&self, text: &str);
}

/// Clipboard backend backed by the system clipboard through `arboard`.
pub struct ArboardBackend {
}

impl ArboardBackend {
    pub fn new() -> Option<Self> {
        arboard::Clipboard::new()
            .ok()
            .map(|_| Self {})
    }
}

impl ClipboardBackend for ArboardBackend {
    fn read(&self) -> Option<String> {
        // arboard's read operation requires mutable access even though this
        // abstraction deliberately keeps reads non-mutating for testability.
        // Re-opening the clipboard also avoids retaining stale OS handles.
        let mut clipboard = arboard::Clipboard::new().ok()?;
        clipboard.get_text().ok()
    }

    fn write(&self, text: &str) {
        // The backend is held by value so use a fresh handle for each write;
        // this keeps the trait's write method ergonomic and cross-platform.
        if let Ok(mut clipboard) = arboard::Clipboard::new() {
            let _ = clipboard.set_text(text.to_owned());
        }
    }
}

/// Applies remote clipboard updates and detects genuine local changes.
pub struct ClipboardSync<B: ClipboardBackend> {
    backend: B,
    last_seen: Option<String>,
    last_hash: Option<u64>,
    next_seq: u64,
}

impl<B: ClipboardBackend> ClipboardSync<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            last_seen: None,
            last_hash: None,
            next_seq: 0,
        }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Applies a remote update and remembers its hash to suppress the echo.
    pub fn apply(&mut self, text: &str) {
        self.backend.write(text);
        self.last_hash = Some(hash(text));
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

    /// Poll and wrap a locally generated clipboard update for the Data channel.
    pub fn poll_data(&mut self) -> Option<DataMsg> {
        self.poll().map(DataMsg::Clipboard)
    }
}

fn hash(text: &str) -> u64 {
    // FNV-1a is small, deterministic, and sufficient for echo detection.
    text.as_bytes().iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// In-memory backend useful for tests and callers that need a deterministic backend.
#[derive(Debug, Default)]
pub struct FakeBackend {
    text: std::cell::RefCell<Option<String>>,
}

impl FakeBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        *self.text.get_mut() = Some(text.into());
    }
}

impl ClipboardBackend for FakeBackend {
    fn read(&self) -> Option<String> {
        self.text.borrow().clone()
    }

    fn write(&self, text: &str) {
        *self.text.borrow_mut() = Some(text.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
