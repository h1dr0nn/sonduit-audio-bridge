import { FALLBACK_LANGUAGE, translate } from './languages';
import { useSettingsContext } from '../context/SettingsContext';

export { FALLBACK_LANGUAGE, LANGUAGES, detectSystemLanguage, translate } from './languages';

export function useTranslation() {
  const { settings } = useSettingsContext();
  const language = settings.language ?? FALLBACK_LANGUAGE;
  return {
    language,
    t: (key) => translate(language, key),
  };
}
