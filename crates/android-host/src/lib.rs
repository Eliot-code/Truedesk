//! Hosting a phone: let a Truedesk viewer see and control an Android device.
//!
//! The Android app captures the screen (MediaProjection) and pushes frames to
//! [`capture::FramePusher`], runs the hosting loop in [`host`], and carries
//! out the touch [`input::Action`]s with an accessibility service. The pieces
//! are plain Rust so the mapping and the session run and are tested on the
//! desktop; [`jni`] wires them to Kotlin on Android.

pub mod capture;
pub mod host;
pub mod input;

#[cfg(target_os = "android")]
mod jni;

pub use capture::{FramePusher, PushCapturer};
pub use host::{host, HostConfig, HostController, HostEvent, HostEvents, HostStatus};
pub use input::{Action, ActionQueue, Edit, GestureInjector, Global};
