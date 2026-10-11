//! Streaming file transfer for the reliable, sealed data channel.
//!
//! The caller owns transport and crypto: it seals/opens `FileMsg` bytes and
//! implements [`FileSink`] and [`FileSource`]. The sender hashes the file in a
//! first pass so the receiver has the expected SHA-256 in the initial offer,
//! then reopens it for the bounded-memory chunking pass.

use async_trait::async_trait;
use protocol::filexfer::FileMsg;
use sha2::{Digest, Sha256};
use std::{
    error::Error,
    future::Future,
    fmt::{self, Display, Formatter},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const CHUNK_SIZE: usize = 64 * 1024;

#[async_trait]
pub trait FileSink {
    async fn send(&mut self, msg: FileMsg);
}

#[async_trait]
pub trait FileSource {
    async fn recv(&mut self) -> Option<FileMsg>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentDecision {
    Accept,
    Reject,
}

#[derive(Debug)]
pub enum FileError {
    Io(io::Error),
    HashMismatch,
    Rejected,
    BadName,
    Protocol(String),
}

impl Display for FileError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "file transfer I/O error: {error}"),
            Self::HashMismatch => f.write_str("file transfer hash mismatch"),
            Self::Rejected => f.write_str("file transfer rejected"),
            Self::BadName => f.write_str("invalid file name"),
            Self::Protocol(message) => write!(f, "file transfer protocol error: {message}"),
        }
    }
}

impl Error for FileError {}

impl From<io::Error> for FileError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Return a filename safe to append to a trusted downloads directory.
///
/// Besides path traversal, this rejects names that could disguise what the
/// file is or where it lands: invisible/bidi format characters (e.g. U+202E
/// turning "invoice\u{202E}fdp.exe" into "invoiceexe.pdf"), hidden dotfiles,
/// trailing dots/spaces (silently stripped by Windows), ':' (Windows
/// drive-relative paths and NTFS alternate data streams) and Windows reserved
/// device names.
pub fn sanitize_filename(name: &str) -> Option<String> {
    if name.is_empty()
        || name.len() > 255
        || name.starts_with('.')
        || name.ends_with('.')
        || name.ends_with(' ')
        || name.contains('/')
        || name.contains('\\')
        || name.contains(':')
        || name.contains('\0')
        || Path::new(name).is_absolute()
        || name.chars().any(|c| c.is_control() || is_format_char(c))
        || is_windows_reserved_name(name)
    {
        return None;
    }
    Some(name.to_owned())
}

/// Unicode general category Cf (format) characters: invisible marks, bidi
/// controls and tags that can make a displayed name differ from the real one.
fn is_format_char(c: char) -> bool {
    matches!(
        c as u32,
        0x00AD
            | 0x0600..=0x0605
            | 0x061C
            | 0x06DD
            | 0x070F
            | 0x0890..=0x0891
            | 0x08E2
            | 0x180E
            | 0x200B..=0x200F
            | 0x202A..=0x202E
            | 0x2060..=0x2064
            | 0x2066..=0x206F
            | 0xFEFF
            | 0xFFF9..=0xFFFB
            | 0x110BD
            | 0x110CD
            | 0x13430..=0x1343F
            | 0x1BCA0..=0x1BCA3
            | 0x1D173..=0x1D17A
            | 0xE0001
            | 0xE0020..=0xE007F
    )
}

/// Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9),
/// case-insensitively and with any extension ("nul.txt", "Com1.tar.gz").
fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            let bytes = upper.as_bytes();
            bytes.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && (b'1'..=b'9').contains(&bytes[3])
        }
    }
}

/// Mark a received file as downloaded from an untrusted source so the OS
/// applies its usual first-open checks (Gatekeeper on macOS, SmartScreen /
/// Mark-of-the-Web on Windows). Failures are logged, never fatal.
fn quarantine(path: &Path) {
    if let Err(error) = apply_quarantine(path) {
        log::warn!("could not mark received file {} as quarantined: {error}", path.display());
    }
}

#[cfg(target_os = "macos")]
fn apply_quarantine(path: &Path) -> io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    // 0081 = downloaded, not yet user-approved; then time, agent, no UUID.
    let value = format!("0081;{now:x};OpenDesk;");
    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let c_name = c"com.apple.quarantine";
    // SAFETY: `c_path` and `c_name` are valid NUL-terminated C strings and
    // `value` is a live buffer of `value.len()` bytes; all outlive the call.
    // setxattr does not retain any of the pointers.
    let result = unsafe {
        libc::setxattr(
            c_path.as_ptr(),
            c_name.as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    if result == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(windows)]
fn apply_quarantine(path: &Path) -> io::Result<()> {
    let mut stream = path.as_os_str().to_owned();
    stream.push(":Zone.Identifier");
    // ZoneId=3 is the Internet zone (Mark-of-the-Web).
    std::fs::write(stream, "[ZoneTransfer]\r\nZoneId=3\r\n")
}

#[cfg(not(any(target_os = "macos", windows)))]
fn apply_quarantine(_path: &Path) -> io::Result<()> {
    Ok(())
}

pub struct FileSender;

impl FileSender {
    pub async fn send<S, P>(
        path: P,
        sink: &mut S,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), FileError>
    where
        S: FileSink + Send,
        P: AsRef<Path>,
    {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let path = path.as_ref();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(sanitize_filename)
            .ok_or(FileError::BadName)?;

        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0u8; CHUNK_SIZE];
        let mut size = 0u64;
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            size = size
                .checked_add(read as u64)
                .ok_or_else(|| FileError::Protocol("file is too large".into()))?;
        }
        let hash: [u8; 32] = hasher.finalize().into();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        sink.send(FileMsg::Offer {
            id,
            name,
            size,
            hash,
        })
        .await;

        let mut file = tokio::fs::File::open(path).await?;
        let mut offset = 0u64;
        progress(0, size);
        loop {
            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            sink.send(FileMsg::Chunk {
                id,
                offset,
                data: buffer[..read].to_vec(),
            })
            .await;
            offset += read as u64;
            progress(offset, size);
        }
        sink.send(FileMsg::Done { id }).await;
        Ok(())
    }
}

pub struct FileReceiver;

impl FileReceiver {
    pub async fn receive<S>(
        source: &mut S,
        downloads_dir: &Path,
        consent: impl FnOnce(&FileMsg) -> ConsentDecision,
        progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, FileError>
    where
        S: FileSource + Send,
    {
        Self::receive_with_async_consent(source, downloads_dir, |offer| async move {
            consent(&offer)
        }, progress)
        .await
    }

    /// Receive a file while obtaining consent without blocking a Tokio worker.
    ///
    /// This is the integration seam for UI consent: the offer callback may
    /// await a command or channel response before any file is created.
    pub async fn receive_with_async_consent<S, C, ConsentFuture>(
        source: &mut S,
        downloads_dir: &Path,
        consent: C,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<PathBuf, FileError>
    where
        S: FileSource + Send,
        C: FnOnce(FileMsg) -> ConsentFuture,
        ConsentFuture: Future<Output = ConsentDecision>,
    {
        let offer = source
            .recv()
            .await
            .ok_or_else(|| FileError::Protocol("source closed before offer".into()))?;
        let (id, name, size, expected_hash) = match &offer {
            FileMsg::Offer {
                id,
                name,
                size,
                hash,
            } => (*id, sanitize_filename(name).ok_or(FileError::BadName)?, *size, *hash),
            _ => return Err(FileError::Protocol("expected offer".into())),
        };
        if consent(offer.clone()).await == ConsentDecision::Reject {
            return Err(FileError::Rejected);
        }

        tokio::fs::create_dir_all(downloads_dir).await?;
        let (temp_path, temp_file) = create_temp_path(downloads_dir, &name).await?;
        let result = Self::receive_to_temp(
            source,
            temp_file,
            id,
            size,
            expected_hash,
            &mut progress,
        )
        .await;
        match result {
            Ok(()) => {
                let final_path = match unique_final_path(downloads_dir, &name).await {
                    Ok(path) => path,
                    Err(error) => {
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        return Err(error);
                    }
                };
                // Defense in depth: the sanitized name must never escape the
                // downloads directory.
                if final_path.parent() != Some(downloads_dir) {
                    let _ = tokio::fs::remove_file(&final_path).await;
                    let _ = tokio::fs::remove_file(&temp_path).await;
                    return Err(FileError::BadName);
                }
                match tokio::fs::rename(&temp_path, &final_path).await {
                    Ok(()) => {
                        quarantine(&final_path);
                        Ok(final_path)
                    }
                    Err(error) => {
                        let _ = tokio::fs::remove_file(&final_path).await;
                        Err(error.into())
                    }
                }
            }
            Err(error) => {
                let _ = tokio::fs::remove_file(&temp_path).await;
                Err(error)
            }
        }
    }

    async fn receive_to_temp<S>(
        source: &mut S,
        mut file: tokio::fs::File,
        id: u64,
        size: u64,
        expected_hash: [u8; 32],
        progress: &mut impl FnMut(u64, u64),
    ) -> Result<(), FileError>
    where
        S: FileSource + Send,
    {
        let mut hasher = Sha256::new();
        let mut expected_offset = 0u64;
        progress(0, size);
        loop {
            let message = source
                .recv()
                .await
                .ok_or_else(|| FileError::Protocol("source closed mid-transfer".into()))?;
            match message {
                FileMsg::Chunk {
                    id: chunk_id,
                    offset,
                    data,
                } if chunk_id == id => {
                    if offset != expected_offset || data.len() > CHUNK_SIZE {
                        return Err(FileError::Protocol("invalid chunk ordering or size".into()));
                    }
                    let new_offset = expected_offset
                        .checked_add(data.len() as u64)
                        .ok_or_else(|| FileError::Protocol("chunk offset overflow".into()))?;
                    if new_offset > size {
                        return Err(FileError::Protocol("received more bytes than offered".into()));
                    }
                    file.write_all(&data).await?;
                    hasher.update(&data);
                    expected_offset = new_offset;
                    progress(expected_offset, size);
                }
                FileMsg::Done { id: done_id } if done_id == id => {
                    if expected_offset != size {
                        return Err(FileError::Protocol("transfer ended at the wrong size".into()));
                    }
                    file.flush().await?;
                    let actual_hash: [u8; 32] = hasher.finalize().into();
                    if actual_hash != expected_hash {
                        return Err(FileError::HashMismatch);
                    }
                    return Ok(());
                }
                FileMsg::Error { id: error_id, .. } if error_id == id => {
                    return Err(FileError::Protocol("sender reported an error".into()));
                }
                _ => return Err(FileError::Protocol("unexpected file message".into())),
            }
        }
    }
}

async fn create_temp_path(dir: &Path, name: &str) -> Result<(PathBuf, tokio::fs::File), FileError> {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);
    for _ in 0..100 {
        let suffix = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!(".{name}.{suffix}.part"));
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => {
                return Ok((path, file));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FileError::Io(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a temporary file",
    )))
}

async fn unique_final_path(dir: &Path, name: &str) -> Result<PathBuf, FileError> {
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let extension = path.extension().and_then(|s| s.to_str());
    for index in 0..u64::MAX {
        let candidate_name = match extension {
            Some(_extension) if index == 0 => name.to_owned(),
            Some(extension) => format!("{stem} ({index}).{extension}"),
            None if index == 0 => name.to_owned(),
            None => format!("{stem} ({index})"),
        };
        let candidate = dir.join(candidate_name);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
            .await
        {
            Ok(file) => {
                // The empty file reserves this name against concurrent
                // receivers. The caller replaces this reservation with the
                // verified temporary file.
                drop(file);
                return Ok(candidate);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(FileError::Protocol("could not choose a destination name".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};
    use tokio::sync::mpsc;

    struct Sink(mpsc::Sender<FileMsg>);
    struct Source(mpsc::Receiver<FileMsg>);

    #[async_trait]
    impl FileSink for Sink {
        async fn send(&mut self, msg: FileMsg) { self.0.send(msg).await.expect("receiver exists"); }
    }

    #[async_trait]
    impl FileSource for Source {
        async fn recv(&mut self) -> Option<FileMsg> { self.0.recv().await }
    }

    fn temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("opendesk-filexfer-{label}-{nanos}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn sanitizes_filenames() {
        for name in ["../etc/passwd", "/abs/x", "..", "a/b", ""] {
            assert_eq!(sanitize_filename(name), None);
        }
        assert_eq!(sanitize_filename("report.pdf"), Some("report.pdf".into()));
        assert_eq!(sanitize_filename("my file.txt"), Some("my file.txt".into()));
        assert_eq!(sanitize_filename(&"a".repeat(256)), None);
        assert_eq!(sanitize_filename("line\nfeed.txt"), None);
        assert_eq!(sanitize_filename("tab\tfile.txt"), None);
    }

    #[test]
    fn rejects_spoofing_format_characters() {
        for name in [
            "invoice\u{202E}fdp.exe",
            "a\u{200E}.txt",
            "a\u{200F}.txt",
            "a\u{2066}b\u{2069}.txt",
            "\u{FEFF}report.pdf",
            "zero\u{200B}width.txt",
            "soft\u{00AD}hyphen.txt",
            "tag\u{E0041}.txt",
        ] {
            assert_eq!(sanitize_filename(name), None, "{name:?}");
        }
        assert_eq!(sanitize_filename("résumé 履歴書.pdf"), Some("résumé 履歴書.pdf".into()));
    }

    #[test]
    fn rejects_hidden_and_trailing_dot_or_space_names() {
        for name in [".", "..", ".bashrc", ".hidden.txt", "file.", "file ", "file.txt.", "file.txt "] {
            assert_eq!(sanitize_filename(name), None, "{name:?}");
        }
        assert_eq!(sanitize_filename("archive.tar.gz"), Some("archive.tar.gz".into()));
    }

    #[test]
    fn rejects_colons_and_streams() {
        for name in ["C:evil.exe", "a.txt:stream", "a.txt::$DATA", "c:"] {
            assert_eq!(sanitize_filename(name), None, "{name:?}");
        }
    }

    #[test]
    fn rejects_windows_reserved_device_names() {
        for name in [
            "CON", "con", "Prn", "aux.txt", "NUL.tar.gz", "com1", "COM9.log", "lpt1", "Lpt9.txt",
            "nul .txt",
        ] {
            assert_eq!(sanitize_filename(name), None, "{name:?}");
        }
        for name in ["console.txt", "com0.txt", "com10", "lpt", "auxiliary.doc", "nullable.rs", "my con.txt"] {
            assert_eq!(sanitize_filename(name), Some(name.to_owned()), "{name:?}");
        }
    }

    #[test]
    fn quarantine_is_best_effort() {
        let dir = temp_dir("quarantine");
        let file = dir.join("received.txt");
        fs::write(&file, b"x").unwrap();
        // Never panics, even for a missing file.
        quarantine(&file);
        quarantine(&dir.join("missing.txt"));
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("xattr")
                .args(["-p", "com.apple.quarantine"])
                .arg(&file)
                .output()
                .unwrap();
            let value = String::from_utf8_lossy(&output.stdout);
            assert!(value.starts_with("0081;") && value.trim_end().ends_with(";OpenDesk;"), "{value}");
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn final_path_reservation_is_unique() {
        let dir = temp_dir("unique-final");
        let first = unique_final_path(&dir, "report.pdf").await.unwrap();
        let second = unique_final_path(&dir, "report.pdf").await.unwrap();
        assert_eq!(first.file_name().unwrap(), "report.pdf");
        assert_eq!(second.file_name().unwrap(), "report (1).pdf");
        fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn final_path_reservation_does_not_follow_symlinks() {
        use std::os::unix::fs::symlink;
        let dir = temp_dir("symlink-final");
        let target = dir.join("outside.txt");
        fs::write(&target, b"keep me").unwrap();
        symlink(&target, dir.join("report.pdf")).unwrap();

        let reserved = unique_final_path(&dir, "report.pdf").await.unwrap();
        assert_eq!(reserved.file_name().unwrap(), "report (1).pdf");
        assert_eq!(fs::read(&target).unwrap(), b"keep me");
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn round_trip_streams_and_verifies() {
        let dir = temp_dir("roundtrip");
        let source_path = dir.join("report.pdf");
        let bytes = (0..(CHUNK_SIZE * 2 + 17)).map(|n| (n % 251) as u8).collect::<Vec<_>>();
        fs::write(&source_path, &bytes).unwrap();
        let (tx, rx) = mpsc::channel(4);
        let sender = tokio::spawn(async move { FileSender::send(source_path, &mut Sink(tx), |_, _| {}).await });
        let received = FileReceiver::receive(&mut Source(rx), &dir, |_| ConsentDecision::Accept, |_, _| {}).await.unwrap();
        sender.await.unwrap().unwrap();
        assert_eq!(fs::read(received).unwrap(), bytes);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn wrong_hash_discards_temp_file() {
        let dir = temp_dir("tamper");
        let (tx, rx) = mpsc::channel(4);
        tx.send(FileMsg::Offer { id: 1, name: "x.bin".into(), size: 3, hash: [0; 32] }).await.unwrap();
        tx.send(FileMsg::Chunk { id: 1, offset: 0, data: vec![1, 2, 4] }).await.unwrap();
        tx.send(FileMsg::Done { id: 1 }).await.unwrap();
        let result = FileReceiver::receive(&mut Source(rx), &dir, |_| ConsentDecision::Accept, |_, _| {}).await;
        assert!(matches!(result, Err(FileError::HashMismatch)));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn rejected_transfer_writes_nothing() {
        let dir = temp_dir("reject");
        let (tx, rx) = mpsc::channel(1);
        tx.send(FileMsg::Offer { id: 1, name: "x.bin".into(), size: 3, hash: [0; 32] }).await.unwrap();
        let result = FileReceiver::receive(&mut Source(rx), &dir, |_| ConsentDecision::Reject, |_, _| {}).await;
        assert!(matches!(result, Err(FileError::Rejected)));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
    }
}
