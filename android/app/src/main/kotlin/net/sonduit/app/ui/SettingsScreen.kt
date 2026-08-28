package net.sonduit.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import net.sonduit.app.AppLocale
import net.sonduit.app.AppSettings
import net.sonduit.app.BuildConfig
import net.sonduit.app.R

/**
 * The author, spelled as the project's own copyright line spells it.
 *
 * `LICENSE` reads "Copyright (c) 2025 h1dr0n", and that is the name a credit
 * owes. The desktop's About page says the same. See [REPOSITORY] for why the
 * line below it is spelled differently.
 */
private const val AUTHOR = "h1dr0n"

/**
 * The project on GitHub, as owner and name.
 *
 * The `github.com/` prefix is dropped so the line fits the width of a phone
 * without wrapping. That makes the spelling easier to mistake for a typo, not
 * harder, so it is worth saying again: `h1dr0nn` carries two n's because it is
 * the GitHub account, while [AUTHOR] is the name `LICENSE` uses. The two are
 * deliberately different, and neither is a misspelling of the other.
 */
private const val REPOSITORY = "h1dr0nn/sonduit-audio-bridge"

/**
 * Everything about this app the user is allowed to change.
 *
 * Four rows, and every one of them reaches something real: the name the
 * computer lists this phone under, whether opening the app starts a session,
 * which of the two colour schemes is drawn, and which of the eleven shipped
 * languages the app is read in. A row that only stored a preference nothing
 * consults would be worse than no row at all.
 *
 * Stateless, like [BridgeScreen]: the activity owns the values, because two of
 * them change how the activity itself is built and it has to be the one that
 * acts on them.
 */
@Composable
fun SettingsScreen(
    deviceName: String,
    defaultDeviceName: String,
    autoStart: Boolean,
    theme: AppSettings.Theme,
    language: String,
    onDeviceName: (String) -> Unit,
    onAutoStart: (Boolean) -> Unit,
    onTheme: (AppSettings.Theme) -> Unit,
    onLanguage: (String) -> Unit,
    onBack: () -> Unit,
) {
    val colors = LocalSonduitColors.current

    // The system back gesture is how a screen like this is normally left. The
    // arrow is there for the same reason the scan screen has a Cancel button.
    BackHandler(onBack = onBack)

    Column(
        modifier = Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            // Edge to edge, as everywhere else in this app: without these the
            // title sits under the clock and the credit under the gesture bar.
            .statusBarsPadding()
            .navigationBarsPadding()
            .verticalScroll(rememberScrollState())
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) {
                Icon(
                    imageVector = Icons.AutoMirrored.Filled.ArrowBack,
                    contentDescription = stringResource(R.string.settings_back),
                    tint = MaterialTheme.colorScheme.onBackground,
                )
            }
            Spacer(Modifier.width(4.dp))
            Text(
                text = stringResource(R.string.settings_title),
                style = MaterialTheme.typography.headlineLarge,
                color = MaterialTheme.colorScheme.onBackground,
            )
        }

        DeviceNameCard(
            name = deviceName,
            placeholder = defaultDeviceName,
            onName = onDeviceName,
        )

        AutoStartCard(enabled = autoStart, onEnabled = onAutoStart)

        AppearanceCard(theme = theme, onTheme = onTheme)

        LanguageCard(language = language, onLanguage = onLanguage)

        AboutCard()

        Spacer(Modifier.height(4.dp))
    }
}

/**
 * The name the computer lists this phone under.
 *
 * Applied as it is typed rather than behind a Save button: the value goes
 * straight to the discovery responder, and the user checking whether it worked
 * is looking at the computer's device list, not at this screen.
 */
@Composable
private fun DeviceNameCard(name: String, placeholder: String, onName: (String) -> Unit) {
    SonduitCard {
        SectionTitle(stringResource(R.string.settings_name_title))
        Spacer(Modifier.height(10.dp))
        OutlinedTextField(
            value = name,
            onValueChange = onName,
            modifier = Modifier.fillMaxWidth(),
            singleLine = true,
            shape = RoundedCornerShape(Radius.inner),
            placeholder = {
                Text(
                    text = placeholder,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            },
            textStyle = MaterialTheme.typography.bodyMedium,
        )
        Spacer(Modifier.height(8.dp))
        Hint(stringResource(R.string.settings_name_hint))
    }
}

@Composable
private fun AutoStartCard(enabled: Boolean, onEnabled: (Boolean) -> Unit) {
    SonduitCard {
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Text(
                text = stringResource(R.string.settings_autostart_title),
                style = MaterialTheme.typography.titleMedium,
                color = MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.weight(1f),
            )
            Spacer(Modifier.width(12.dp))
            Switch(checked = enabled, onCheckedChange = onEnabled)
        }
        Spacer(Modifier.height(6.dp))
        Hint(stringResource(R.string.settings_autostart_hint))
    }
}

@Composable
private fun AppearanceCard(theme: AppSettings.Theme, onTheme: (AppSettings.Theme) -> Unit) {
    val labels = mapOf(
        AppSettings.Theme.SYSTEM to stringResource(R.string.settings_theme_system),
        AppSettings.Theme.LIGHT to stringResource(R.string.settings_theme_light),
        AppSettings.Theme.DARK to stringResource(R.string.settings_theme_dark),
    )

    SonduitCard {
        SectionTitle(stringResource(R.string.settings_appearance_title))
        Spacer(Modifier.height(10.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            AppSettings.Theme.entries.forEach { option ->
                Pill(
                    label = labels.getValue(option),
                    selected = option == theme,
                    onClick = { onTheme(option) },
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }
}

/**
 * One row per shipped language, plus the system.
 *
 * A list rather than a dropdown: twelve entries in an alphabet the reader may
 * not use are hard enough to find without hiding eleven of them behind the
 * twelfth.
 */
@Composable
private fun LanguageCard(language: String, onLanguage: (String) -> Unit) {
    SonduitCard {
        SectionTitle(stringResource(R.string.settings_language_title))
        Spacer(Modifier.height(4.dp))
        LanguageRow(
            label = stringResource(R.string.settings_language_system),
            selected = language == AppLocale.SYSTEM,
            onClick = { onLanguage(AppLocale.SYSTEM) },
        )
        AppLocale.SUPPORTED.forEach { tag ->
            LanguageRow(
                label = AppLocale.endonym(tag),
                selected = language == tag,
                onClick = { onLanguage(tag) },
            )
        }
    }
}

@Composable
private fun LanguageRow(label: String, selected: Boolean, onClick: () -> Unit) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(Radius.inner))
            .clickable(onClick = onClick)
            .padding(horizontal = 8.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            text = label,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurface,
        )
        if (selected) {
            Icon(
                imageVector = Icons.Filled.Check,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.primary,
            )
        }
    }
}

/**
 * The version and the credit, at the foot of the screen.
 *
 * The version is [BuildConfig.VERSION_NAME], which Gradle takes from
 * `gradle.properties`, which `tools/version.mjs sync` takes from the root
 * `Cargo.toml`. ADR-008 makes that the only place a version is written, so it
 * must not be typed again here.
 */
@Composable
private fun AboutCard() {
    SonduitCard {
        SectionTitle(stringResource(R.string.settings_about_title))
        Spacer(Modifier.height(10.dp))
        AboutRow(stringResource(R.string.settings_version), BuildConfig.VERSION_NAME)
        Spacer(Modifier.height(8.dp))
        AboutRow(stringResource(R.string.settings_author), AUTHOR)
        Spacer(Modifier.height(8.dp))
        SectionTitle(stringResource(R.string.settings_repository))
        Spacer(Modifier.height(4.dp))
        Text(
            text = REPOSITORY,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun AboutRow(label: String, value: String) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween,
    ) {
        Text(
            text = label,
            style = MaterialTheme.typography.bodyMedium,
            color = LocalSonduitColors.current.faint,
        )
        Text(
            text = value,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurface,
        )
    }
}

/** A selectable pill, in the shape the rest of the app already uses. */
@Composable
private fun Pill(
    label: String,
    selected: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
            .clip(RoundedCornerShape(Radius.pill))
            .background(
                if (selected) {
                    MaterialTheme.colorScheme.primary
                } else {
                    MaterialTheme.colorScheme.surfaceVariant
                },
            )
            .border(
                width = 1.dp,
                color = MaterialTheme.colorScheme.outline,
                shape = RoundedCornerShape(Radius.pill),
            )
            .clickable(onClick = onClick)
            .padding(vertical = 12.dp),
        horizontalArrangement = Arrangement.Center,
    ) {
        Text(
            text = label,
            style = MaterialTheme.typography.bodyMedium,
            color = if (selected) {
                MaterialTheme.colorScheme.onPrimary
            } else {
                MaterialTheme.colorScheme.onSurface
            },
        )
    }
}

@Composable
private fun SectionTitle(text: String) {
    Text(
        text = text.uppercase(),
        style = MaterialTheme.typography.labelSmall,
        color = LocalSonduitColors.current.faint,
    )
}

@Composable
private fun Hint(text: String) {
    Text(
        text = text,
        style = MaterialTheme.typography.bodyMedium,
        color = LocalSonduitColors.current.faint,
    )
}
