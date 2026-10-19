package com.truedesk.host

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.ComponentName
import android.content.Context
import android.graphics.Path
import android.os.Bundle
import android.provider.Settings
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Carries out the viewer's input on this device. It pulls one action at a
 * time from the hosting engine (as JSON lines) and performs it: strokes and
 * scrolls as gestures, typing and editing on the focused field, and the
 * system Back/Home/Recents buttons.
 *
 * The user turns this on in Settings > Accessibility, and it only does
 * anything while a session is hosting (the engine hands out no actions
 * otherwise). No screen content leaves this service.
 */
class ControlService : AccessibilityService() {
    @Volatile
    private var running = false
    private var worker: Thread? = null

    override fun onServiceConnected() {
        running = true
        worker = Thread({ loop() }, "truedesk-control").also { it.start() }
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {}

    override fun onInterrupt() {}

    override fun onDestroy() {
        running = false
        worker?.interrupt()
        worker = null
        super.onDestroy()
    }

    /** Pulls actions and performs them until the service is torn down. */
    private fun loop() {
        while (running) {
            val line = try {
                Native.nativeNextAction(250)
            } catch (_: Throwable) {
                null
            }
            if (line != null) {
                try {
                    perform(line)
                } catch (_: Throwable) {
                    // One bad action should not stop the loop.
                }
            }
        }
    }

    private fun perform(line: String) {
        val json = JSONObject(line)
        when (json.optString("t")) {
            "stroke" -> stroke(json.optJSONArray("p"), json.optLong("ms", 1))
            "scroll" -> scroll(json)
            "text" -> type(json.optString("s"))
            "edit" -> edit(json.optString("k"))
            "global" -> global(json.optString("k"))
        }
    }

    private fun stroke(points: JSONArray?, ms: Long) {
        if (points == null || points.length() == 0) return
        val width = resources.displayMetrics.widthPixels
        val height = resources.displayMetrics.heightPixels
        val path = Path()
        for (i in 0 until points.length()) {
            val point = points.optJSONArray(i) ?: continue
            val x = clamp(point.optDouble(0) * width, width)
            val y = clamp(point.optDouble(1) * height, height)
            if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }
        // A tap is one point; Android needs a non-zero path, so nudge 1px.
        if (points.length() == 1) {
            val point = points.optJSONArray(0)!!
            val x = clamp(point.optDouble(0) * width, width)
            val y = clamp(point.optDouble(1) * height, height)
            path.lineTo(clamp((x + 1).toDouble(), width), y)
        }
        dispatchBlocking(path, ms.coerceIn(1, 59_000))
    }

    private fun scroll(json: JSONObject) {
        val width = resources.displayMetrics.widthPixels
        val height = resources.displayMetrics.heightPixels
        val x = clamp(json.optDouble("x") * width, width)
        val y = clamp(json.optDouble("y") * height, height)
        // Content moves by (dx, dy) screens, so the finger swipes the other way.
        val endX = clamp(x - json.optDouble("dx") * width, width)
        val endY = clamp(y - json.optDouble("dy") * height, height)
        val path = Path()
        path.moveTo(x, y)
        path.lineTo(endX, endY)
        dispatchBlocking(path, 250)
    }

    private fun dispatchBlocking(path: Path, duration: Long) {
        val gesture = GestureDescription.Builder()
            .addStroke(GestureDescription.StrokeDescription(path, 0, duration))
            .build()
        val done = CountDownLatch(1)
        val dispatched = dispatchGesture(gesture, object : GestureResultCallback() {
            override fun onCompleted(description: GestureDescription?) = done.countDown()
            override fun onCancelled(description: GestureDescription?) = done.countDown()
        }, null)
        if (dispatched) {
            done.await(duration + 1000, TimeUnit.MILLISECONDS)
        }
    }

    private fun type(text: String) {
        if (text.isEmpty()) return
        val node = focusedEditable() ?: return
        val current = node.text?.toString() ?: ""
        replaceText(node, current + text, (current + text).length)
    }

    private fun edit(key: String) {
        val node = focusedEditable() ?: return
        val text = node.text?.toString() ?: ""
        val caret = node.textSelectionEnd.let { if (it in 0..text.length) it else text.length }
        when (key) {
            "backspace" -> if (caret > 0) replaceText(node, text.removeRange(caret - 1, caret), caret - 1)
            "delete" -> if (caret < text.length) replaceText(node, text.removeRange(caret, caret + 1), caret)
            "enter" -> replaceText(node, text.substring(0, caret) + "\n" + text.substring(caret), caret + 1)
            "left" -> select(node, (caret - 1).coerceAtLeast(0), (caret - 1).coerceAtLeast(0))
            "right" -> select(node, (caret + 1).coerceAtMost(text.length), (caret + 1).coerceAtMost(text.length))
            "home" -> select(node, 0, 0)
            "end" -> select(node, text.length, text.length)
            "select_all" -> select(node, 0, text.length)
            "copy" -> node.performAction(AccessibilityNodeInfo.ACTION_COPY)
            "cut" -> node.performAction(AccessibilityNodeInfo.ACTION_CUT)
            "paste" -> node.performAction(AccessibilityNodeInfo.ACTION_PASTE)
        }
    }

    private fun global(key: String) {
        val action = when (key) {
            "back" -> GLOBAL_ACTION_BACK
            "home" -> GLOBAL_ACTION_HOME
            "recents" -> GLOBAL_ACTION_RECENTS
            "notifications" -> GLOBAL_ACTION_NOTIFICATIONS
            else -> return
        }
        performGlobalAction(action)
    }

    private fun focusedEditable(): AccessibilityNodeInfo? {
        val node = findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return null
        return if (node.isEditable) node else null
    }

    private fun replaceText(node: AccessibilityNodeInfo, text: String, caret: Int) {
        val args = Bundle()
        args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text)
        node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)
        select(node, caret, caret)
    }

    private fun select(node: AccessibilityNodeInfo, start: Int, end: Int) {
        val args = Bundle()
        args.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, start)
        args.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, end)
        node.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, args)
    }

    private fun clamp(value: Double, size: Int): Float =
        value.coerceIn(0.0, (size - 1).toDouble()).toFloat()

    companion object {
        /** Whether the user has turned this service on in Accessibility settings. */
        fun isEnabled(context: Context): Boolean {
            val expected = ComponentName(context, ControlService::class.java).flattenToString()
            val enabled = Settings.Secure.getString(
                context.contentResolver,
                Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES,
            ) ?: return false
            return enabled.split(':').any { it.equals(expected, ignoreCase = true) }
        }
    }
}
