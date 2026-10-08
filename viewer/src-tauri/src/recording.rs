//! Session recordings, streamed to disk chunk by chunk.

use super::*;

/// Maximum size of one session recording on disk.
pub(crate) const MAX_RECORDING_SIZE: usize = 2 * 1024 * 1024 * 1024;

#[allow(dead_code)]
pub(crate) fn recording_path(suggested_name: &str) -> PathBuf {
    downloads_dir().join(sanitize_recording_name(suggested_name))
}

pub(crate) fn validate_recording_size_len(size: usize) -> Result<(), String> {
    if size > MAX_RECORDING_SIZE {
        return Err(format!(
            "recording exceeds the {} GiB maximum size",
            MAX_RECORDING_SIZE / (1024 * 1024 * 1024)
        ));
    }
    Ok(())
}

pub(crate) fn recording_name_with_suffix(name: &str, suffix: usize) -> String {
    if suffix == 1 {
        return name.to_owned();
    }
    let stem = name.strip_suffix(".webm").unwrap_or(name);
    format!("{stem} ({suffix}).webm")
}

pub(crate) fn next_recording_path(directory: &Path, suggested_name: &str) -> Result<PathBuf, String> {
    let sanitized_name = sanitize_recording_name(suggested_name);
    for suffix in 1.. {
        let path = directory.join(recording_name_with_suffix(&sanitized_name, suffix));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(format!("refusing to write through symlink: {}", path.display()));
            }
            Ok(_) => continue,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(path),
            Err(error) => return Err(error.to_string()),
        }
    }
    unreachable!("recording filename suffix range is unbounded")
}

pub(crate) fn sanitize_recording_name(suggested_name: &str) -> String {
    let name = Path::new(suggested_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .replace('\\', "_");
    let sanitized: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let sanitized = sanitized.trim_matches('.');
    let sanitized = if sanitized.is_empty() { "recording" } else { sanitized };
    if sanitized.to_ascii_lowercase().ends_with(".webm") {
        sanitized.to_owned()
    } else {
        format!("{sanitized}.webm")
    }
}

/// A recording streamed to disk chunk by chunk, so long sessions never sit in
/// memory. The file is opened create-new (never follows an existing symlink
/// or overwrites a file) and capped at `MAX_RECORDING_SIZE`.
pub(crate) struct RecordingSink {
    pub(crate) file: fs::File,
    pub(crate) path: PathBuf,
    pub(crate) written: usize,
}

impl RecordingSink {
    pub(crate) fn create(directory: &Path, suggested_name: &str) -> Result<Self, String> {
        let mut path = next_recording_path(directory, suggested_name)?;
        let file = loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => break file,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    // A competing writer won the name race; choose another name
                    // and retain create-new semantics so no file is clobbered.
                    path = next_recording_path(directory, suggested_name)?;
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        if !file.metadata().map_err(|error| error.to_string())?.is_file() {
            return Err("recording target is not a regular file".into());
        }
        Ok(Self { file, path, written: 0 })
    }

    pub(crate) fn append(&mut self, bytes: &[u8]) -> Result<(), String> {
        let total = self.written.checked_add(bytes.len()).ok_or("recording size overflow")?;
        validate_recording_size_len(total)?;
        io::Write::write_all(&mut self.file, bytes).map_err(|error| error.to_string())?;
        self.written = total;
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<PathBuf, String> {
        self.file.sync_all().map_err(|error| error.to_string())?;
        Ok(self.path)
    }
}

/// Recordings in progress, keyed by the id handed to the viewer UI.
#[derive(Default)]
pub(crate) struct Recordings {
    pub(crate) next_id: std::sync::atomic::AtomicU32,
    pub(crate) sinks: Mutex<HashMap<u32, RecordingSink>>,
}

impl Recordings {
    pub(crate) fn sinks(&self) -> std::sync::MutexGuard<'_, HashMap<u32, RecordingSink>> {
        self.sinks.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

pub(crate) fn recordings_dir() -> Result<PathBuf, String> {
    let directory = downloads_dir();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let metadata = fs::symlink_metadata(&directory).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("recording Downloads path must be a regular directory".into());
    }
    fs::canonicalize(&directory).map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) fn start_recording(recordings: tauri::State<'_, Recordings>, suggested_name: String) -> Result<u32, String> {
    let sink = RecordingSink::create(&recordings_dir()?, &suggested_name)?;
    let id = recordings.next_id.fetch_add(1, Ordering::Relaxed);
    recordings.sinks().insert(id, sink);
    Ok(id)
}

/// `chunk` is base64, like the frame events, to keep large payloads off the
/// JSON number-array path.
#[tauri::command]
pub(crate) fn append_recording(recordings: tauri::State<'_, Recordings>, id: u32, chunk: String) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD.decode(chunk).map_err(|error| error.to_string())?;
    let mut sinks = recordings.sinks();
    let sink = sinks.get_mut(&id).ok_or("unknown recording")?;
    if let Err(error) = sink.append(&bytes) {
        // Keep what was written so far; stop accepting more for this id.
        if let Some(sink) = sinks.remove(&id) { let _ = sink.finish(); }
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn finish_recording(recordings: tauri::State<'_, Recordings>, id: u32) -> Result<String, String> {
    let sink = recordings.sinks().remove(&id).ok_or("unknown recording")?;
    Ok(sink.finish()?.to_string_lossy().into_owned())
}
