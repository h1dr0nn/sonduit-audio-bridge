package net.sonduit.app

import android.Manifest
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import net.sonduit.app.audio.BridgeController
import net.sonduit.app.ui.BridgeScreen
import net.sonduit.app.ui.ScanScreen
import net.sonduit.app.ui.SonduitTheme

/**
 * The only activity.
 *
 * It does not own the session: [BridgeService] does, so audio survives the
 * screen going off. This polls the controller for a snapshot while it is
 * visible and stops when it is not, because a UI nobody is looking at should
 * not be waking the CPU four times a second.
 *
 * It does not own discovery either. That belongs to the native handle and
 * lasts as long as the process; all this does is ask for it in [onCreate],
 * because the app being open is the condition for being pairable.
 */
class MainActivity : ComponentActivity() {

    private val requestNotifications =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { _ ->
            // The answer does not change what happens next. Denied is
            // survivable: Android 13 and up will not show the service
            // notification, but the service still runs and still plays audio.
            // Refusing to start would punish the user for a choice about
            // notifications, not about audio.
            pendingStart?.invoke()
            pendingStart = null
        }

    private var pendingStart: (() -> Unit)? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        // Before anything is drawn, and before the user has pressed anything.
        // This is what makes the phone answer the computer's discovery probe,
        // which the typed-code pairing flow needs and which has nothing to do
        // with whether a session is running.
        BridgeController.prepare()

        setContent {
            SonduitTheme {
                var telemetry by remember { mutableStateOf(BridgeController.Snapshot.EMPTY) }
                var running by remember { mutableStateOf(BridgeController.isRunning()) }
                var error by remember { mutableStateOf(BridgeController.lastError) }
                var pairingCode by remember { mutableStateOf(BridgeController.pairingCode()) }
                var scanning by remember { mutableStateOf(false) }
                var pairingStatus by remember { mutableStateOf<String?>(null) }
                val scope = rememberCoroutineScope()

                LaunchedEffect(Unit) {
                    // Four a second, matching the desktop. Fast enough to read
                    // as live, slow enough not to matter to the battery.
                    while (true) {
                        telemetry = BridgeController.telemetry()
                        running = BridgeController.isRunning()
                        // A device that could not be opened matters more than
                        // a start that failed a minute ago.
                        error = telemetry.playbackError ?: BridgeController.lastError
                        delay(250)
                    }
                }

                if (scanning) {
                    ScanScreen(
                        onScanned = { payload ->
                            scanning = false
                            scope.launch {
                                pairingStatus = getString(pair(payload))
                                // The computer chose the code, so this device
                                // is now showing a different one than it was.
                                pairingCode = BridgeController.pairingCode()
                                running = BridgeController.isRunning()
                            }
                        },
                        onCancel = { scanning = false },
                    )
                } else {
                    BridgeScreen(
                        telemetry = telemetry,
                        running = running,
                        error = error,
                        pairingCode = pairingCode,
                        pairingStatus = pairingStatus,
                        onScan = {
                            pairingStatus = null
                            // Started before the camera opens rather than after
                            // the scan: the announcement has to advertise the
                            // port audio will arrive on, and the notification
                            // prompt is better asked now than on top of a live
                            // camera preview.
                            if (!BridgeController.isRunning()) startBridge()
                            scanning = true
                        },
                        onStart = { startBridge() },
                        onStop = {
                            BridgeService.stop(this@MainActivity)
                            running = false
                        },
                        onRegenerateCode = {
                            BridgeController.regeneratePairingCode()
                            pairingCode = BridgeController.pairingCode()
                        },
                    )
                }
            }
        }
    }

    private fun startBridge() {
        val start = { BridgeService.start(this, DEFAULT_PORT) }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            // Asked before starting rather than after: a foreground service
            // started without the permission shows no notification at all,
            // which looks like the app doing nothing.
            pendingStart = start
            requestNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
            return
        }

        start()
    }

    /**
     * Pair from a scanned code, returning the string resource to show for it.
     *
     * The resource id rather than the text: only the UI layer should be
     * reading resources, and only this layer knows which outcome happened.
     */
    private suspend fun pair(payload: String): Int {
        if (!BridgeController.isRunning()) {
            // The session is normally already up, because tapping Scan starts
            // it. This covers the case where the service was still coming up
            // while the user was pointing the camera.
            startBridge()
            if (!awaitRunning()) return R.string.scan_failed
        }

        return when (BridgeController.acceptInvite(payload)) {
            BridgeController.PairResult.SENT -> R.string.scan_paired
            BridgeController.PairResult.NOT_A_CODE -> R.string.scan_not_a_code
            BridgeController.PairResult.UNREACHABLE -> R.string.scan_failed
            BridgeController.PairResult.NOT_RUNNING -> R.string.scan_failed
        }
    }

    /** Wait for the service to have a socket bound, or give up. */
    private suspend fun awaitRunning(): Boolean {
        repeat(START_POLLS) {
            if (BridgeController.isRunning()) return true
            delay(START_POLL_MS)
        }
        return BridgeController.isRunning()
    }

    private companion object {
        /** Zero means the Rust side picks its own default. */
        const val DEFAULT_PORT = 0

        /**
         * How long to wait for the foreground service to bind its socket.
         *
         * The system delivers the start intent on its own schedule, so there
         * is a gap where there is no port to announce. Five seconds is far
         * longer than that gap and still short of the pairing window the
         * computer holds open.
         */
        const val START_POLLS = 50
        const val START_POLL_MS = 100L
    }
}
