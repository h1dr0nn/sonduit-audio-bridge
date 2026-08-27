import React from 'react';
import { FiAlertTriangle, FiX } from 'react-icons/fi';
import { cn } from '../../utils/cn';
import { useTranslation } from '../../i18n';

export function ErrorModal({ isOpen, errorFiles, onClose }) {
  const { t } = useTranslation();

  if (!isOpen || !errorFiles || errorFiles.length === 0) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4">
      {/* Backdrop */}
      <div
        className="absolute inset-0 bg-black/50 backdrop-blur-sm transition-opacity duration-smooth"
        onClick={onClose}
      />

      {/* Modal */}
      <div className="relative z-10 w-full max-w-lg rounded-card border border-white/60 bg-white/90 p-6 shadow-2xl backdrop-blur-[32px] transition-all duration-smooth dark:border-white/10 dark:bg-slate-900/90">
        {/* Header */}
        <div className="mb-4 flex items-start justify-between">
          <div>
            <div className="mb-1 flex items-center gap-2">
              <FiAlertTriangle className="h-6 w-6 text-red-600 dark:text-red-400" />
              <h3 className="text-lg font-semibold text-slate-900 dark:text-white">
                {t('processingErrors')}
              </h3>
            </div>
            <p className="text-sm text-slate-600 dark:text-slate-300">
              {errorFiles.length} {errorFiles.length === 1 ? t('fileFailed') : t('filesFailed')}
            </p>
          </div>
          <button
            onClick={onClose}
            className="flex h-8 w-8 items-center justify-center rounded-full border border-white/60 bg-white/70 text-slate-600 transition duration-smooth hover:bg-white hover:text-slate-900 dark:border-white/10 dark:bg-white/10 dark:text-slate-300 dark:hover:bg-white/20"
            aria-label={t('close')}
          >
            <FiX className="h-5 w-5" />
          </button>
        </div>

        {/* Error List */}
        <div className="max-h-96 space-y-2 overflow-y-auto">
          {errorFiles.map((file, index) => (
            <div
              key={file.id || index}
              className="rounded-xl border border-red-200 bg-red-50 p-3 dark:border-red-900/30 dark:bg-red-900/10"
            >
              <div className="flex items-start gap-3">
                <div className="flex h-10 w-10 flex-shrink-0 items-center justify-center rounded-xl bg-red-500 text-white">
                  <span className="text-sm font-semibold">{file.format || '?'}</span>
                </div>
                <div className="min-w-0 flex-1">
                  <p className="truncate text-sm font-semibold text-slate-900 dark:text-white">
                    {file.name}
                  </p>
                  <p className="mt-1 text-xs text-red-600 dark:text-red-400">
                    {file.error || t('unknownError')}
                  </p>
                </div>
              </div>
            </div>
          ))}
        </div>

        {/* Footer */}
        <div className="mt-6 flex justify-end gap-3">
          <button
            onClick={onClose}
            className="rounded-full border border-white/70 bg-white/70 px-6 py-2.5 text-sm font-semibold text-slate-800 shadow-md transition duration-smooth hover:-translate-y-[1px] hover:shadow-lg dark:border-white/10 dark:bg-white/10 dark:text-slate-100"
          >
            {t('close')}
          </button>
        </div>
      </div>
    </div>
  );
}
