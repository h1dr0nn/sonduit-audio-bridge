package net.sonduit.app.ui

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import android.util.Log
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ExperimentalGetImage
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.ImageProxy
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.mlkit.vision.barcode.BarcodeScanner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import net.sonduit.app.R
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

private const val TAG = "SonduitScan"

/**
 * The camera screen that reads the pairing code off the computer.
 *
 * Pairing used to mean reading six digits off this phone and typing the
 * phone's address on the computer. Here the computer puts its own addresses,
 * the port and the code into one square, and the camera reads all of it at
 * once. Nothing is typed on either end.
 *
 * [onScanned] is called at most once. A second read of the same square while
 * the caller is still deciding what to do with the first would start pairing
 * twice.
 */
@Composable
fun ScanScreen(onScanned: (String) -> Unit, onCancel: () -> Unit) {
    val context = LocalContext.current
    val colors = LocalSonduitColors.current

    var granted by remember { mutableStateOf(hasCameraPermission(context)) }
    // Only meaningful after a request has actually been answered. Android
    // cannot tell "never asked" from "denied forever" on its own, and treating
    // the first as the second would send a user to Settings before they had
    // been offered the prompt.
    var refused by remember { mutableStateOf(false) }
    var cameraError by remember { mutableStateOf(false) }

    val request = rememberLauncherForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { allowed ->
        granted = allowed
        refused = !allowed
    }

    LaunchedEffect(Unit) {
        if (!granted) request.launch(Manifest.permission.CAMERA)
    }

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            // Edge to edge, as everywhere else in this app: without these the
            // title sits under the clock and the buttons under the gesture bar.
            .statusBarsPadding()
            .navigationBarsPadding()
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(
            text = stringResource(R.string.scan_title),
            style = MaterialTheme.typography.headlineSmall,
            color = MaterialTheme.colorScheme.onBackground,
        )

        when {
            cameraError -> Message(stringResource(R.string.scan_camera_error))

            granted -> {
                CameraPreview(
                    onScanned = onScanned,
                    onCameraError = { cameraError = true },
                    modifier = Modifier
                        .fillMaxWidth()
                        .aspectRatio(3f / 4f)
                        .clip(RoundedCornerShape(Radius.card))
                        .border(
                            width = 1.dp,
                            color = MaterialTheme.colorScheme.outline,
                            shape = RoundedCornerShape(Radius.card),
                        ),
                )
                Text(
                    text = stringResource(R.string.scan_hint),
                    style = MaterialTheme.typography.bodyMedium,
                    color = colors.faint,
                    textAlign = TextAlign.Center,
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            // Denied, and the system will no longer show the dialog. Asking
            // again does nothing at all and looks like a broken button, so the
            // only honest offer left is the Settings page.
            refused && !canAskAgain(context) -> {
                Message(stringResource(R.string.scan_permission_denied))
                Button(
                    onClick = { openAppSettings(context) },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(56.dp),
                    shape = RoundedCornerShape(Radius.inner),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.primary,
                        contentColor = MaterialTheme.colorScheme.onPrimary,
                    ),
                ) {
                    Text(
                        text = stringResource(R.string.scan_open_settings),
                        style = MaterialTheme.typography.titleMedium,
                    )
                }
            }

            else -> {
                Message(stringResource(R.string.scan_permission_rationale))
                Button(
                    onClick = { request.launch(Manifest.permission.CAMERA) },
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(56.dp),
                    shape = RoundedCornerShape(Radius.inner),
                    colors = ButtonDefaults.buttonColors(
                        containerColor = MaterialTheme.colorScheme.primary,
                        contentColor = MaterialTheme.colorScheme.onPrimary,
                    ),
                ) {
                    Text(
                        text = stringResource(R.string.scan_permission_grant),
                        style = MaterialTheme.typography.titleMedium,
                    )
                }
            }
        }

        TextButton(onClick = onCancel, modifier = Modifier.fillMaxWidth()) {
            Text(
                text = stringResource(R.string.scan_cancel),
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

/** A short paragraph in the soft card the rest of the app uses. */
@Composable
private fun Message(text: String) {
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(Radius.card))
            .background(MaterialTheme.colorScheme.surface)
            .border(
                width = 1.dp,
                color = MaterialTheme.colorScheme.outline,
                shape = RoundedCornerShape(Radius.card),
            )
            .padding(18.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = text,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurface,
            textAlign = TextAlign.Center,
        )
    }
}

/**
 * The live camera, with a barcode analyser bound to it.
 *
 * The provider is unbound explicitly when this leaves the composition. It is
 * bound to the activity's lifecycle, not to the composable, so navigating away
 * would otherwise leave the camera running behind the bridge screen.
 */
@Composable
private fun CameraPreview(
    onScanned: (String) -> Unit,
    onCameraError: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val lifecycleOwner = LocalLifecycleOwner.current
    val executor: ExecutorService = remember { Executors.newSingleThreadExecutor() }
    val scanner: BarcodeScanner = remember {
        BarcodeScanning.getClient(
            BarcodeScannerOptions.Builder()
                .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                .build(),
        )
    }
    val bound = remember { arrayOfNulls<ProcessCameraProvider>(1) }

    DisposableEffect(Unit) {
        onDispose {
            bound[0]?.unbindAll()
            scanner.close()
            executor.shutdown()
        }
    }

    AndroidView(
        modifier = modifier,
        factory = { context ->
            val view = PreviewView(context).apply {
                scaleType = PreviewView.ScaleType.FILL_CENTER
            }

            val future = ProcessCameraProvider.getInstance(context)
            future.addListener({
                try {
                    val provider = future.get()
                    bound[0] = provider

                    val preview = Preview.Builder().build().also {
                        it.setSurfaceProvider(view.surfaceProvider)
                    }
                    // KEEP_ONLY_LATEST, because a queue of stale frames would
                    // have the analyser working on what the camera saw a
                    // second ago while the user waits for it to catch up.
                    val analysis = ImageAnalysis.Builder()
                        .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                        .build()
                        .also { it.setAnalyzer(executor, QrAnalyzer(scanner, onScanned)) }

                    provider.unbindAll()
                    provider.bindToLifecycle(
                        lifecycleOwner,
                        CameraSelector.DEFAULT_BACK_CAMERA,
                        preview,
                        analysis,
                    )
                } catch (error: Exception) {
                    // A camera another app is holding, or a device with no
                    // back camera at all. Neither is recoverable here, and the
                    // typed-code path still works.
                    Log.e(TAG, "the camera could not be bound", error)
                    onCameraError()
                }
            }, ContextCompat.getMainExecutor(context))

            view
        },
    )
}

/**
 * Turns camera frames into the text of the first QR code in view.
 *
 * ML Kit rather than ZXing: the reason is in `app/build.gradle.kts` beside the
 * dependency, and comes down to the bundled model working with no network at
 * all, which is the case this whole feature exists for.
 */
private class QrAnalyzer(
    private val scanner: BarcodeScanner,
    private val onScanned: (String) -> Unit,
) : ImageAnalysis.Analyzer {

    /**
     * Set on the first successful read.
     *
     * Frames keep arriving while the caller tears the screen down, and every
     * one of them holds the same code. Without this, pairing would be started
     * several times over.
     */
    @Volatile
    private var found = false

    // ImageProxy.getImage is behind an androidx opt-in rather than an unstable
    // API: ML Kit needs the underlying Image and there is no supported way to
    // reach it without saying so here.
    @androidx.annotation.OptIn(ExperimentalGetImage::class)
    override fun analyze(proxy: ImageProxy) {
        val frame = proxy.image
        if (frame == null || found) {
            proxy.close()
            return
        }

        val image = InputImage.fromMediaImage(frame, proxy.imageInfo.rotationDegrees)
        scanner.process(image)
            .addOnSuccessListener { barcodes ->
                val text = barcodes.firstNotNullOfOrNull { it.rawValue }
                if (text != null && !found) {
                    found = true
                    onScanned(text)
                }
            }
            .addOnFailureListener { error ->
                // One unreadable frame is normal: the code is out of focus or
                // half out of frame. The next one usually works.
                Log.d(TAG, "frame not decoded", error)
            }
            // The proxy must be closed whatever happened, or the analyser
            // stops receiving frames after a handful and the preview freezes.
            .addOnCompleteListener { proxy.close() }
    }
}

private fun hasCameraPermission(context: Context): Boolean =
    ContextCompat.checkSelfPermission(context, Manifest.permission.CAMERA) ==
        PackageManager.PERMISSION_GRANTED

/**
 * Whether the system would still show the permission dialog.
 *
 * False after a permanent denial, which is the only way to tell that case
 * apart from an ordinary one.
 */
private fun canAskAgain(context: Context): Boolean {
    val activity = context.findActivity() ?: return false
    return ActivityCompat.shouldShowRequestPermissionRationale(
        activity,
        Manifest.permission.CAMERA,
    )
}

/**
 * The activity behind a composition's context.
 *
 * Compose hands out whatever context the composition was created with, which
 * on some hosts is a wrapper rather than the activity itself, and the
 * permission API takes nothing else.
 */
private fun Context.findActivity(): Activity? {
    var current: Context = this
    while (current is ContextWrapper) {
        if (current is Activity) return current
        current = current.baseContext
    }
    return null
}

private fun openAppSettings(context: Context) {
    val intent = Intent(
        Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
        Uri.fromParts("package", context.packageName, null),
    ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    context.startActivity(intent)
}
