import React, { useState } from 'react';
import { FiUpload, FiFolder } from 'react-icons/fi';
import { open } from '@tauri-apps/plugin-dialog';
import { readFile } from '@tauri-apps/plugin-fs';
import { designTokens } from '../../utils/theme';
import { isAudioFile } from '../../utils/audioUtils';

import { useTranslation } from '../../i18n';

export function DragDropArea({ onFilesAdded }) {
  const { t } = useTranslation();
  const [isDragging, setIsDragging] = useState(false);

  // ... (handlers remain same)

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

    console.log('[DragDropArea] Drop event triggered');
    console.log('[DragDropArea] dataTransfer:', e.dataTransfer);
    console.log('[DragDropArea] files:', e.dataTransfer.files);

    const droppedFiles = Array.from(e.dataTransfer.files);
    console.log('[DragDropArea] Dropped files count:', droppedFiles.length);
    console.log('[DragDropArea] Dropped files:', droppedFiles);

    const audioFiles = droppedFiles.filter(isAudioFile);
    console.log('[DragDropArea] Audio files count:', audioFiles.length);

    if (audioFiles.length > 0) {
      console.log('[DragDropArea] Calling onFilesAdded with:', audioFiles);
      onFilesAdded(audioFiles);
    } else if (droppedFiles.length > 0) {
      console.log('[DragDropArea] No audio files found in drop');
      alert('Please drop audio files only (MP3, WAV, OGG, FLAC, AAC, M4A, WMA, AIFF, OPUS)');
    } else {
      console.log('[DragDropArea] No files in drop event');
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
      
      // Create minimal file objects with paths - don't read full files yet
      const files = paths.map(path => {
        const fileName = path.split('/').pop().split('\\').pop();
        
        // Create a minimal File-like object
        // We'll load the actual data asynchronously when needed
        return {
          name: fileName,
          path: path,
          size: 0,  // Will be filled when we read the file
          type: 'audio/*',
          lastModified: Date.now(),
          // Mark this as a path-based file so we know to read it later
          _needsReading: true
        };
      });

      if (files.length > 0) {
        onFilesAdded(files);
      }
    } catch (error) {
      console.error('File picker error:', error);
      alert(`Error opening file picker: ${error.message || error}`);
    }
  };

  return (
    <div
      onDragOver={handleDragOver}
      onDragEnter={handleDragEnter}
      onDragLeave={handleDragLeave}
      onDrop={handleDrop}
      className={cn(
        'glass-surface relative flex h-full flex-col items-center justify-center overflow-hidden rounded-card border-2 bg-white p-10 shadow-soft transition-all duration-smooth dark:bg-white/10',
        isDragging
          ? 'border-accent bg-accent/10 shadow-xl dark:bg-accent/20 scale-[1.02]'
          : 'border-slate-200 hover:border-slate-300 hover:shadow-xl dark:border-white/10 dark:hover:border-white/20'
      )}
      style={{
        backdropFilter: `blur(${designTokens.blur})`,
        WebkitBackdropFilter: `blur(${designTokens.blur})`,
      }}
    >
      <div className={cn(
        'absolute inset-0 bg-gradient-to-br transition-all duration-smooth',
        isDragging
          ? 'from-accent/20 via-accent/10 to-accent/5'
          : 'from-white/40 via-white/20 to-white/5 opacity-70 dark:from-white/5 dark:via-white/0 dark:to-white/5'
      )} />
      
      <div className="relative flex flex-col items-center gap-4 text-center">
        <div className={cn(
          'flex h-16 w-16 items-center justify-center rounded-2xl shadow-inner backdrop-blur-[20px] transition-all duration-smooth',
          isDragging
            ? 'bg-accent/20 dark:bg-accent/30'
            : 'bg-white/80 dark:bg-white/10'
        )}>
          {isDragging ? (
            <FiFolder className="h-8 w-8 text-accent" />
          ) : (
            <FiUpload className="h-8 w-8 text-slate-600 dark:text-slate-300" />
          )}
        </div>
        
        <div className="space-y-2">
          <p className="text-lg font-semibold text-slate-900 dark:text-slate-100">
            {isDragging ? t('dropFilesHere') : t('dropAudioFiles')}
          </p>
          <p className="max-w-xs text-sm text-slate-600 dark:text-slate-300">
            {isDragging 
              ? t('releaseToAdd')
              : t('dragOrClick')}
          </p>
        </div>
        
        <button
          onClick={handleFilePickerClick}
          type="button"
          className="flex items-center gap-2 rounded-full bg-accent px-5 py-2 text-sm font-semibold text-white shadow-md transition duration-smooth hover:-translate-y-[1px] hover:shadow-lg focus:outline-none focus:ring-2 focus:ring-accent focus:ring-offset-2"
        >
          <FiFolder className="h-4 w-4" />
          {t('browseFiles')}
        </button>
      </div>
    </div>
  );
}

// Import cn utility
function cn(...classes) {
  return classes.filter(Boolean).join(' ');
}
