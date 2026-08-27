import React, { useState } from 'react';
import { FiUpload, FiFolder } from 'react-icons/fi';
import { open } from '@tauri-apps/plugin-dialog';
import { Dialog } from '../ui/Dialog';
import { cn } from '../../utils/cn';
import { isAudioFile } from '../../utils/audioUtils';

import { useTranslation } from '../../i18n';

export function DragDropArea({ onFilesAdded }) {
  const { t } = useTranslation();
  const [isDragging, setIsDragging] = useState(false);

  // `null` when no dialog is up. Holds the two keys plus an optional technical
  // detail, so one Dialog instance serves both failure paths.
  const [notice, setNotice] = useState(null);

  const handleDragOver = (e) => {
    e.preventDefault();
    e.stopPropagation();
  };

  const handleDragEnter = (e) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(true);
  };

  const handleDragLeave = (e) => {
    e.preventDefault();
    e.stopPropagation();
    const rect = e.currentTarget.getBoundingClientRect();
    if (
      e.clientX <= rect.left ||
      e.clientX >= rect.right ||
      e.clientY <= rect.top ||
      e.clientY >= rect.bottom
    ) {
      setIsDragging(false);
    }
  };

  const handleDrop = (e) => {
    e.preventDefault();
    e.stopPropagation();
    setIsDragging(false);

    const droppedFiles = Array.from(e.dataTransfer.files);
    const audioFiles = droppedFiles.filter(isAudioFile);

    if (audioFiles.length > 0) {
      onFilesAdded(audioFiles);
      return;
    }

    // A drop of nothing at all is not worth interrupting for; a drop of the
    // wrong kind of file is, because otherwise it looks like the app ignored it.
    if (droppedFiles.length > 0) {
      setNotice({ titleKey: 'dropNotAudioTitle', bodyKey: 'dropNotAudioBody' });
    }
  };

  const handleFilePickerClick = async (e) => {
    e.preventDefault();
    e.stopPropagation();

    try {
      const selected = await open({
        multiple: true,
        filters: [{
          name: 'Audio Files',
          extensions: ['mp3', 'wav', 'ogg', 'flac', 'aac', 'm4a', 'wma', 'aiff', 'opus']
        }]
      });

      if (!selected) return;

      const paths = Array.isArray(selected) ? selected : [selected];

      // Only the path is captured here. Reading the bytes is deferred to the
      // queue, so picking a hundred files does not stall the picker closing.
      const files = paths.map(path => {
        const fileName = path.split('/').pop().split('\\').pop();

        return {
          name: fileName,
          path: path,
          size: 0,
          type: 'audio/*',
          lastModified: Date.now(),
          _needsReading: true
        };
      });

      if (files.length > 0) {
        onFilesAdded(files);
      }
    } catch (error) {
      console.error('File picker error:', error);
      setNotice({
        titleKey: 'filePickerFailedTitle',
        bodyKey: 'filePickerFailedBody',
        detail: error?.message || String(error),
      });
    }
  };

  return (
    <div
      onDragOver={handleDragOver}
      onDragEnter={handleDragEnter}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
      className={cn(
        'flex min-h-[13rem] flex-1 flex-col items-center justify-center gap-4',
        'rounded-inner border-2 border-dashed p-6 text-center',
        'transition-colors duration-normal ease-out',
        isDragging
          ? 'border-accent bg-accent-soft'
          : 'border-line-strong bg-sunken',
      )}
    >
      <div
        className={cn(
          'flex h-14 w-14 shrink-0 items-center justify-center rounded-inner',
          'transition-colors duration-normal ease-out',
          isDragging ? 'bg-accent-soft text-accent' : 'bg-card text-ink-soft',
        )}
      >
        {isDragging ? (
          <FiFolder className="h-6 w-6" strokeWidth={1.8} />
        ) : (
          <FiUpload className="h-6 w-6" strokeWidth={1.8} />
        )}
      </div>

      <div className="space-y-1">
        <p className="text-base font-semibold text-ink">
          {isDragging ? t('dropFilesHere') : t('dropAudioFiles')}
        </p>
        <p className="text-sm text-ink-soft">
          {isDragging ? t('releaseToAdd') : t('dragOrClick')}
        </p>
      </div>

      <button
        onClick={handleFilePickerClick}
        type="button"
        className={cn(
          'flex items-center gap-2 rounded-pill px-5 py-2 text-sm font-semibold text-white',
          'transition-opacity duration-fast ease-out hover:opacity-90',
          'focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
        )}
        style={{ background: 'var(--accent-color)' }}
      >
        <FiFolder className="h-4 w-4" strokeWidth={1.9} />
        {t('browseFiles')}
      </button>

      <Dialog
        open={notice !== null}
        tone="warning"
        title={notice ? t(notice.titleKey) : ''}
        onClose={() => setNotice(null)}
      >
        {notice && (
          <>
            <p>{t(notice.bodyKey)}</p>
            {notice.detail && (
              <p className="mt-2 break-words font-mono text-xs text-ink-faint">{notice.detail}</p>
            )}
          </>
        )}
      </Dialog>
    </div>
  );
}
