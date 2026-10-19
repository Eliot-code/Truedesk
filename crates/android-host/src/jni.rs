//! JNI entry points the Android app calls (class `com.truedesk.host.Native`).
//!
//! The app starts hosting, feeds screen frames in, takes touch actions out,
//! answers prompts and stops; status and prompts come back as JSON lines the
//! app polls. All hosting runs on one shared Tokio runtime.

use crate::host::{self, HostConfig, HostController, HostEvent, HostEvents};
use crate::input::Action;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jboolean, jint, jlong, JNI_FALSE, JNI_TRUE};
use jni::JNIEnv;
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;
use tokio::runtime::Runtime;

/// The hosting runtime and the live session, if any. Replaced on each start.
struct Hosting {
    controller: HostController,
    events: Arc<EventQueue>,
    /// Kept so the loop is aborted when hosting restarts or stops.
    task: tokio::task::JoinHandle<()>,
}

fn runtime() -> &'static Runtime {
    static RUNTIME: OnceLock<Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| Runtime::new().expect("tokio runtime"))
}

fn hosting() -> &'static Mutex<Option<Hosting>> {
    static HOSTING: OnceLock<Mutex<Option<Hosting>>> = OnceLock::new();
    HOSTING.get_or_init(|| Mutex::new(None))
}

/// Host events waiting for the UI to poll them.
#[derive(Default)]
struct EventQueue {
    queue: Mutex<VecDeque<String>>,
    ready: Condvar,
}

impl HostEvents for EventQueue {
    fn on(&self, event: HostEvent) {
        let mut queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        // Bound it: the UI usually keeps up, but if it stops polling, keep
        // the newest rather than grow without limit.
        if queue.len() >= 256 {
            queue.pop_front();
        }
        queue.push_back(event.to_json());
        self.ready.notify_one();
    }
}

impl EventQueue {
    fn next(&self, timeout: Duration) -> Option<String> {
        let queue = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        let (mut queue, _) = self
            .ready
            .wait_timeout_while(queue, timeout, |queue| queue.is_empty())
            .unwrap_or_else(|p| p.into_inner());
        queue.pop_front()
    }
}

fn take_string(env: &mut JNIEnv, value: &JString) -> String {
    env.get_string(value).map(Into::into).unwrap_or_default()
}

/// Splits a newline-separated list of relay URLs (the app joins them).
fn parse_relays(value: String) -> Vec<String> {
    value.lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect()
}

/// `nativeInit()`: route Rust logs to logcat. Safe to call more than once.
#[no_mangle]
pub extern "system" fn Java_com_truedesk_host_Native_nativeInit(_env: JNIEnv, _class: JClass) {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("truedesk-host"),
    );
}

/// `nativeStart(relays, id, token, password, unattended)`: start hosting,
/// replacing any current session. Returns false only if the arguments are
/// unusable (no relay, empty id/password).
#[no_mangle]
pub extern "system" fn Java_com_truedesk_host_Native_nativeStart<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    relays: JString<'local>,
    id: JString<'local>,
    token: JString<'local>,
    password: JString<'local>,
    unattended: jboolean,
) -> jboolean {
    let config = HostConfig {
        relay_urls: parse_relays(take_string(&mut env, &relays)),
        id: take_string(&mut env, &id),
        token: take_string(&mut env, &token),
        password: take_string(&mut env, &password),
        unattended: unattended == JNI_TRUE,
    };
    if config.relay_urls.is_empty() || config.id.is_empty() || config.password.is_empty() {
        return JNI_FALSE;
    }
    let events = Arc::new(EventQueue::default());
    let (controller, run) = host::host(config, events.clone() as Arc<dyn HostEvents>);
    let task = runtime().spawn(run);
    let previous = hosting()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .replace(Hosting { controller, events, task });
    if let Some(previous) = previous {
        previous.controller.stop();
        previous.task.abort();
    }
    JNI_TRUE
}

/// `nativeStop()`: end the session and stop hosting.
#[no_mangle]
pub extern "system" fn Java_com_truedesk_host_Native_nativeStop(_env: JNIEnv, _class: JClass) {
    if let Some(hosting) = hosting().lock().unwrap_or_else(|p| p.into_inner()).take() {
        hosting.controller.stop();
        hosting.task.abort();
    }
}

/// `nativeAnswer(allow)`: answer the current allow/deny prompt.
#[no_mangle]
pub extern "system" fn Java_com_truedesk_host_Native_nativeAnswer(_env: JNIEnv, _class: JClass, allow: jboolean) {
    if let Some(hosting) = &*hosting().lock().unwrap_or_else(|p| p.into_inner()) {
        hosting.controller.answer(allow == JNI_TRUE);
    }
}

/// `nativePushFrame(rgba, width, height, stride)`: one captured screen frame.
#[no_mangle]
pub extern "system" fn Java_com_truedesk_host_Native_nativePushFrame<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    rgba: JByteArray<'local>,
    width: jint,
    height: jint,
    stride: jint,
) {
    let frames = {
        let hosting = hosting().lock().unwrap_or_else(|p| p.into_inner());
        match &*hosting {
            Some(hosting) => hosting.controller.frames(),
            None => return,
        }
    };
    let (Ok(width), Ok(height)) = (u16::try_from(width), u16::try_from(height)) else { return };
    // elements() gives a view over the Java array without a copy; it is
    // committed back on drop, which we don't need but can't avoid.
    let Ok(bytes) = (unsafe { env.get_array_elements(&rgba, jni::objects::ReleaseMode::NoCopyBack) }) else {
        return;
    };
    // JNI bytes are i8; the pixels are the same octets.
    let pixels = unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u8>(), bytes.len()) };
    frames.push(width, height, pixels, stride.max(0) as usize);
}

/// `nativeNextAction(timeoutMs)`: the next touch action as a JSON line, or
/// null if none arrived within the timeout. The accessibility service loops
/// on this.
#[no_mangle]
pub extern "system" fn Java_com_truedesk_host_Native_nativeNextAction<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    timeout_ms: jlong,
) -> JString<'local> {
    poll(env, timeout_ms, |hosting| hosting.controller.actions().next(Duration::ZERO).as_ref().map(Action::to_json))
}

/// `nativeNextEvent(timeoutMs)`: the next UI event as a JSON line, or null.
#[no_mangle]
pub extern "system" fn Java_com_truedesk_host_Native_nativeNextEvent<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    timeout_ms: jlong,
) -> JString<'local> {
    poll(env, timeout_ms, |hosting| hosting.events.next(Duration::ZERO))
}

/// Shared body of the polling calls: wait up to `timeout_ms` for `pick` to
/// return a line, then hand it back as a Java string (null when none).
fn poll<'local>(
    env: JNIEnv<'local>,
    timeout_ms: jlong,
    pick: impl Fn(&Hosting) -> Option<String>,
) -> JString<'local> {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);
    let null = || JString::from(jni::objects::JObject::null());
    loop {
        let line = {
            let hosting = hosting().lock().unwrap_or_else(|p| p.into_inner());
            match &*hosting {
                Some(hosting) => pick(hosting),
                // Hosting stopped: nothing more to poll.
                None => return null(),
            }
        };
        if let Some(line) = line {
            return env.new_string(line).unwrap_or_else(|_| null());
        }
        if std::time::Instant::now() >= deadline {
            return null();
        }
        // Short sleeps rather than holding the lock across a wait, so start
        // and stop stay responsive while a service polls.
        std::thread::sleep(Duration::from_millis(15).min(deadline.saturating_duration_since(std::time::Instant::now())));
    }
}

/// Keeps the JNI symbols in the final shared library. A dependency crate's
/// `#[no_mangle]` functions are dropped unless something references them, and
/// Kotlin resolves these by name at runtime, so the app calls this once on
/// startup to pin them.
pub fn keep_linked() -> usize {
    let exports: [*const (); 7] = [
        Java_com_truedesk_host_Native_nativeInit as *const (),
        Java_com_truedesk_host_Native_nativeStart as *const (),
        Java_com_truedesk_host_Native_nativeStop as *const (),
        Java_com_truedesk_host_Native_nativeAnswer as *const (),
        Java_com_truedesk_host_Native_nativePushFrame as *const (),
        Java_com_truedesk_host_Native_nativeNextAction as *const (),
        Java_com_truedesk_host_Native_nativeNextEvent as *const (),
    ];
    exports.iter().map(|pointer| *pointer as usize).sum()
}
