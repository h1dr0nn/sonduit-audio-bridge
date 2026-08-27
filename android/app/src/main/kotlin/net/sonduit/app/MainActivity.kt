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
import androidx.compose.runtime.setValue
import androidx.core.content.ContextCompat
import kotlinx.coroutines.delay
import net.sonduit.app.audio.BridgeController
import net.sonduit.app.ui.BridgeScreen
import net.sonduit.app.ui.SonduitTheme

/**
 * The only activity.
 *
 * It does not own the session: [BridgeService] does, so audio survives the
 * screen going off. This polls the controller for a snapshot while it is
 * visible and stops when it is not, because a UI nobody is looking at should
 * not be waking the CPU four times a second.
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

        setContent {
            SonduitTheme {
                var telemetry by remember { mutableStateOf(BridgeController.Snapshot.EMPTY) }
                var running by remember { mutableStateOf(BridgeController.isRunning()) }
                var error by remember { mutableStateOf(BridgeController.lastError) }

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

                BridgeScreen(
                    telemetry = telemetry,
                    running = running,
                    error = error,
                    onStart = { startBridge() },
                    onStop = {
                        BridgeService.stop(this@MainActivity)
                        running = false
                    },
                )
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

    private companion object {
        /** Zero means the Rust side picks its own default. */
        const val DEFAULT_PORT = 0
    }
}
