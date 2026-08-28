package net.sonduit.app

import android.content.Context
import android.content.SharedPreferences

/**
 * The handful of choices the user is allowed to make, and where they are kept.
 *
 * SharedPreferences rather than DataStore: there are four scalars here, every
 * one of them is needed before the first frame is drawn, and two of the
 * readers are [MainActivity.attachBaseContext] and [BridgeService], neither of
 * which has a coroutine scope to wait on an asynchronous read. DataStore would
 * buy consistency guarantees this has no writer contention to need, and cost a
 * dependency plus a suspending read on the startup path.
 *
 * No cached copy is held. `getSharedPreferences` returns the same instance for
 * the same name, so every call after the first is a map lookup, and a cache
 * here would only add a second thing that can be stale.
 */
object AppSettings {

    private const val FILE = "sonduit.settings"

    private const val KEY_DEVICE_NAME = "device_name"
    private const val KEY_AUTO_START = "auto_start"
    private const val KEY_THEME = "theme"
    private const val KEY_LANGUAGE = "language"

    /** Which colour scheme to draw, regardless of what the system is doing. */
    enum class Theme {
        SYSTEM,
        LIGHT,
        DARK,
        ;

        companion object {
            fun parse(stored: String?): Theme =
                entries.firstOrNull { it.name.equals(stored, ignoreCase = true) } ?: SYSTEM
        }
    }

    private fun prefs(context: Context): SharedPreferences =
        // The application context where there is one, so every caller shares
        // the same instance. There is not always one: this is read from
        // `attachBaseContext`, before the component is attached, and a null
        // there would crash the process on launch rather than anywhere useful.
        (context.applicationContext ?: context).getSharedPreferences(FILE, Context.MODE_PRIVATE)

    /**
     * The name the user chose to announce, or empty for the phone's own model.
     *
     * Empty is stored rather than the resolved model name, so a phone that is
     * left on the default keeps following the model instead of freezing
     * whatever the model was on the day the app first ran.
     */
    fun deviceName(context: Context): String =
        prefs(context).getString(KEY_DEVICE_NAME, "").orEmpty()

    fun setDeviceName(context: Context, name: String) {
        prefs(context).edit().putString(KEY_DEVICE_NAME, name.trim()).apply()
    }

    /** Whether opening the app should start a session without being asked. */
    fun autoStart(context: Context): Boolean =
        prefs(context).getBoolean(KEY_AUTO_START, false)

    fun setAutoStart(context: Context, enabled: Boolean) {
        prefs(context).edit().putBoolean(KEY_AUTO_START, enabled).apply()
    }

    fun theme(context: Context): Theme =
        Theme.parse(prefs(context).getString(KEY_THEME, null))

    fun setTheme(context: Context, theme: Theme) {
        prefs(context).edit().putString(KEY_THEME, theme.name).apply()
    }

    /**
     * The chosen language tag, or empty to follow the system.
     *
     * Only consulted below Android 13. From 13 onward the platform stores the
     * per-app language itself and shows it in system settings, so a copy here
     * would be a second answer able to disagree with the one the system acts
     * on. See [AppLocale].
     */
    fun languageTag(context: Context): String =
        prefs(context).getString(KEY_LANGUAGE, "").orEmpty()

    fun setLanguageTag(context: Context, tag: String) {
        prefs(context).edit().putString(KEY_LANGUAGE, tag).apply()
    }
}
