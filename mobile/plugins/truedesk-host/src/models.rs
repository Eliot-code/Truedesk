use serde::{Deserialize, Serialize};

/// What the UI passes to start sharing the phone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartArgs {
    /// Relays to try, in order.
    pub relays: Vec<String>,
    /// This phone's device ID.
    pub id: String,
    /// The registration token for the ID.
    pub token: String,
    /// The password a viewer must enter.
    pub password: String,
    /// Accept viewers without asking. Off by default.
    #[serde(default)]
    pub unattended: bool,
}

/// Whether the phone is ready to host and whether it is hosting now.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostState {
    /// The accessibility service (which carries out touches) is on.
    pub accessibility_enabled: bool,
    /// A session or the registration loop is running.
    pub hosting: bool,
}

/// The result of asking to start: either hosting began, or a reason it did
/// not that the UI acts on (open accessibility settings, retry the capture
/// permission).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartResult {
    pub started: bool,
    /// One of "accessibility", "projection_denied", "" (started).
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnswerArgs {
    pub allow: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollArgs {
    /// How long to wait for an event, milliseconds.
    #[serde(default)]
    pub timeout_ms: u32,
}

/// The next host event as a JSON line (see `android_host::HostEvent`), or
/// none if the wait ran out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PollResult {
    #[serde(default)]
    pub event: Option<String>,
}
