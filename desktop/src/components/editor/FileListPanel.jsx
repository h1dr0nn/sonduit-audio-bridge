import React, { useState } from 'react';
import { FiTrash2, FiX, FiLoader, FiRefreshCw, FiMusic, FiSquare } from 'react-icons/fi';
import { cn } from '../../utils/cn';
import { WaveformPlayer } from './WaveformPlayer';
import { formatBitrate } from '../../utils/metadataUtils';

import { useTranslation } from '../../i18n';

const statusTone = {
  loading: 'bg-blue-500/10 text-blue-600 dark:bg-blue-400/15 dark:text-blue-300',
  ready: 'bg-white/60 text-slate-800 dark:bg-white/10 dark:text-slate-100',
  pending: 'bg-white/60 text-slate-800 dark:bg-white/10 dark:text-slate-100',
  processing: 'bg-accent/10 text-accent dark:bg-accent/15 dark:text-accent',
  done: 'bg-emerald-500/15 text-emerald-600 dark:bg-emerald-400/15 dark:text-emerald-200',
  error: 'bg-red-500/15 text-red-600 dark:bg-red-400/15 dark:text-red-200',
};

const getStatusText = (status, t) => {
  const map = {
    loading: t('statusLoading'),
    ready: t('statusReady'),
    pending: t('statusPending'),
    processing: t('statusProcessing'),
    done: t('statusDone'),
    error: t('statusError'),
  };
  return map[status] || status;
};

/**
 * @param actions Rendered at the right of the panel header, before the queue's
 *   own controls. The Run tab puts its run control here: it is the widest
 *   header on that screen, and the narrow left column wrapped both the button
 *   and the heading beside it onto two lines each.
 */
export function FileListPanel({ files = [], onClearAll, onRemoveFile, onReload, actions }) {
  const { t } = useTranslation();
  const [isReloading, setIsReloading] = useState(false);
  const [previewFileId, setPreviewFileId] = useState(null);

  const handleReload = async () => {
    setIsReloading(true);
    await onReload();
    // Minimum 500ms animation
    setTimeout(() => setIsReloading(false), 500);
  };

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="mb-4 flex flex-shrink-0 items-center justify-between gap-3">
        <div>
          <p className="text-xs uppercase tracking-[0.18em] text-ink-faint">{t('session')}</p>
          <h3 className="text-lg font-semibold text-ink">{t('filesInQueue')}</h3>
        </div>
        <div className="flex items-center gap-2">
          {actions}
          {files.length > 0 && (
            <>
            <button
              type="button"
              onClick={handleReload}
              disabled={isReloading}
              className="inline-flex h-[34px] w-[34px] items-center justify-center rounded-full bg-blue-500/10 text-blue-600 shadow-md transition duration-smooth hover:-translate-y-[1px] hover:bg-blue-500/20 hover:shadow-lg disabled:cursor-not-allowed disabled:opacity-50 dark:bg-blue-500/20 dark:text-blue-400"
              aria-label="Reload files"
              title="Reset all files to ready"
            >
              <FiRefreshCw className={`h-3.5 w-3.5 ${isReloading ? 'animate-spin' : ''}`} />
            </button>
            <button
              type="button"
              onClick={onClearAll}
              className="inline-flex h-[34px] items-center gap-2 rounded-full bg-red-500/10 px-4 text-xs font-semibold text-red-600 shadow-md transition duration-smooth hover:-translate-y-[1px] hover:bg-red-500/20 hover:shadow-lg dark:bg-red-500/20 dark:text-red-400"
            >
              <FiTrash2 className="h-3.5 w-3.5" />
              {t('clearAll')}
            </button>
            </>
          )}
        </div>
      </div>

      {/* The queue is the only part of this screen that grows without bound, so
        * it is the part given the leftover height and its own scrollbar. */}
      <div className="card-sunken flex min-h-0 flex-1 flex-col overflow-hidden p-3">
        {files.length === 0 ? (
          <div className="flex h-full items-center justify-center px-4 py-3 text-sm text-ink-soft">
            <span>{t('noFilesYet')}</span>
          </div>
        ) : (
          <div className="scroll-area min-h-0 flex-1 space-y-2">
            {files.map((file, index) => (
              <div key={file.id || index} className="space-y-2">
                <article
                  className={cn(
                    "group flex items-center gap-3 rounded-xl border border-white/60 bg-white/70 px-3 py-2.5 text-sm text-slate-800 shadow-sm transition duration-smooth hover:-translate-y-[1px] hover:shadow-lg dark:border-white/5 dark:bg-white/10 dark:text-slate-50",
                    file.status === 'processing' && "ring-2 ring-accent/30 animate-pulse",
                    file.status === 'done' && "scale-[1.02]",
                    file.status === 'error' && "animate-shake"
                  )}
                >
                  <div className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-lg bg-gradient-to-br from-accent/90 to-accent/70 shadow-sm">
                    <span className="text-[10px] font-bold uppercase tracking-tight text-white">{file.format}</span>
                  </div>
                  
                  <div className="flex min-w-0 flex-1 items-center justify-between gap-3 relative z-10">
                    <div className="min-w-0 flex-1">
                      {previewFileId === file.id ? (
                        <div className="h-10 w-full">
                          <WaveformPlayer 
                            file={file.file || { path: file.path }} 
                          />
                        </div>
                      ) : (
                        <>
                          <p className="truncate text-sm font-semibold leading-5" title={file.name}>{file.name}</p>
                          <p className="text-[10px] text-slate-500 dark:text-slate-400">
                            {file.duration === '00:00' ? '--' : file.duration}
                            {file.bitrate && <> • {formatBitrate(file.bitrate)}</>}
                            {' • '}{file.size === '0 B' ? '--' : file.size}
                          </p>
                        </>
                      )}
                    </div>

                    <div className="flex flex-shrink-0 items-center gap-2">
                      {/* Preview/Stop Button */}
                      <button
                        onClick={() => setPreviewFileId(previewFileId === file.id ? null : file.id)}
                        className={cn(
                          "flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-full transition-all duration-300",
                          previewFileId === file.id 
                            ? "bg-red-500 text-white hover:bg-red-600 shadow-md scale-105" 
                            : "bg-indigo-500/10 text-indigo-600 hover:bg-indigo-500/20 dark:text-indigo-400"
                        )}
                        title={previewFileId === file.id ? "Stop Preview" : "Preview Audio"}
                      >
                        {previewFileId === file.id ? <FiSquare className="h-3.5 w-3.5 fill-current" /> : <FiMusic className="h-4 w-4" />}
                      </button>

                      {/* Status Badge - only show when NOT in preview mode */}
                      {previewFileId !== file.id && (
                        <span
                          className={cn(
                            'flex items-center gap-1.5 rounded-full px-2.5 py-1 text-xs font-semibold shadow-sm transition duration-smooth',
                            statusTone[file.status] || 'bg-white/50 text-slate-700 dark:bg-white/10 dark:text-slate-200',
                          )}
                        >
                          {file.status === 'loading' && (
                            <FiLoader className="h-3 w-3 animate-spin" />
                          )}
                          {getStatusText(file.status, t)}
                        </span>
                      )}
                      
                      {/* Remove Button - hidden during processing/loading */}
                      {previewFileId !== file.id && file.status !== 'processing' && file.status !== 'loading' && onRemoveFile && (
                        <button
                          onClick={() => onRemoveFile(file.id || index)}
                          className="flex h-6 w-6 flex-shrink-0 items-center justify-center rounded-full bg-red-500/10 text-red-600 transition duration-smooth hover:bg-red-500/20 dark:text-red-400"
                          aria-label="Remove file"
                          title="Remove file"
                        >
                          <FiX className="h-4 w-4" />
                        </button>
                      )}
                    </div>
                  </div>
                </article>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
