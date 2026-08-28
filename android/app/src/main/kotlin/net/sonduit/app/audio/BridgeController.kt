package net.sonduit.app.audio

import android.content.Context
import android.os.Build
import android.util.Log
import uniffi.sonduit_ffi.Bridge
import uniffi.sonduit_ffi.BridgeState
import uniffi.sonduit_ffi.FfiException

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

    /**
     * The native handle.
     *
     * Constructing it is not free of consequence: the constructor binds the
     * discovery port and starts the thread that answers the computer's probes.
     * That is why [prepare] exists and why the activity calls it as it comes
     * up, rather than leaving the first touch to whatever happened to ask for
     * telemetry first.
     */
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
        val driftPpm: Double?,
        val correctionPpm: Double,
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
                driftPpm = null,
                correctionPpm = 0.0,
            )
        }
    }

    /**
     * Make this device findable, and tell it the name to answer with.
     *
     * Discovery is not part of playing audio. The computer finds this phone by
     * broadcasting a probe, and the phone has to answer it while the user is
     * still reading the six digits off the screen with nothing started: a
     * responder that only ran during a session meant the typed-code flow never
     * found anything. So this is wired to the activity's onCreate, not to a
     * button, and the user does nothing to make it happen.
     *
     * Idempotent. There is one native handle per process and one responder
     * behind it, so calling this twice renames the same one.
     */
    @Synchronized
    fun prepare() {
        try {
            bridge.setDeviceName(deviceName())
        } catch (error: Exception) {
            Log.e(TAG, "the bridge could not be prepared", error)
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

    /**
     * The code the desktop has to be given before it will accept this device.
     *
     * Read from Rust rather than generated here: the announce thread needs the
     * same value, and two copies of a secret are one copy too many.
     */
    fun pairingCode(): String = try {
        bridge.pairingCode()
    } catch (error: Exception) {
        Log.e(TAG, "pairing code unavailable", error)
        ""
    }

    /**
     * What came of trying to pair from a scanned code.
     *
     * An enum rather than an exception: every one of these is a normal outcome
     * the user has to be told about in their own words, and only the caller
     * knows which string resource says it.
     */
    enum class PairResult {
        /** The announcement left this device. */
        SENT,

        /** The camera read something that was not a Sonduit pairing code. */
        NOT_A_CODE,

        /** No address in the code could be reached from here. */
        UNREACHABLE,

        /** There is no session listening, so there is no port to announce. */
        NOT_RUNNING,
    }

    /**
     * Pair from a code scanned off the computer's screen.
     *
     * The session must already be running: the announcement advertises the
     * port audio will arrive on, and there is no such port until the socket is
     * bound.
     *
     * [PairResult.SENT] means the datagram left this device, not that the
     * computer accepted it. Nothing here can know that; what tells the user is
     * audio starting to play.
     */
    @Synchronized
    fun acceptInvite(payload: String): PairResult {
        if (!running) return PairResult.NOT_RUNNING

        return try {
            bridge.acceptInvite(payload)
            lastError = null
            PairResult.SENT
        } catch (error: FfiException.BadInvite) {
            PairResult.NOT_A_CODE
        } catch (error: FfiException.NotRunning) {
            PairResult.NOT_RUNNING
        } catch (error: Exception) {
            // Usually every address in the code is on a network this phone is
            // not on, which is what happens when the user scans a code shown
            // by a computer they cannot actually route to.
            Log.e(TAG, "pairing from a scanned code failed", error)
            PairResult.UNREACHABLE
        }
    }

    /** Replace the pairing code. Any desktop paired with the old one stops working. */
    fun regeneratePairingCode() {
        try {
            bridge.regeneratePairingCode()
        } catch (error: Exception) {
            Log.e(TAG, "could not regenerate the pairing code", error)
        }
    }

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
                driftPpm = native.driftPpm,
                correctionPpm = native.correctionPpm,
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
