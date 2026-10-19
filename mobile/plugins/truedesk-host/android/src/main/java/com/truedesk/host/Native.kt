package com.truedesk.host

/**
 * Bindings to the Rust hosting engine (`android-host`), which is linked into
 * the app's library. The names resolve to the engine's JNI exports
 * (`Java_com_truedesk_host_Native_*`); the Rust side pins them so they survive
 * linking.
 *
 * `nextAction` and `nextEvent` block up to the given timeout, so callers run
 * them off the main thread.
 */
internal object Native {
    init {
        // Already loaded by the Tauri activity; loading again is a no-op but
        // guarantees the symbols are present whichever thread touches them.
        System.loadLibrary("truedesk_mobile_lib")
    }

    /** Route the engine's logs to logcat. Safe to call more than once. */
    external fun nativeInit()

    /**
     * Start hosting, replacing any current session. Returns false only when
     * the arguments are unusable (no relay, empty id or password).
     */
    external fun nativeStart(
        relays: String,
        id: String,
        token: String,
        password: String,
        unattended: Boolean,
    ): Boolean

    /** End the session and stop hosting. */
    external fun nativeStop()

    /** Answer the current allow/deny prompt. */
    external fun nativeAnswer(allow: Boolean)

    /** Hand one captured screen frame (RGBA, `stride` bytes per row) to the engine. */
    external fun nativePushFrame(rgba: ByteArray, width: Int, height: Int, stride: Int)

    /** The next touch action as a JSON line, or null if none arrived in time. */
    external fun nativeNextAction(timeoutMs: Long): String?

    /** The next UI event as a JSON line, or null if none arrived in time. */
    external fun nativeNextEvent(timeoutMs: Long): String?
}
