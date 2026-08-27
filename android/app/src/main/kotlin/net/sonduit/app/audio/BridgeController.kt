package net.sonduit.app.audio

import android.content.Context
import android.os.Build
import android.util.Log
import uniffi.sonduit_ffi.Bridge
import uniffi.sonduit_ffi.BridgeState

/**
 * The one place Kotlin talks to Rust.
 *
 * A single object rather than an injected dependency: there is exactly one
 * audio device and exactly one socket, and the service and the activity both
 * need to reach the same session. Two Bridge instances would fight over the
 * port and the second would fail to bind, which is a confusing way to discover
 * a design mistake.
 *
 * Nothing here does audio work. Rust owns the receive thread, the jitter buffer
 * and the AAudio callback; this object starts them, stops them, and copies out
 * a snapshot for the UI.
 */
object BridgeController {

    private const val TAG = "SonduitBridge"

    private val bridge: Bridge by lazy { Bridge() }

    /** Set once the native session is live, so stop is not called on nothing. */
    @Volatile
    private var running = false

    /** Last failure, shown in the UI until the next successful start. */
    @Volatile
    var lastError: String? = null
        private set

    /**
     * A flat copy of what Rust reports.
     *
     * Copied rather than referenced: the UI reads this from the main thread on
     * every recomposition, and handing out a live view of native state would
     * mean holding a lock there.
     */
    data class Snapshot(
        val streaming: Boolean,
        val listening: Boolean,
        val packetsAccepted: ULong,
        val packetsLost: ULong,
        val packetsLate: ULong,
        val packetsMalformed: ULong,
        val bufferDepthMs: Double,
        val bufferTargetMs: Double,
        val jitterMs: Double,
        val concealedFrames: ULong,
        val sampleRate: UInt,
        val channels: UByte,
        val playbackError: String?,
    ) {
        companion object {
            val EMPTY = Snapshot(
                streaming = false,
                listening = false,
                packetsAccepted = 0uL,
                packetsLost = 0uL,
                packetsLate = 0uL,
                packetsMalformed = 0uL,
                bufferDepthMs = 0.0,
                bufferTargetMs = 0.0,
                jitterMs = 0.0,
                concealedFrames = 0uL,
                sampleRate = 0u,
                channels = 0u,
                playbackError = null,
            )
        }
    }

    /**
     * Start a session.
     *
     * Returns false rather than throwing: the caller is a service that has
     * already put a notification on screen, and it needs to decide what to do,
     * not to catch.
     */
    @Synchronized
    fun start(context: Context, port: Int): Boolean {
        if (running) return true

        bridge.setDeviceName(deviceName())
        return try {
            bridge.start(port.toUShort())
            running = true
            lastError = null
            true
        } catch (error: Exception) {
            // The usual cause is the port already being held by another copy
            // of the app, which the user can do nothing about except stop it.
            Log.e(TAG, "start failed", error)
            lastError = error.message ?: context.toString()
            false
        }
    }

    @Synchronized
    fun stop() {
        if (!running) return
        try {
            bridge.stop()
        } catch (error: Exception) {
            Log.e(TAG, "stop failed", error)
        } finally {
            running = false
        }
    }

    fun isRunning(): Boolean = running

    /** A snapshot for the UI. Safe to call from any thread. */
    fun telemetry(): Snapshot {
        if (!running) return Snapshot.EMPTY
        return try {
            val native = bridge.telemetry()
            Snapshot(
                streaming = native.state == BridgeState.STREAMING,
                listening = native.state == BridgeState.DISCOVERING,
                packetsAccepted = native.packetsAccepted,
                packetsLost = native.packetsLost,
                packetsLate = native.packetsLate,
                packetsMalformed = native.packetsMalformed,
                bufferDepthMs = native.bufferDepthMs,
                bufferTargetMs = native.bufferTargetMs,
                jitterMs = native.jitterMs,
                concealedFrames = native.concealedFrames,
                sampleRate = native.sampleRate,
                channels = native.channels,
                playbackError = native.playbackError,
            )
        } catch (error: Exception) {
            Log.e(TAG, "telemetry failed", error)
            Snapshot.EMPTY
        }
    }

    /**
     * The name this device announces to the desktop.
     *
     * `Build.MODEL` rather than the user's device name: reading the latter
     * needs a permission on newer releases, and the model is enough to tell
     * two phones apart in a list.
     */
    private fun deviceName(): String {
        val manufacturer = Build.MANUFACTURER.replaceFirstChar { it.uppercase() }
        val model = Build.MODEL
        return if (model.startsWith(manufacturer, ignoreCase = true)) model else "$manufacturer $model"
    }
}
