use crate::{AnswerArgs, HostState, PollArgs, PollResult, Result, StartArgs, StartResult, TruedeskHostExt};
use tauri::{AppHandle, Runtime};

#[tauri::command]
pub(crate) async fn host_state<R: Runtime>(app: AppHandle<R>) -> Result<HostState> {
    app.truedesk_host().host_state()
}

#[tauri::command]
pub(crate) async fn open_accessibility_settings<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    app.truedesk_host().open_accessibility_settings()
}

#[tauri::command]
pub(crate) async fn start_hosting<R: Runtime>(app: AppHandle<R>, args: StartArgs) -> Result<StartResult> {
    app.truedesk_host().start_hosting(args)
}

#[tauri::command]
pub(crate) async fn stop_hosting<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    app.truedesk_host().stop_hosting()
}

#[tauri::command]
pub(crate) async fn answer_request<R: Runtime>(app: AppHandle<R>, args: AnswerArgs) -> Result<()> {
    app.truedesk_host().answer_request(args)
}

#[tauri::command]
pub(crate) async fn poll_event<R: Runtime>(app: AppHandle<R>, args: PollArgs) -> Result<PollResult> {
    app.truedesk_host().poll_event(args)
}
