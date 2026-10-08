//! File transfer over the session data channel.

use super::*;

/// A FileSink backed by the session's reliable, sealed Data channel.
pub(crate) struct ReliableFileSink {
    pub(crate) outgoing: ReliableSender,
    pub(crate) sealer: Arc<Mutex<Sealer>>,
    pub(crate) offer_id: Option<oneshot::Sender<Result<u64, String>>>,
    pub(crate) transfer_id: Arc<Mutex<Option<u64>>>,
}

#[async_trait::async_trait]
impl FileSink for ReliableFileSink {
    async fn send(&mut self, message: FileMsg) {
        if let FileMsg::Offer { id, .. } = &message {
            *lock_unpoisoned(&self.transfer_id) = Some(*id);
            if let Some(sender) = self.offer_id.take() {
                let _ = sender.send(Ok(*id));
            }
        }
        let plaintext = DataMsg::FileTransfer(message).to_bytes();
        let sealed = lock_unpoisoned(&self.sealer).seal(&[Tag::Data as u8], &plaintext);
        let _ = self.outgoing.send_frame(Tag::Data, &sealed).await;
    }
}

impl ReliableFileSink {
    pub(crate) fn fail_before_offer(&mut self, error: &str) {
        if let Some(sender) = self.offer_id.take() {
            let _ = sender.send(Err(error.to_owned()));
        }
    }
}

pub(crate) struct ReliableFileSource {
    pub(crate) incoming: mpsc::Receiver<FileMsg>,
}

#[async_trait::async_trait]
impl FileSource for ReliableFileSource {
    async fn recv(&mut self) -> Option<FileMsg> {
        self.incoming.recv().await
    }
}

pub(crate) fn downloads_dir() -> PathBuf {
    if let Ok(path) = std::env::var("XDG_DOWNLOAD_DIR") {
        if !path.trim().is_empty() {
            return PathBuf::from(path);
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    home.map(|path| path.join("Downloads"))
        .unwrap_or_else(|| PathBuf::from("Downloads"))
}

pub(crate) fn route_file_consent(
    pending: &mut HashMap<u64, oneshot::Sender<bool>>,
    id: u64,
    accept: bool,
) -> Result<(), String> {
    pending
        .remove(&id)
        .ok_or_else(|| "no incoming file offer with that id".to_owned())?
        .send(accept)
        .map_err(|_| "incoming file offer expired".to_owned())
}

/// Start the receiver half once the session loop has a relay Data channel.
/// Consent is awaited asynchronously before FileReceiver creates a temporary
/// file, so no Tokio worker is blocked by the UI response.
pub(crate) fn spawn_file_receiver(
    app: AppHandle,
    incoming: mpsc::Receiver<FileMsg>,
    from: String,
) {
    tauri::async_runtime::spawn(async move {
        let mut source = ReliableFileSource { incoming };
        let pending_app = app.clone();
        let transfer_id = Arc::new(Mutex::new(None));
        let progress_id = transfer_id.clone();
        let offer_id = transfer_id.clone();
        let result = FileReceiver::receive_with_async_consent(
            &mut source,
            &downloads_dir(),
            move |offer| async move {
                let FileMsg::Offer { id, name, size, .. } = offer else {
                    return ConsentDecision::Reject;
                };
                *lock_unpoisoned(&offer_id) = Some(id);
                let (decision_tx, decision_rx) = oneshot::channel();
                pending_app
                    .state::<AppState>()
                    .lock()
                    .pending_file_requests
                    .insert(id, decision_tx);
                emit(
                    &pending_app,
                    "file_offer",
                    FileOffer {
                        id,
                        name: name.clone(),
                        size,
                        from: from.clone(),
                    },
                );

                match decision_rx.await.unwrap_or(false) {
                    true => ConsentDecision::Accept,
                    false => ConsentDecision::Reject,
                }
            },
            {
                let progress_app = app.clone();
                move |transferred, total| {
                    if let Some(id) = *lock_unpoisoned(&progress_id) {
                        emit(
                            &progress_app,
                            "file_progress",
                            FileProgress {
                                id,
                                transferred,
                                total,
                                direction: "receive",
                            },
                        );
                    }
                }
            },
        )
        .await;
        if let Err(error) = result {
            emit(
                &app,
                "file_error",
                FileError {
                    id: (*lock_unpoisoned(&transfer_id)).unwrap_or(0),
                    message: error.to_string(),
                },
            );
        } else if let Some(id) = *lock_unpoisoned(&transfer_id) {
            emit(&app, "file_done", FileDone { id });
        }
    });
}

#[tauri::command]
pub(crate) async fn send_file(
    state: tauri::State<'_, AppState>,
    app: AppHandle,
    path: String,
) -> Result<u64, String> {
    let (outgoing, sealer) = {
        let state = state.lock();
        (
        state.file_outgoing.clone(),
        state.file_sealer.clone(),
        )
    };
    let outgoing = outgoing.ok_or_else(|| "no active reliable file channel".to_owned())?;
    let sealer = sealer.ok_or_else(|| "file session keys are not ready".to_owned())?;
    let (offer_tx, offer_rx) = oneshot::channel();
    let task_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let transfer_id = Arc::new(Mutex::new(None));
        let progress_id = transfer_id.clone();
        let progress_app = task_app.clone();
        let mut sink = ReliableFileSink {
            outgoing,
            sealer,
            offer_id: Some(offer_tx),
            transfer_id: transfer_id.clone(),
        };
        let result = FileSender::send(Path::new(&path), &mut sink, move |transferred, total| {
            if let Some(id) = *lock_unpoisoned(&progress_id) {
                emit(
                    &progress_app,
                    "file_progress",
                    FileProgress {
                        id,
                        transferred,
                        total,
                        direction: "send",
                    },
                );
            }
        })
        .await;
        if let Err(error) = result {
            sink.fail_before_offer(&error.to_string());
            let id = *lock_unpoisoned(&transfer_id);
            emit(
                &task_app,
                "file_error",
                FileError {
                    id: id.unwrap_or(0),
                    message: error.to_string(),
                },
            );
        } else if let Some(id) = *lock_unpoisoned(&transfer_id) {
            emit(&task_app, "file_done", FileDone { id });
        }
    });
    // FileSender allocates the id when it emits Offer. Returning after that
    // first sealed frame lets the UI track a transfer while chunks continue.
    let id = offer_rx
        .await
        .map_err(|_| "file transfer stopped before sending its offer".to_owned())??;
    Ok(id)
}

#[tauri::command]
pub(crate) fn respond_file(
    state: tauri::State<'_, AppState>,
    id: u64,
    accept: bool,
) -> Result<(), String> {
    route_file_consent(
        &mut state.lock().pending_file_requests,
        id,
        accept,
    )
}
