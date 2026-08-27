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
      <p className="text-xs uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">{t('outputFolderLabel')}</p>
      <div className="space-y-2">
        <div className="flex items-center gap-2 rounded-xl border border-slate-200 bg-slate-50 px-4 py-2.5 text-sm text-slate-700 shadow-sm dark:border-white/10 dark:bg-white/5 dark:text-slate-200">
          <FiFolder className="h-4 w-4 flex-shrink-0 text-slate-500 dark:text-slate-400" />
          <span className="flex-1 truncate font-mono text-xs">
            {path || t('noFolderSelected')}
          </span>
        </div>
        <button
          type="button"
          onClick={handleChooseFolder}
          className="w-full rounded-xl border border-slate-200 bg-white px-4 py-2.5 text-sm font-semibold text-slate-800 shadow-md transition duration-smooth hover:-translate-y-[1px] hover:shadow-lg focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent dark:border-white/10 dark:bg-white/10 dark:text-slate-100"
        >
          {t('chooseFolder')}
        </button>
        {!path && (
          <p className="text-xs text-slate-500 dark:text-slate-400">
            {t('autoFilled')}
          </p>
        )}
      </div>
    </div>
  );
}
