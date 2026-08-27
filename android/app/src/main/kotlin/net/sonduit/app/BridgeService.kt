package net.sonduit.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat
import androidx.core.app.ServiceCompat
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import net.sonduit.app.audio.BridgeController

/**
 * Keeps the bridge alive while the screen is off.
 *
 * An activity alone is not enough: the moment it stops being visible the
 * process becomes a background process, and a background process gets its
 * threads throttled and eventually gets killed. `mediaPlayback` is the honest
 * service type, because that is exactly what this does.
 */
class BridgeService : LifecycleService() {

    private var wakeLock: PowerManager.WakeLock? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private var notifier: Job? = null

    override fun onBind(intent: Intent): IBinder? {
        super.onBind(intent)
        return null
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        super.onStartCommand(intent, flags, startId)

        when (intent?.action) {
            ACTION_STOP -> {
                stopBridge()
                return START_NOT_STICKY
            }
        }

        createChannel()
        val port = intent?.getIntExtra(EXTRA_PORT, 0) ?: 0

        ServiceCompat.startForeground(
            this,
            NOTIFICATION_ID,
            buildNotification(getString(R.string.status_waiting)),
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK
            } else {
                0
            },
        )

        acquireLocks()

        val started = BridgeController.start(this, port)
        if (!started) {
            // Starting failed, and a foreground service with nothing behind it
            // is a notification that lies. Stop rather than sit there.
            stopBridge()
            return START_NOT_STICKY
        }

        // The notification carries the state, so it has to follow it. Once a
        // second: this is a glanceable summary, not a meter.
        notifier = lifecycleScope.launch(Dispatchers.Default) {
            while (isActive) {
                val telemetry = BridgeController.telemetry()
                val text = describe(telemetry)
                notificationManager().notify(NOTIFICATION_ID, buildNotification(text))
                delay(1_000)
            }
        }

        // START_STICKY would have Android restart this with a null intent after
        // a kill, silently reopening the audio device without the user asking.
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        stopBridge()
        super.onDestroy()
    }

    private fun stopBridge() {
        notifier?.cancel()
        notifier = null
        BridgeController.stop()
        releaseLocks()
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    /**
     * Multicast and CPU locks.
     *
     * The multicast lock is not optional: Android drops multicast datagrams
     * before they reach the socket unless one is held, so a sender using the
     * default group would appear to be sending into nothing.
     */
    private fun acquireLocks() {
        if (wakeLock == null) {
            val power = getSystemService(Context.POWER_SERVICE) as PowerManager
            wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, WAKE_TAG).apply {
                setReferenceCounted(false)
                acquire(WAKE_TIMEOUT_MS)
            }
        }
        if (multicastLock == null) {
            val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
            multicastLock = wifi.createMulticastLock(MULTICAST_TAG).apply {
                setReferenceCounted(false)
                acquire()
            }
        }
    }

    private fun releaseLocks() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        multicastLock?.let { if (it.isHeld) it.release() }
        multicastLock = null
    }

    private fun describe(telemetry: BridgeController.Snapshot): String =
        when {
            !telemetry.streaming -> getString(R.string.status_waiting)
            else -> getString(
                R.string.status_streaming,
                telemetry.sampleRate.toInt() / 1000,
                telemetry.bufferDepthMs.toInt(),
            )
        }

    private fun notificationManager(): NotificationManager =
        getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager

    private fun createChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            CHANNEL_ID,
            getString(R.string.channel_name),
            // Low, not default: this notification exists because the platform
            // requires one, not because the user wants to be interrupted.
            NotificationManager.IMPORTANCE_LOW,
        ).apply {
            description = getString(R.string.channel_description)
            setShowBadge(false)
        }
        notificationManager().createNotificationChannel(channel)
    }

    private fun buildNotification(text: String): Notification {
        val open = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, BridgeService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE,
        )

        return NotificationCompat.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(R.string.app_name))
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentIntent(open)
            .addAction(0, getString(R.string.action_stop), stop)
            .setOngoing(true)
            .setSilent(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .build()
    }

    companion object {
        private const val CHANNEL_ID = "sonduit.bridge"
        private const val NOTIFICATION_ID = 1
        private const val WAKE_TAG = "sonduit:bridge"
        private const val MULTICAST_TAG = "sonduit:multicast"

        /**
         * Ceiling on the wake lock.
         *
         * A lock with no timeout that leaks survives the app and flattens the
         * battery. Eight hours is longer than any session and still bounded.
         */
        private const val WAKE_TIMEOUT_MS = 8L * 60 * 60 * 1000

        const val ACTION_STOP = "net.sonduit.app.STOP"
        const val EXTRA_PORT = "port"

        fun start(context: Context, port: Int) {
            val intent = Intent(context, BridgeService::class.java)
                .putExtra(EXTRA_PORT, port)
            context.startForegroundService(intent)
        }

        fun stop(context: Context) {
            val intent = Intent(context, BridgeService::class.java).setAction(ACTION_STOP)
            context.startService(intent)
        }
    }
}
