package com.truedesk.host

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.provider.Settings
import android.webkit.WebView
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class StartArgs {
    lateinit var relays: Array<String>
    lateinit var id: String
    var token: String = ""
    lateinit var password: String
    var unattended: Boolean = false
}

@InvokeArg
class AnswerArgs {
    var allow: Boolean = false
}

@InvokeArg
class PollArgs {
    var timeoutMs: Long = 0
}

/**
 * The Android side of sharing this device. The "Share this phone" screen in
 * the web UI drives it; this class asks the user for the screen-capture grant,
 * starts the capture foreground service and the hosting engine, and relays the
 * engine's events back. Touches are carried out by [ControlService], which the
 * user turns on in Accessibility settings.
 */
@TauriPlugin
class TruedeskHostPlugin(private val activity: Activity) : Plugin(activity) {
    @Volatile
    private var hosting = false
    private var pending: StartArgs? = null

    override fun load(webView: WebView) {
        Native.nativeInit()
    }

    @Command
    fun hostState(invoke: Invoke) {
        val result = JSObject()
        result.put("accessibilityEnabled", ControlService.isEnabled(activity))
        result.put("hosting", hosting)
        invoke.resolve(result)
    }

    @Command
    fun openAccessibilitySettings(invoke: Invoke) {
        val intent = Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        activity.startActivity(intent)
        invoke.resolve()
    }

    @Command
    fun startHosting(invoke: Invoke) {
        val args = invoke.parseArgs(StartArgs::class.java)
        // Input needs the accessibility service; without it the viewer could
        // see the screen but not touch it, so send the user to turn it on.
        if (!ControlService.isEnabled(activity)) {
            invoke.resolve(startResult(false, "accessibility"))
            return
        }
        pending = args
        val manager = activity.getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
        // The system shows its own "start recording / casting?" dialog here;
        // hosting only begins if the user allows it.
        startActivityForResult(invoke, manager.createScreenCaptureIntent(), "onProjectionResult")
    }

    @ActivityCallback
    fun onProjectionResult(invoke: Invoke, result: androidx.activity.result.ActivityResult) {
        val args = pending
        pending = null
        val data = result.data
        if (result.resultCode != Activity.RESULT_OK || data == null || args == null) {
            invoke.resolve(startResult(false, "projection_denied"))
            return
        }
        val ok = Native.nativeStart(
            args.relays.joinToString("\n"),
            args.id,
            args.token,
            args.password,
            args.unattended,
        )
        if (!ok) {
            invoke.resolve(startResult(false, "invalid"))
            return
        }
        CaptureService.start(activity, result.resultCode, data)
        hosting = true
        invoke.resolve(startResult(true, ""))
    }

    @Command
    fun stopHosting(invoke: Invoke) {
        hosting = false
        Native.nativeStop()
        CaptureService.stop(activity)
        invoke.resolve()
    }

    @Command
    fun answerRequest(invoke: Invoke) {
        val args = invoke.parseArgs(AnswerArgs::class.java)
        Native.nativeAnswer(args.allow)
        invoke.resolve()
    }

    @Command
    fun pollEvent(invoke: Invoke) {
        val args = invoke.parseArgs(PollArgs::class.java)
        // nativeNextEvent blocks up to the timeout, so wait off the main thread.
        Thread {
            val line = Native.nativeNextEvent(args.timeoutMs)
            val result = JSObject()
            if (line != null) {
                result.put("event", line)
            }
            invoke.resolve(result)
        }.start()
    }

    private fun startResult(started: Boolean, reason: String): JSObject {
        val result = JSObject()
        result.put("started", started)
        result.put("reason", reason)
        return result
    }
}
