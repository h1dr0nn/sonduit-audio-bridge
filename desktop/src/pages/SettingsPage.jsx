import React, { useEffect, useState } from 'react';
import { Button } from '../components/ui/Button';
import { Card } from '../components/ui/Card';
import { NumberField } from '../components/ui/NumberField';
import { Select } from '../components/ui/Select';
import { SettingRow } from '../components/ui/SettingRow';
import { showToast } from '../components/ui/Toast';
import { useSettingsContext } from '../context/SettingsContext';
import { useBridge } from '../hooks/useBridge';
import { useTranslation, LANGUAGES } from '../i18n';
import { cn } from '../utils/cn';

const ACCENT_COLORS = ['#7c93e8', '#007aff', '#5856d6', '#2fa96b', '#d99b28', '#d9503f'];
const FONT_SIZES = ['small', 'medium', 'large'];
const TRANSPORTS = ['auto', 'wifi', 'usb'];
const THEMES = ['light', 'dark'];

const TRANSPORT_LABEL_KEY = {
  auto: 'connection.transportAuto',
  wifi: 'connection.transportWifi',
  usb: 'connection.transportUsb',
};

export function SettingsPage({ theme, onSetTheme }) {
  const { settings, updateSetting, updateSettings, resetSettings } = useSettingsContext();
  const { listEndpoints } = useBridge();
  const { t } = useTranslation();
  const [endpoints, setEndpoints] = useState([]);

  // Read once, when the page opens. A device list is only true at the moment
  // it is read, and this is the moment the user is looking at it; polling
  // would walk every endpoint's property store on a timer for a page that is
  // usually not open.
  useEffect(() => {
    let live = true;
    listEndpoints()
      .then((found) => {
        if (live) setEndpoints(found);
      })
      .catch((reason) => {
        // Said out loud rather than swallowed. Without this the dropdown would
        // silently offer nothing but the system default, which looks like a
        // machine with one output rather than like a failure.
        showToast({
          id: 'endpoints',
          tone: 'error',
          titleKey: 'settings.captureDeviceFailed',
          detail: String(reason),
        });
      });
    return () => {
      live = false;
    };
  }, [listEndpoints]);

  const deviceOptions = [
    { value: '', label: t('settings.captureDeviceDefault') },
    ...endpoints.map((endpoint) => ({
      value: endpoint.id,
      label: endpoint.isDefault
        ? `${endpoint.name} (${t('settings.captureDeviceIsDefault')})`
        : endpoint.name,
    })),
  ];

  // The chosen device is not in the list when it has been unplugged or
  // disabled. Keeping it as an option, labelled, is the difference between
  // "your headset is out" and a control that has apparently forgotten what it
  // was set to. A session started like this falls back to the default anyway,
  // and the connection panel names the device it actually ended up on.
  const chosenIsMissing =
    settings.captureDeviceId &&
    !endpoints.some((endpoint) => endpoint.id === settings.captureDeviceId);
  if (chosenIsMissing) {
    const remembered = settings.captureDeviceName || settings.captureDeviceId;
    deviceOptions.push({
      value: settings.captureDeviceId,
      label: `${remembered} (${t('settings.captureDeviceMissing')})`,
    });
  }

  // The name is stored beside the id, because the id is the only thing the
  // backend can be given and the name is the only thing the user recognises.
  const chooseDevice = (id) => {
    const chosen = endpoints.find((endpoint) => endpoint.id === id);
    updateSettings({ captureDeviceId: id, captureDeviceName: chosen?.name ?? '' });
  };

  return (
    /* Fills the window: the heading is pinned and the rows scroll under it.
     *
     * `min-h-0` on every flex child in the chain is what allows that. Without
     * it a flex child keeps `min-height: auto`, refuses to shrink below its
     * content, and hands the overflow back to the shell, which is how this
     * page used to drag the whole layout into a scroll. */
    <div className="flex h-full min-h-0 flex-col gap-4">
      <header className="shrink-0 px-1">
        <h1 className="text-3xl font-semibold tracking-tight text-ink">{t('settings.title')}</h1>
      </header>

      {/* This is the one page whose content is genuinely longer than any window
        * it will be given, so the list keeps its own scrollbar. The cards do
        * not shrink to fit: a squashed settings card is worse than a scroll. */}
      <div className="scroll-area flex min-h-0 flex-1 flex-col gap-4">
        <Card className="shrink-0" title={t('settings.appearance')}>
          <SettingRow label={t('settings.theme')} description={t('settings.themeDesc')}>
            <Select
              className="w-36"
              ariaLabel={t('settings.theme')}
              value={theme}
              onChange={onSetTheme}
              options={THEMES.map((mode) => ({ value: mode, label: t(`common.${mode}`) }))}
            />
          </SettingRow>

          <SettingRow label={t('settings.accentColor')} description={t('settings.accentColorDesc')}>
            {ACCENT_COLORS.map((color) => (
              <button
                key={color}
                type="button"
                aria-label={color}
                aria-pressed={settings.accentColor === color}
                onClick={() => updateSetting('accentColor', color)}
                style={{ backgroundColor: color }}
                className={cn(
                  'h-6 w-6 rounded-pill transition-transform duration-fast ease-out',
                  settings.accentColor === color
                    ? 'ring-2 ring-[var(--text-primary)] ring-offset-2 ring-offset-[var(--surface-card)]'
                    : 'hover:scale-110',
                )}
              />
            ))}
          </SettingRow>

          <SettingRow label={t('settings.fontSize')} description={t('settings.fontSizeDesc')}>
            <Select
              className="w-36"
              ariaLabel={t('settings.fontSize')}
              value={settings.fontSize}
              onChange={(next) => updateSetting('fontSize', next)}
              options={FONT_SIZES.map((size) => ({ value: size, label: t(`common.${size}`) }))}
            />
          </SettingRow>

          <SettingRow label={t('settings.language')} description={t('settings.languageDesc')}>
            <Select
              className="w-36"
              ariaLabel={t('settings.language')}
              value={settings.language}
              onChange={(next) => updateSetting('language', next)}
              options={LANGUAGES.map((language) => ({
                value: language.code,
                label: language.label,
              }))}
            />
          </SettingRow>
        </Card>

        <Card className="shrink-0" title={t('settings.audio')}>
          <SettingRow
            label={t('settings.captureDevice')}
            description={t('settings.captureDeviceDesc')}
          >
            <Select
              className="w-64"
              ariaLabel={t('settings.captureDevice')}
              value={settings.captureDeviceId}
              onChange={chooseDevice}
              options={deviceOptions}
            />
          </SettingRow>

          <SettingRow label={t('settings.targetBuffer')} description={t('settings.targetBufferDesc')}>
            <NumberField
              ariaLabel={t('settings.targetBuffer')}
              value={settings.targetBufferMs}
              onChange={(next) => updateSetting('targetBufferMs', next)}
              min={5}
              max={200}
              step={5}
              unit="ms"
            />
          </SettingRow>

          <SettingRow label={t('connection.transport')}>
            <Select
              className="w-36"
              ariaLabel={t('connection.transport')}
              value={settings.preferredTransport}
              onChange={(next) => updateSetting('preferredTransport', next)}
              options={TRANSPORTS.map((transport) => ({
                value: transport,
                label: t(TRANSPORT_LABEL_KEY[transport]),
              }))}
            />
          </SettingRow>

          <SettingRow label={t('settings.reset')} description={t('settings.resetDesc')}>
            <Button onClick={resetSettings} size="sm">
              {t('settings.reset')}
            </Button>
          </SettingRow>
        </Card>
      </div>
    </div>
  );
}
