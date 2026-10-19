//! Tauri plugin that lets a Truedesk viewer see and control this Android
//! device. The UI (the mobile app's "Share this phone" screen) drives it
//! through the plugin commands; the real work is in the Kotlin side
//! (`TruedeskHostPlugin`), which captures the screen with MediaProjection,
//! carries out touches with an accessibility service, and runs the hosting
//! engine from `android-host` over JNI.
//!
//! Only Android does anything: a phone host needs to inject input, which
//! iOS forbids, so every other platform answers each command with
//! [`Error::Unsupported`] and the app and its tests still build everywhere.

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

mod commands;
mod error;
mod models;

pub use error::{Error, Result};
pub use models::*;

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.truedesk.host";

/// Access to the phone-hosting commands, managed as app state.
pub struct TruedeskHost<R: Runtime> {
    #[cfg(target_os = "android")]
    handle: tauri::plugin::PluginHandle<R>,
    #[cfg(not(target_os = "android"))]
    _marker: std::marker::PhantomData<fn() -> R>,
}

#[cfg(target_os = "android")]
impl<R: Runtime> TruedeskHost<R> {
    fn call<T: serde::de::DeserializeOwned>(&self, command: &str, args: impl serde::Serialize) -> Result<T> {
        self.handle.run_mobile_plugin(command, args).map_err(Into::into)
    }

    pub fn host_state(&self) -> Result<HostState> {
        self.call("hostState", ())
    }

    pub fn open_accessibility_settings(&self) -> Result<()> {
        self.call("openAccessibilitySettings", ())
    }

    pub fn start_hosting(&self, args: StartArgs) -> Result<StartResult> {
        self.call("startHosting", args)
    }

    pub fn stop_hosting(&self) -> Result<()> {
        self.call("stopHosting", ())
    }

    pub fn answer_request(&self, args: AnswerArgs) -> Result<()> {
        self.call("answerRequest", args)
    }

    pub fn poll_event(&self, args: PollArgs) -> Result<PollResult> {
        self.call("pollEvent", args)
    }
}

#[cfg(not(target_os = "android"))]
impl<R: Runtime> TruedeskHost<R> {
    pub fn host_state(&self) -> Result<HostState> {
        Ok(HostState::default())
    }

    pub fn open_accessibility_settings(&self) -> Result<()> {
        Err(Error::Unsupported)
    }

    pub fn start_hosting(&self, _args: StartArgs) -> Result<StartResult> {
        Err(Error::Unsupported)
    }

    pub fn stop_hosting(&self) -> Result<()> {
        Err(Error::Unsupported)
    }

    pub fn answer_request(&self, _args: AnswerArgs) -> Result<()> {
        Err(Error::Unsupported)
    }

    pub fn poll_event(&self, _args: PollArgs) -> Result<PollResult> {
        Ok(PollResult::default())
    }
}

/// The app's handle on the plugin.
pub trait TruedeskHostExt<R: Runtime> {
    fn truedesk_host(&self) -> &TruedeskHost<R>;
}

impl<R: Runtime, T: Manager<R>> TruedeskHostExt<R> for T {
    fn truedesk_host(&self) -> &TruedeskHost<R> {
        self.state::<TruedeskHost<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("truedesk-host")
        .invoke_handler(tauri::generate_handler![
            commands::host_state,
            commands::open_accessibility_settings,
            commands::start_hosting,
            commands::stop_hosting,
            commands::answer_request,
            commands::poll_event,
        ])
        .setup(|app, _api| {
            #[cfg(target_os = "android")]
            {
                let handle = _api.register_android_plugin(PLUGIN_IDENTIFIER, "TruedeskHostPlugin")?;
                app.manage(TruedeskHost { handle });
                // Pin the JNI exports Kotlin resolves at runtime.
                let _ = android_host::jni::keep_linked();
            }
            #[cfg(not(target_os = "android"))]
            {
                app.manage(TruedeskHost::<R> { _marker: std::marker::PhantomData });
            }
            Ok(())
        })
        .build()
}
