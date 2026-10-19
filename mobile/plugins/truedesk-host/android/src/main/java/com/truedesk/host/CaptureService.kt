package com.truedesk.host

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.util.DisplayMetrics
import android.view.WindowManager

/**
 * Holds the screen-capture grant the user gave and feeds the hosting engine a
 * frame whenever the screen changes. It runs in the foreground with an ongoing
 * notification, as Android requires for MediaProjection, so the user always
 * sees that the screen is being shared.
 */
class CaptureService : Service() {
    private var projection: MediaProjection? = null
    private var virtualDisplay: VirtualDisplay? = null
    private var imageReader: ImageReader? = null
    private var thread: HandlerThread? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent == null || intent.action == ACTION_STOP) {
            stopSelf()
            return START_NOT_STICKY
        }

        startInForeground()

        val code = intent.getIntExtra(EXTRA_CODE, 0)
        val data = intent.getParcelableExtra<Intent>(EXTRA_DATA)
        val manager = getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
        val projection = if (data != null) manager.getMediaProjection(code, data) else null
        if (projection == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        this.projection = projection
        // On recent Android the projection must have a registered callback,
        // and we stop cleanly if the system revokes it.
        projection.registerCallback(object : MediaProjection.Callback() {
            override fun onStop() {
                stopSelf()
            }
        }, Handler(mainLooper))

        startCapture(projection)
        return START_NOT_STICKY
    }

    private fun startCapture(projection: MediaProjection) {
        val metrics = screenMetrics()
        val (width, height) = captureSize(metrics.widthPixels, metrics.heightPixels)
        val thread = HandlerThread("truedesk-capture").also { it.start() }
        this.thread = thread
        val handler = Handler(thread.looper)

        val reader = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 2)
        reader.setOnImageAvailableListener({ r ->
            val image = r.acquireLatestImage() ?: return@setOnImageAvailableListener
            try {
                val plane = image.planes[0]
                val buffer = plane.buffer
                val bytes = ByteArray(buffer.remaining())
                buffer.get(bytes)
                Native.nativePushFrame(bytes, image.width, image.height, plane.rowStride)
            } catch (_: Throwable) {
                // A dropped frame is not worth crashing the service over.
            } finally {
                image.close()
            }
        }, handler)
        this.imageReader = reader

        virtualDisplay = projection.createVirtualDisplay(
            "truedesk",
            width,
            height,
            metrics.densityDpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
            reader.surface,
            null,
            handler,
        )
    }

    private fun startInForeground() {
        val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Screen sharing", NotificationManager.IMPORTANCE_LOW)
        )
        val notification: Notification = Notification.Builder(this, CHANNEL)
            .setContentTitle("Truedesk")
            .setContentText("Sharing this screen")
            .setSmallIcon(applicationInfo.icon)
            .setOngoing(true)
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    @Suppress("DEPRECATION")
    private fun screenMetrics(): DisplayMetrics {
        val metrics = DisplayMetrics()
        val window = getSystemService(Context.WINDOW_SERVICE) as WindowManager
        window.defaultDisplay.getRealMetrics(metrics)
        return metrics
    }

    override fun onDestroy() {
        virtualDisplay?.release()
        imageReader?.close()
        projection?.stop()
        thread?.quitSafely()
        virtualDisplay = null
        imageReader = null
        projection = null
        thread = null
        super.onDestroy()
    }

    companion object {
        private const val CHANNEL = "truedesk_host"
        private const val NOTIFICATION_ID = 0x7D01
        private const val EXTRA_CODE = "code"
        private const val EXTRA_DATA = "data"
        private const val ACTION_STOP = "com.truedesk.host.STOP"

        /** Longest side of the captured frame, to cap bandwidth and encoding cost. */
        private const val MAX_SIDE = 1280

        fun start(context: Context, resultCode: Int, data: Intent) {
            val intent = Intent(context, CaptureService::class.java)
            intent.putExtra(EXTRA_CODE, resultCode)
            intent.putExtra(EXTRA_DATA, data)
            context.startForegroundService(intent)
        }

        fun stop(context: Context) {
            val intent = Intent(context, CaptureService::class.java)
            intent.action = ACTION_STOP
            context.startService(intent)
        }

        /** Scales the screen down so its longest side is at most [MAX_SIDE], keeping
         *  both sides even (H.264 wants even dimensions). */
        fun captureSize(width: Int, height: Int): Pair<Int, Int> {
            val longest = maxOf(width, height)
            val scale = if (longest > MAX_SIDE) MAX_SIDE.toDouble() / longest else 1.0
            fun even(value: Int) = (value and 1.inv()).coerceAtLeast(2)
            return Pair(even((width * scale).toInt()), even((height * scale).toInt()))
        }
    }
}
