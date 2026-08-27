/**
 * The locale bundles and everything that can be answered from them alone.
 *
 * Kept out of `index.js` because that module reaches into React context, and
 * `useSettings` needs the language list before any context exists. Importing
 * the whole of `index.js` from there would close an import cycle.
 */

import de from './locales/de.json';
import en from './locales/en.json';
import es from './locales/es.json';
import fr from './locales/fr.json';
import it from './locales/it.json';
import ja from './locales/ja.json';
import ko from './locales/ko.json';
import pt from './locales/pt.json';
import ru from './locales/ru.json';
import vi from './locales/vi.json';
import zh from './locales/zh.json';

const bundles = { de, en, es, fr, it, ja, ko, pt, ru, vi, zh };

export const FALLBACK_LANGUAGE = 'en';

/** Language picker options. `label` is the endonym and is intentionally untranslated. */
export const LANGUAGES = [
  { code: 'en', label: 'English' },
  { code: 'vi', label: 'Tieng Viet' },
  { code: 'ja', label: 'Nihongo' },
  { code: 'ko', label: 'Hangugeo' },
  { code: 'zh', label: 'Zhongwen' },
  { code: 'es', label: 'Espanol' },
  { code: 'fr', label: 'Francais' },
  { code: 'de', label: 'Deutsch' },
  { code: 'it', label: 'Italiano' },
  { code: 'pt', label: 'Portugues' },
  { code: 'ru', label: 'Russkiy' },
];

/**
 * Resolve a translation key for an explicit language.
 * Falls back to English, then to the key itself so a missing string is visible
 * during development rather than rendering as an empty node.
 */
export function translate(language, key) {
  const bundle = bundles[language] ?? bundles[FALLBACK_LANGUAGE];
  return bundle[key] ?? bundles[FALLBACK_LANGUAGE][key] ?? key;
}

/**
 * The language to use before the user has ever chosen one.
 *
 * The webview reports the operating system display language, so a first run
 * can match the phone the user is already holding instead of opening in
 * English and making them go and find the setting.
 *
 * Only the primary subtag is matched, against the bundles themselves rather
 * than a second list that could drift from them: `vi-VN` and `zh-CN` resolve,
 * and a language with no bundle falls back to English rather than to a
 * half-translated neighbour.
 */
export function detectSystemLanguage() {
  const tags =
    typeof navigator === 'undefined' ? [] : [...(navigator.languages ?? []), navigator.language];

  for (const tag of tags) {
    const code = String(tag ?? '')
      .toLowerCase()
      .split('-')[0];
    if (Object.prototype.hasOwnProperty.call(bundles, code)) return code;
  }

  return FALLBACK_LANGUAGE;
}
