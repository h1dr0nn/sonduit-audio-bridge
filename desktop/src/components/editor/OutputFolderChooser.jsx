import React from 'react';
import { FiFolder } from 'react-icons/fi';
import { open } from '@tauri-apps/plugin-dialog';
import { useTranslation } from '../../i18n';

export function OutputFolderChooser({ path, onChoose }) {
  const { t } = useTranslation();

  const handleChooseFolder = async () => {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: t('selectOutputFolder')
      });

      if (selected && typeof selected === 'string') {
        onChoose(selected);
      }
    } catch (error) {
      console.error('Folder picker error:', error);
    }
  };

  return (
    <div className="space-y-2">
      <p className="text-xs uppercase tracking-[0.18em] text-ink-faint">{t('outputFolderLabel')}</p>
      <div className="space-y-2">
        <div className="card-sunken flex items-center gap-2 px-4 py-2.5 text-sm text-ink">
          <FiFolder className="h-4 w-4 flex-shrink-0 text-ink-faint" strokeWidth={1.9} />
          <span className="flex-1 truncate font-mono text-xs">
            {path || t('noFolderSelected')}
          </span>
        </div>
        <button
          type="button"
          onClick={handleChooseFolder}
          className="w-full rounded-pill border border-line-strong px-4 py-2.5 text-sm font-semibold text-ink transition-colors duration-fast ease-out hover:bg-sunken focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
        >
          {t('chooseFolder')}
        </button>
        {!path && (
          <p className="text-xs text-ink-faint">
            {t('autoFilled')}
          </p>
        )}
      </div>
    </div>
  );
}
