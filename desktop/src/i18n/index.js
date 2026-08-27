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
import { useSettingsContext } from '../context/SettingsContext';

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

export function useTranslation() {
  const { settings } = useSettingsContext();
  const language = settings.language ?? FALLBACK_LANGUAGE;
  return {
    language,
    t: (key) => translate(language, key),
  };
}
