package net.sonduit.app

import android.app.LocaleManager
import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.os.LocaleList
import java.util.Locale

/**
 * The app's language, on both sides of Android 13.
 *
 * Android 13 added a per-app language the system owns: it survives reinstalls,
 * it appears in the system settings screen `locales_config.xml` puts this app
 * in, and setting it recreates the activity. Below 13 none of that exists, so
 * the choice is stored by [AppSettings] and applied by wrapping the context
 * every component is built on.
 *
 * The platform store is the only store from 13 onward, deliberately. Keeping a
 * copy in preferences as well would mean a user who changes the language from
 * system settings sees the old answer in this app's own screen.
 */
object AppLocale {

    /**
     * The languages this app ships, as they appear in `locales_config.xml`.
     *
     * Duplicated from the resource because there is no way to read it back
     * below Android 13, which is exactly the range that needs the list most.
     */
    val SUPPORTED = listOf("en", "vi", "de", "es", "fr", "it", "pt", "ru", "ja", "ko", "zh")

    /** The tag meaning "whatever the phone is set to". */
    const val SYSTEM = ""

    /**
     * Apply a language tag chosen by the user.
     *
     * On Android 13 and up the system persists this and recreates the
     * activity; below that the caller's activity has to be recreated by hand,
     * because the locale is only read when a context is built.
     */
    fun apply(context: Context, tag: String) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val manager = context.getSystemService(LocaleManager::class.java)
            manager.applicationLocales = if (tag == SYSTEM) {
                LocaleList.getEmptyLocaleList()
            } else {
                LocaleList.forLanguageTags(tag)
            }
            return
        }

        AppSettings.setLanguageTag(context, tag)
    }

    /** The language in force, as one of [SUPPORTED] or [SYSTEM]. */
    fun current(context: Context): String {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            val locales = context.getSystemService(LocaleManager::class.java).applicationLocales
            val chosen = if (locales.isEmpty) null else locales[0]
            return chosen?.language?.takeIf { it in SUPPORTED } ?: SYSTEM
        }

        return AppSettings.languageTag(context).takeIf { it in SUPPORTED } ?: SYSTEM
    }

    /**
     * A context that resolves resources in the chosen language.
     *
     * Called from `attachBaseContext` in every component that loads a string,
     * which is the activity and the service: the service writes the text of
     * the ongoing notification, and a notification in a different language
     * from the screen behind it looks like a bug.
     *
     * A no-op from Android 13 up, where the platform has already applied the
     * per-app language before any of this runs.
     */
    fun wrap(context: Context): Context {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) return context

        val tag = AppSettings.languageTag(context)
        if (tag == SYSTEM) return context

        val locale = Locale.forLanguageTag(tag)
        val configuration = Configuration(context.resources.configuration)
        configuration.setLocale(locale)
        configuration.setLayoutDirection(locale)
        return context.createConfigurationContext(configuration)
    }

    /**
     * A language's name in its own language.
     *
     * Endonyms rather than names in the current language: someone who has the
     * app in a language they cannot read is looking for the row that is
     * legible to them, and "Deutsch" is that row whatever the app is set to.
     */
    fun endonym(tag: String): String {
        val locale = Locale.forLanguageTag(tag)
        return locale.getDisplayLanguage(locale).replaceFirstChar { it.titlecase(locale) }
    }
}
