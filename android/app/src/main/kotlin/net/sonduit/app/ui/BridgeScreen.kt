package net.sonduit.app.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import net.sonduit.app.R
import net.sonduit.app.audio.BridgeController

/**
 * The whole app.
 *
 * One screen on purpose. The phone is the receiving end: it has one decision
 * to make, which is whether to listen, and one thing to show, which is whether
 * audio is arriving and how well. Anything more would be settings nobody
 * changes, on the device with the smaller screen.
 */
@Composable
fun BridgeScreen(
    telemetry: BridgeController.Snapshot,
    running: Boolean,
    error: String?,
    onStart: () -> Unit,
    onStop: () -> Unit,
) {
    val colors = LocalSonduitColors.current

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .verticalScroll(rememberScrollState())
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text(
            text = stringResource(R.string.app_name),
            style = MaterialTheme.typography.headlineLarge,
            color = MaterialTheme.colorScheme.onBackground,
        )

        StatusCard(telemetry = telemetry, running = running)

        if (error != null) {
            Card {
                Text(
                    text = error,
                    style = MaterialTheme.typography.bodyMedium,
                    color = colors.danger,
                )
            }
        }

        Button(
            onClick = if (running) onStop else onStart,
            modifier = Modifier
                .fillMaxWidth()
                .height(56.dp),
            shape = RoundedCornerShape(Radius.inner),
            colors = ButtonDefaults.buttonColors(
                containerColor = if (running) {
                    MaterialTheme.colorScheme.surfaceVariant
                } else {
                    MaterialTheme.colorScheme.primary
                },
                contentColor = if (running) {
                    MaterialTheme.colorScheme.onSurface
                } else {
                    MaterialTheme.colorScheme.onPrimary
                },
            ),
        ) {
            Text(
                text = stringResource(if (running) R.string.action_stop else R.string.action_start),
                style = MaterialTheme.typography.titleMedium,
            )
        }

        if (running) {
            TelemetryGrid(telemetry)
        }

        Text(
            text = stringResource(R.string.hint_same_network),
            style = MaterialTheme.typography.bodyMedium,
            color = colors.faint,
            textAlign = TextAlign.Center,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

/**
 * The one thing the user looks at.
 *
 * The dot is the state and the line under it is the reason. A pill that only
 * said "connected" would be useless on the failure this app actually has,
 * which is a session that is listening and receiving nothing.
 */
@Composable
private fun StatusCard(telemetry: BridgeController.Snapshot, running: Boolean) {
    val colors = LocalSonduitColors.current

    val (tint, label) = when {
        telemetry.streaming -> colors.ok to stringResource(R.string.status_streaming_short)
        running -> colors.warn to stringResource(R.string.status_waiting)
        else -> colors.faint to stringResource(R.string.status_idle)
    }
    val dot by animateColorAsState(tint, label = "status")

    Card {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(
                modifier = Modifier
                    .size(10.dp)
                    .clip(CircleShape)
                    .background(dot),
            )
            Spacer(Modifier.size(10.dp))
            Text(
                text = label,
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurface,
            )
        }

        if (telemetry.streaming) {
            Spacer(Modifier.height(8.dp))
            Text(
                text = stringResource(
                    R.string.format_summary,
                    telemetry.sampleRate.toInt() / 1000,
                    telemetry.channels.toInt(),
                ),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun TelemetryGrid(telemetry: BridgeController.Snapshot) {
    // Loss matters more than any absolute count: a hundred lost packets out of
    // a million is nothing and out of a thousand is a broken link.
    val total = telemetry.packetsAccepted + telemetry.packetsLost
    val lossPercent = if (total == 0uL) 0.0 else telemetry.packetsLost.toDouble() * 100.0 / total.toDouble()

    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Stat(
                label = stringResource(R.string.stat_buffer),
                value = "%.0f".format(telemetry.bufferDepthMs),
                unit = "ms",
                modifier = Modifier.weight(1f),
            )
            Stat(
                label = stringResource(R.string.stat_jitter),
                value = "%.1f".format(telemetry.jitterMs),
                unit = "ms",
                modifier = Modifier.weight(1f),
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Stat(
                label = stringResource(R.string.stat_loss),
                value = "%.2f".format(lossPercent),
                unit = "%",
                modifier = Modifier.weight(1f),
            )
            Stat(
                label = stringResource(R.string.stat_packets),
                value = telemetry.packetsAccepted.toString(),
                unit = "",
                modifier = Modifier.weight(1f),
            )
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Stat(
                label = stringResource(R.string.stat_drift),
                // Absent until the estimator has about 25 seconds of history.
                // A dash says "not measured yet"; a zero would claim the
                // clocks match.
                value = telemetry.driftPpm?.let { "%+.1f".format(it) }
                    ?: stringResource(R.string.value_unknown),
                unit = "ppm",
                modifier = Modifier.weight(1f),
            )
            Stat(
                label = stringResource(R.string.stat_correction),
                value = "%+.1f".format(telemetry.correctionPpm),
                unit = "ppm",
                modifier = Modifier.weight(1f),
            )
        }
        if (telemetry.packetsMalformed > 0uL) {
            // Only shown when it is non-zero. A permanent row of zeroes teaches
            // the user to stop reading the panel.
            Stat(
                label = stringResource(R.string.stat_malformed),
                value = telemetry.packetsMalformed.toString(),
                unit = "",
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

@Composable
private fun Stat(label: String, value: String, unit: String, modifier: Modifier = Modifier) {
    Column(
        modifier = modifier
            .clip(RoundedCornerShape(Radius.inner))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .border(
                width = 1.dp,
                color = MaterialTheme.colorScheme.outline,
                shape = RoundedCornerShape(Radius.inner),
            )
            .padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        Text(
            text = label.uppercase(),
            style = MaterialTheme.typography.labelSmall,
            color = LocalSonduitColors.current.faint,
        )
        Spacer(Modifier.height(4.dp))
        Row(verticalAlignment = Alignment.Bottom) {
            Text(
                text = value,
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurface,
            )
            if (unit.isNotEmpty()) {
                Spacer(Modifier.size(3.dp))
                Text(
                    text = unit,
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

/** The soft card the desktop shell uses, in Compose. */
@Composable
private fun Card(content: @Composable ColumnScope.() -> Unit) {
    Column(
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
        content = content,
    )
}
