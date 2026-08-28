import React, { useState, useEffect, useCallback, useRef } from 'react';
import { FiFolder, FiPlay, FiSettings, FiSliders } from 'react-icons/fi';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/core';
import { dirname } from '@tauri-apps/api/path';
import { readFile } from '@tauri-apps/plugin-fs';
import { open } from '@tauri-apps/plugin-dialog';
import { DragDropArea } from '../components/editor/DragDropArea';
import { FileListPanel } from '../components/editor/FileListPanel';
import { FormatSelector } from '../components/editor/FormatSelector';
import { OutputFolderChooser } from '../components/editor/OutputFolderChooser';
import { ProgressIndicator } from '../components/editor/ProgressIndicator';
import { showToast } from '../components/ui/Toast';
import { ModeSelector } from '../components/editor/ModeSelector';
import { MasterControls, PRESETS } from '../components/editor/MasterControls';
import { TrimControls } from '../components/editor/TrimControls';
import { ModifyControls } from '../components/editor/ModifyControls';
import { ErrorModal } from '../components/editor/ErrorModal';
import { useConvertAudio } from '../hooks/useTauriCommand';
import { getFileMetadata, formatFileSize, formatDuration, getAudioDuration } from '../utils/audioUtils';
import { notifySuccess, notifyError } from '../utils/notifications';

import { useSettingsContext } from '../context/SettingsContext';
import { useTranslation } from '../i18n';
import { cn } from '../utils/cn';

const formatOptions = ['AAC', 'MP3', 'WAV', 'FLAC', 'OGG', 'M4A'];

/* Two columns, never stacked rows. The window's own minWidth is 960, so a
 * responsive fallback below the `lg` breakpoint is unreachable in the shipped
 * application and only ever showed up as wasted vertical space. The track
 * sizes are shared by all three tabs so the columns do not jump when the tab
 * changes; the left one is a little wider than the Process tab used to be
 * because the drop zone and the output path need more room than a list of
 * mode buttons. */
const TAB_COLUMNS = 'grid min-h-0 flex-1 grid-cols-[minmax(280px,340px)_1fr] gap-4';

/* A column that fills the tab and takes its own scrollbar.
 *
 * `min-h-0` is load-bearing, not decoration: a flex or grid child defaults to
 * `min-height: auto`, refuses to shrink below its content, and pushes the
 * overflow out to the page instead of scrolling inside itself. */
const SCROLL_COLUMN = 'scroll-area flex min-h-0 min-w-0 flex-col gap-4';

/* A card that is itself the scroll container for its one piece of content. */
const SCROLL_CARD = 'card flex min-h-0 min-w-0 flex-col p-5';

/** The three stages of a job: bring audio in, say what to do, watch it run. */
const EDITOR_TABS = [
  { id: 'files', Icon: FiFolder, labelKey: 'editor.tabFiles' },
  { id: 'process', Icon: FiSliders, labelKey: 'editor.tabProcess' },
  { id: 'run', Icon: FiPlay, labelKey: 'editor.tabRun' },
];

export function EditorPage({ onOpenSettings }) {
  // Harmonix lifted this state into App so it survived navigating to the
  // settings screen. Sonduit keeps every screen mounted-on-demand instead, so
  // the editor owns its own queue.
  const [files, setFiles] = useState([]);
  const [outputFolder, setOutputFolder] = useState('');
  const [editorTab, setEditorTab] = useState('files');
  const { convert, loading: converting } = useConvertAudio();
  const { settings } = useSettingsContext();
  const { t } = useTranslation();

  // Mode state
  const [mode, setMode] = useState('format'); // 'format' | 'enhance' | 'clean' | 'modify'

  // Convert mode - use default from settings
  const [selectedFormat, setSelectedFormat] = useState(settings.defaultFormat || 'AAC');

  // Master mode
  const [masterPreset, setMasterPreset] = useState('Music');
  const [masterParams, setMasterParams] = useState({
    target_lufs: -14.0,
    apply_compression: true,
    apply_limiter: true,
    output_gain: 0.0
  });

  // Trim mode
  const [trimThreshold, setTrimThreshold] = useState(-50.0);
  const [trimMinSilence, setTrimMinSilence] = useState(500);
  const [trimPadding, setTrimPadding] = useState(0);

  // Modify mode
  const [modifyParams, setModifyParams] = useState({
    speed: 1.0,
    pitch: 0,
    cutStart: 0,
    cutEnd: 100,
    isCutEnabled: false
  });

  // Progress tracking
  const [progress, setProgress] = useState(0);
  const [currentFile, setCurrentFile] = useState('');
  const [processingStatus, setProcessingStatus] = useState('Idle');

  // Error handling
  const [errorFiles, setErrorFiles] = useState([]);

  // Session summary
  const sessionSummary = {
    filesCount: files.length,
    format: mode === 'format' ? selectedFormat : 
            mode === 'enhance' ? masterPreset : 
            mode === 'clean' ? t('autoTrim') : 
            mode === 'modify' ? `${t('speed')} ${modifyParams.speed}x` : t('modify'),
    status: processingStatus
  };

  // Sync default format from settings
  useEffect(() => {
    setSelectedFormat(settings.defaultFormat || 'AAC');
  }, [settings.defaultFormat]);

  // Keep ref of files to avoid re-creating handleFilesAdded on every file change
  const filesRef = useRef(files);
  useEffect(() => {
    filesRef.current = files;
  }, [files]);

  // Handle files added from drag & drop or file picker
  /**
   * Report something to the user.
   *
   * These used to render as a card at the bottom of the middle column, so
   * every completed step shoved the content above it out of place. They go to
   * the app's toast stack now, which floats over the page instead.
   *
   * One id, so a run's messages replace each other rather than stacking: the
   * interesting one is always the latest.
   */
  const notify = useCallback(
    ({ type = 'info', message }) =>
      showToast({ id: 'editor', tone: type, titleKey: type, message }),
    [],
  );

  const handleFilesAdded = useCallback(async (newFiles) => {
    try {
      // Create minimal file objects immediately to show in UI
      const immediateFiles = newFiles.map(file => {
        const name = file.name || 'Unknown';
        const parts = name.split('.');
        let format = parts.length > 1 ? parts.pop().toUpperCase() : 'FILE';
        format = format.replace(/[^A-Z0-9]/g, '').substring(0, 4) || 'FILE';
        
        return {
          id: crypto.randomUUID(),
          file,
          name,
          path: file.path || name,
          format,
          size: formatFileSize(file.size || 0),
          sizeBytes: file.size || 0,
          duration: '--',
          status: 'loading',
          error: null,
          output: null
        };
      });

      // Filter out duplicates by comparing paths using ref
      const existingPaths = new Set(filesRef.current.map(f => f.path));
      const newUniqueFiles = immediateFiles.filter(file => !existingPaths.has(file.path));
      
      if (newUniqueFiles.length === 0) {
        const duplicateCount = immediateFiles.length;
        if (duplicateCount > 0) {
          notify({ 
            type: 'info', 
            message: `${duplicateCount} ${t('duplicateSkipped')}` 
          });
        }
        return;
      }

      const duplicateCount = immediateFiles.length - newUniqueFiles.length;
      if (duplicateCount > 0) {
        notify({ 
          type: 'info', 
          message: `${duplicateCount} ${t('duplicateSkipped')}` 
        });
      }

      // Add files to UI immediately
      setFiles(prev => [...prev, ...newUniqueFiles]);
      notify({ type: 'info', message: t('addingFiles', { count: newUniqueFiles.length }) });

      // Auto-fill output folder if not already set
      if (!outputFolder && newUniqueFiles.length > 0) {
        if (settings.outputLocation === 'Same as source') {
          try {
            const firstFilePath = newUniqueFiles[0].path;
            
            // Check if path contains directory separators
            if (firstFilePath && (firstFilePath.includes('/') || firstFilePath.includes('\\'))) {
              const dir = await dirname(firstFilePath);
              setOutputFolder(dir);
            } else {
              // Drag files don't have full path, fallback to Downloads
              const { downloadDir } = await import('@tauri-apps/api/path');
              const downloads = await downloadDir();
              setOutputFolder(downloads);
            }
          } catch (error) {
            console.error('Failed to get directory:', error);
          }
        } else if (settings.outputLocation === 'Custom folder' && settings.customOutputFolder) {
          setOutputFolder(settings.customOutputFolder);
        } else {
          // Default to Downloads if no setting
          try {
            const { downloadDir } = await import('@tauri-apps/api/path');
            const downloads = await downloadDir();
            setOutputFolder(downloads);
          } catch (error) {
            console.error('Failed to get Downloads folder:', error);
          }
        }
      }

      // Load metadata for each file independently in background
      const maxSizeMB = parseInt(settings.maxFileSize);
      const maxSizeBytes = maxSizeMB * 1024 * 1024;

      newUniqueFiles.forEach(immediateFile => {
        // Each file gets its own async processing
        (async () => {
          try {
            // Check size limit first (already have size for drag, need to read for browse)
            let actualFile = immediateFile.file;
            let fileSize = immediateFile.sizeBytes;

            // If this is a path-based file from browse, read it now
            if (actualFile._needsReading) {
              try {
                const content = await readFile(actualFile.path);
                actualFile = new File([content], actualFile.name, {
                  type: 'audio/*',
                  lastModified: actualFile.lastModified
                });
                actualFile.path = immediateFile.path;
                fileSize = actualFile.size;

                // Update size in UI
                setFiles(prev => prev.map(f => 
                  f.id === immediateFile.id 
                    ? { ...f, size: formatFileSize(fileSize), sizeBytes: fileSize }
                    : f
                ));
              } catch (error) {
                console.error('Failed to read file:', immediateFile.path, error);
                setFiles(prev => prev.map(f => 
                  f.id === immediateFile.id 
                    ? { ...f, status: 'error', error: t('failedToRead') }
                    : f
                ));
                return;
              }
            }

            // Check size limit
            if (fileSize > maxSizeBytes) {
              setFiles(prev => prev.map(f => 
                f.id === immediateFile.id 
                  ? { ...f, status: 'error', error: t('exceedsLimit', { size: maxSizeMB }) }
                  : f
              ));
              return;
            }

            // Load duration in background
            let duration = '--';
            if (actualFile instanceof File && typeof actualFile.arrayBuffer === 'function') {
              try {
                const durationSeconds = await getAudioDuration(actualFile);
                duration = formatDuration(durationSeconds);
              } catch (error) {
                // Duration stays as '--' if fails
              }
            }

            // Load metadata in background (non-blocking)
            let metadata = {};
            try {
              const analysisPayload = {
                files: [immediateFile.path],
                format: 'wav',
                output: './',
                operation: 'analyze'
              };
              
              const result = await invoke('analyze_audio', { payload: analysisPayload });
              
              if (result.status === 'success' && result.data && result.data.length > 0) {
                const analysis = result.data[0];
                metadata = {
                  bitrate: analysis.bit_rate,
                  channels: analysis.channels,
                  sampleRate: analysis.sample_rate,
                  codec: analysis.codec_name || analysis.codec
                };
              }
            } catch (error) {
              // Metadata is optional - don't fail if analysis fails
              console.warn('Metadata analysis failed for', immediateFile.name, error);
            }

            // Update with duration, metadata and mark as ready
            setFiles(prev => prev.map(f => 
              f.id === immediateFile.id 
                ? { ...f, duration, ...metadata, status: 'ready' }
                : f
            ));
          } catch (error) {
            console.error('Failed to load metadata for', immediateFile.name, error);
            setFiles(prev => prev.map(f => 
              f.id === immediateFile.id 
                ? { ...f, status: 'error', error: t('failedToLoad') }
                : f
            ));
          }
        })();
      });

    } catch (error) {
      console.error('Error adding files:', error);
      notify({ type: 'error', message: t('failedToAdd') });
    }
  }, [outputFolder, settings, setOutputFolder]);

  // Files handed to the window from outside it: opened through the shell
  // association, or asked for from the window menu.
  useEffect(() => {
    let unlistenFileOpened;
    let unlistenRequestOpen;

    const setupListeners = async () => {
      unlistenFileOpened = await listen('file-opened', (event) => {
        const paths = event.payload;
        if (Array.isArray(paths) && paths.length > 0) {
          const fileObjs = paths.map(path => {
            const name = path.split(/[/\\]/).pop();
            return { name, path, size: 0, _needsReading: true };
          });
          handleFilesAdded(fileObjs);
        }
      });

      unlistenRequestOpen = await listen('request-open-file', async () => {
        try {
          const selected = await open({
            multiple: true,
            filters: [{
              name: 'Audio',
              extensions: ['mp3', 'wav', 'flac', 'm4a', 'ogg', 'aac', 'aiff']
            }]
          });
          
          if (selected) {
            const paths = Array.isArray(selected) ? selected : [selected];
            const fileObjs = paths.map(path => {
              const name = path.split(/[/\\]/).pop();
              return { name, path, size: 0, _needsReading: true };
            });
            handleFilesAdded(fileObjs);
          }
        } catch (err) {
          console.error('Failed to open dialog:', err);
        }
      });
    };

    setupListeners();

    return () => {
      if (unlistenFileOpened) unlistenFileOpened();
      if (unlistenRequestOpen) unlistenRequestOpen();
    };
    // Re-subscribing whenever the callback identity changes is deliberate: it
    // closes over the queue for duplicate detection, and a stale copy would
    // let the same file in twice.
  }, [handleFilesAdded]);

  // Handle clear all
  const handleClearAll = () => {
    setFiles([]);
    setOutputFolder('');
    setProgress(0);
    setCurrentFile('');
    setProcessingStatus('Idle');
    setErrorFiles([]);
  };

  // Handle remove individual file
  const handleRemoveFile = (fileId) => {
    setFiles(prev => prev.filter(f => f.id !== fileId));
  };

  // Handle reload - reset all files to ready
  const handleReload = async () => {
    setFiles(prev => prev.map(f => ({
      ...f,
      status: f.status === 'error' || f.status === 'done' ? 'ready' : f.status,
      error: null,
      output: null
    })));
  };

  // Build payload for backend based on mode
  const buildPayload = () => {
    const filePaths = files.filter(f => f.status === 'ready').map(f => f.path);

    const basePayload = {
      files: filePaths,  // Required by Rust command
      output: outputFolder || './',
      concurrent_files: parseInt(settings.concurrentFiles) || 2,
    };

    if (mode === 'format') {
      return {
        operation: 'convert',
        ...basePayload,
        format: selectedFormat.toLowerCase()
      };
    } else if (mode === 'enhance') {
      return {
        operation: 'master',
        ...basePayload,
        format: 'wav',  // Dummy value for Rust validation
        input_paths: filePaths,  // For Python backend
        output_directory: outputFolder || './',
        preset: masterPreset,
        parameters: masterParams
      };
    } else if (mode === 'clean') {
      return {
        operation: 'trim',
        ...basePayload,
        format: 'wav',  // Dummy value for Rust validation
        input_paths: filePaths,  // For Python backend
        output_directory: outputFolder || './',
        silence_threshold: trimThreshold,
        minimum_silence_ms: trimMinSilence,
        padding_ms: trimPadding
      };
    } else if (mode === 'modify') {
      return {
        operation: 'modify',
        ...basePayload,
        format: 'wav',  // Dummy value for Rust validation
        input_paths: filePaths,  // For Python backend
        output_directory: outputFolder || './',
        speed: modifyParams.speed,
        pitch: modifyParams.pitch,
        cut_start: modifyParams.cutStart,
        cut_end: modifyParams.cutEnd
      };
    }
  };

  // Handle Smart Analysis
  const handleSmartAnalysis = async () => {
    console.log('[HomePage] Smart Analysis started');
    if (files.length === 0) {
      notify({ type: 'info', message: t('pleaseAddAnalyze') });
      return;
    }

    try {
      notify({ type: 'info', message: t('analyzing') });
      
      // Analyze the first file
      const fileToAnalyze = files[0];
      console.log('[HomePage] Analyzing file:', fileToAnalyze.path);
      
      const payload = {
        files: [fileToAnalyze.path],
        format: 'wav', // Dummy
        output: './', // Dummy
        operation: 'analyze'
      };

      const result = await invoke('analyze_audio', { payload });
      console.log('[HomePage] Analysis result:', result);
      
      if (result.status === 'success' && result.data && result.data.length > 0) {
        const analysis = result.data[0];
        const suggestion = analysis.suggestion || 'Music';
        
        console.log('[HomePage] Suggestion:', suggestion);
        setMasterPreset(suggestion);
        
        // Update parameters based on suggestion
        if (PRESETS[suggestion]) {
          setMasterParams({
            target_lufs: PRESETS[suggestion].target_lufs,
            apply_compression: PRESETS[suggestion].apply_compression,
            apply_limiter: PRESETS[suggestion].apply_limiter,
            output_gain: PRESETS[suggestion].output_gain
          });
        }

        notify({ 
          type: 'success', 
          message: t('detectedContent', { content: suggestion }) 
        });
      } else {
        throw new Error(result.message || t('analysisFailed'));
      }

    } catch (error) {
      console.error('Analysis error:', error);
      notify({ type: 'error', message: t('smartAnalysisFailed') });
    }
  };

  // Handle process button
  const handleProcess = async () => {
    if (files.length === 0) {
      notify({ type: 'error', message: t('pleaseAddProcess') });
      return;
    }

    if (!outputFolder) {
      notify({ type: 'error', message: t('pleaseSelectOutput') });
      return;
    }

    try {
      // Reset all files to ready (in case they were done/error from previous run)
      setFiles(prev => prev.map(f => ({ ...f, status: 'ready', error: null, output: null })));
      setProgress(0);
      setProcessingStatus(t('starting'));
      setErrorFiles([]);

      const payload = buildPayload();
      
      if (settings.enableLogging) {
        console.log('[HomePage] Starting conversion with payload:', payload);
        console.log('[HomePage] Files:', files.length);
        console.log('[HomePage] Mode:', mode);
      }

      await convert(payload);

    } catch (error) {
      console.error('Processing error:', error);
      if (settings.enableLogging) {
        console.error('[HomePage] Full error details:', error);
      }
      
      // Mark all files as failed
      setFiles(prev => prev.map(f => ({
        ...f,
        status: 'error',
        error: t('failedToStart')
      })));
      
      setProgress(0);
      setProcessingStatus(t('failed'));
      
      // Show user-friendly error
      const isFFmpegMissing = error.message?.toLowerCase().includes('located') || 
                             error.message?.toLowerCase().includes('install ffmpeg');
      
      let userMessage = t('processingFailedTryAgain');
      
      if (isFFmpegMissing) {
        userMessage = t('toolsNotFound');
      } else if (error.message?.includes('missing field')) {
        userMessage = t('invalidConfig');
      } else if (error.message) {
        // Only show error message if it's not too technical
        const simplifiedMessage = error.message.replace(/`.*?`/g, '').trim();
        if (simplifiedMessage.length < 100 && !simplifiedMessage.includes('Error:')) {
          userMessage = simplifiedMessage;
        }
      }
      
      notify({ type: 'error', message: userMessage });
    }
  };

  // Listen to conversion progress events
  useEffect(() => {
    let unlisten;

    const setupListener = async () => {
      unlisten = await listen('conversion-progress', (event) => {
        const payload = event.payload;

        // IGNORE analysis events - they're for metadata only, not conversion progress
        if (payload.operation_type === 'analyze') {
          console.log('[HomePage] Ignoring analysis event');
          return;
        }

        // Handle progress events
        if (payload.event === 'progress') {
          const { index, total, file, status } = payload;
          
          setCurrentFile(file);
          setProcessingStatus(`Processing ${index}/${total}`);

          // Update individual file status
          setFiles(prev => prev.map((f, i) => 
            i === index - 1 ? { ...f, status: status === 'processing' ? 'processing' : f.status } : f
          ));
        }

          // Handle complete event
        if (payload.event === 'complete') {
          const { status, message, outputs = [] } = payload;

          if (status === 'success') {
            setProcessingStatus(t('processingCompleteTitle'));
            setProgress(100);
            setCurrentFile('');
            
            // Mark all files as done
            setFiles(prev => prev.map((f, i) => ({
              ...f,
              status: 'done',
              output: (outputs && outputs[i]) || null
            })));

            notify({ type: 'success', message: message || t('processingComplete') });
            if (settings.notifications) {
              notifySuccess(t('processingCompleteTitle'), t('filesProcessedSuccess', { count: files.length }));
            }

            // Auto-clear if enabled
            if (settings.autoClear) {
              setTimeout(() => {
                handleClearAll();
              }, 2000); // Wait 2s for user to see completion
            }
          } else {
            // Error occurred - mark all files as failed
            setFiles(prev => prev.map(f => ({
              ...f,
              status: 'error',
              error: t('processingFailed')
            })));

            setProgress(0);
            setProcessingStatus(t('failed'));
            setCurrentFile('');
            
            // Show user-friendly error message
            const isFFmpegMissing = message?.toLowerCase().includes('located') || 
                                   message?.toLowerCase().includes('install ffmpeg');
            
            let userMessage = t('processingFailedCheck');
            
            if (isFFmpegMissing) {
              userMessage = t('toolsNotFound');
            } else if (message && message.length < 100 && !message.includes('Error:')) {
              userMessage = message;
            }
            
            notify({ type: 'error', message: userMessage });
          }
        }
      });
    };

    setupListener();

    return () => {
      if (unlisten) unlisten();
    };
  }, [files]);

  const hasReadyFiles = files.some(f => f.status === 'ready');
  const canProcess = hasReadyFiles && outputFolder && !converting;

  // Falls back to three minutes so the Modify sliders have a sane range before
  // any file is loaded.
  const modifyDurationSeconds = (() => {
    if (files.length === 0) return 180;
    const durationText = files[0].duration;
    if (!durationText || durationText === '00:00') return 180;
    const [minutes, seconds] = durationText.split(':').map(Number);
    return minutes * 60 + seconds;
  })();

  return (
    <div className="flex h-full min-h-0 flex-col gap-4">
      {/* Everything that steers the page is on this one row: the tabs, the
        * session readout and the run control. They each had a strip of their
        * own before, which cost three rows of height on every tab and left the
        * right half of each one empty. */}
      <header className="flex shrink-0 flex-wrap items-end justify-between gap-4 px-1">
        <div>
          <h1 className="text-3xl font-semibold tracking-tight text-ink">{t('audioSuite')}</h1>
          <p className="mt-1 text-sm text-ink-soft">{t('appDesc')}</p>
        </div>

        <nav
          role="tablist"
          aria-label={t('audioSuite')}
          // A pill outside as well as in, so the two curves agree. Written out
          // rather than reusing card-sunken: that class is plain CSS declared
          // after Tailwind's utilities layer, so its 16px radius beat the
          // rounded-pill utility and the inner corner stayed the rounder of
          // the two.
          className="flex shrink-0 gap-1 rounded-pill border border-line-soft bg-sunken p-1"
        >
          {EDITOR_TABS.map((tab) => (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={editorTab === tab.id}
              onClick={() => setEditorTab(tab.id)}
              className={cn(
                'flex items-center gap-2 rounded-pill px-4 py-2 text-sm font-medium',
                'transition-colors duration-fast ease-out',
                editorTab === tab.id
                  ? 'bg-card text-ink shadow-card'
                  : 'text-ink-soft hover:text-ink',
              )}
            >
              <tab.Icon className="h-4 w-4" strokeWidth={1.9} />
              {t(tab.labelKey)}
              {tab.id === 'files' && files.length > 0 && (
                <span className="rounded-pill bg-accent-soft px-2 py-0.5 font-mono text-xs text-accent">
                  {files.length}
                </span>
              )}
            </button>
          ))}
        </nav>
      </header>


      {/* ---- Files: get audio in, decide where it goes ---- */}
      {editorTab === 'files' && (
        <div className={TAB_COLUMNS}>
          {/* Intake on the left. The drop zone takes the slack because it is
            * the target the user aims at; the output path is a one-line
            * setting and keeps its own height. */}
          <div className={SCROLL_COLUMN}>
            <section className="card flex flex-1 flex-col p-5">
              <DragDropArea onFilesAdded={handleFilesAdded} />
            </section>

            <section className="card shrink-0 p-5">
              <OutputFolderChooser path={outputFolder} onChoose={setOutputFolder} />
            </section>
          </div>

          <section className={SCROLL_CARD}>
            <FileListPanel
              files={files}
              onClearAll={handleClearAll}
              onRemoveFile={handleRemoveFile}
              onReload={handleReload}
            />
          </section>
        </div>
      )}

      {/* ---- Process: choose what to do to it ---- */}
      {editorTab === 'process' && (
        <div className={TAB_COLUMNS}>
          <section className={SCROLL_CARD}>
            <div className="scroll-area min-h-0 flex-1">
              <ModeSelector selected={mode} onChange={setMode} />
            </div>
          </section>

          <section className={SCROLL_CARD}>
            {/* Enhance and Modify are taller than the window, so this pane
              * scrolls rather than dragging the whole page down with it. */}
            <div className="scroll-area flex min-h-0 flex-1 flex-col gap-5">
              <div>
                <h2 className="text-lg font-semibold text-ink">{t('sessionOverview')}</h2>
                <p className="mt-1 text-sm text-ink-soft">
                  {mode === 'format' && t('modeFormatDesc')}
                  {mode === 'enhance' && t('modeEnhanceDesc')}
                  {mode === 'clean' && t('modeCleanDesc')}
                  {mode === 'modify' && t('modeModifyDesc')}
                </p>
              </div>

              {mode === 'format' && (
                <FormatSelector
                  formats={formatOptions}
                  selected={selectedFormat}
                  onSelect={setSelectedFormat}
                />
              )}
              {mode === 'enhance' && (
                <MasterControls
                  preset={masterPreset}
                  onPresetChange={setMasterPreset}
                  parameters={masterParams}
                  onParametersChange={setMasterParams}
                  onSmartAnalysis={handleSmartAnalysis}
                />
              )}
              {mode === 'clean' && (
                <TrimControls
                  threshold={trimThreshold}
                  onThresholdChange={setTrimThreshold}
                  minSilence={trimMinSilence}
                  onMinSilenceChange={setTrimMinSilence}
                  padding={trimPadding}
                  onPaddingChange={setTrimPadding}
                />
              )}
              {mode === 'modify' && (
                <ModifyControls
                  parameters={modifyParams}
                  onParametersChange={setModifyParams}
                  duration={modifyDurationSeconds}
                />
              )}
            </div>
          </section>
        </div>
      )}

      {/* ---- Run: watch it happen ---- */}
      {editorTab === 'run' && (
        <div className={TAB_COLUMNS}>
          <div className={SCROLL_COLUMN}>
            <section className="card shrink-0 p-5">
              <ProgressIndicator
                progress={progress}
                status={processingStatus}
                currentFile={currentFile}
              />
            </section>

            <section className="card shrink-0 p-5">
              <h2 className="mb-4 text-lg font-semibold text-ink">{t('sessionOverview')}</h2>
              {/* Stacked, not three across: this column is 340px at its widest
                * and three tiles side by side truncated their own values. */}
              <dl className="grid gap-3">
                <div className="card-sunken px-4 py-3">
                  <dt className="text-xs uppercase tracking-wide text-ink-faint">{t('files')}</dt>
                  <dd className="mt-1 font-mono text-lg text-ink">{sessionSummary.filesCount}</dd>
                </div>
                <div className="card-sunken px-4 py-3">
                  <dt className="text-xs uppercase tracking-wide text-ink-faint">{t('mode')}</dt>
                  <dd className="mt-1 font-mono text-lg text-ink">{sessionSummary.format}</dd>
                </div>
                <div className="card-sunken px-4 py-3">
                  <dt className="text-xs uppercase tracking-wide text-ink-faint">{t('status')}</dt>
                  <dd className="mt-1 font-mono text-lg text-ink">{sessionSummary.status}</dd>
                </div>
              </dl>
            </section>

          </div>

          {/* The queue is on the right here as well as on Files, so per-file
            * status stays in the same place whichever tab is open. */}
          <section className={SCROLL_CARD}>
            <FileListPanel
              files={files}
              onClearAll={handleClearAll}
              onRemoveFile={handleRemoveFile}
              onReload={handleReload}
              actions={
                <button
                  type="button"
                  onClick={handleProcess}
                  disabled={!canProcess}
                  className={cn(
                    'whitespace-nowrap rounded-pill px-6 py-2.5 text-sm font-semibold text-white',
                    'transition-opacity duration-fast ease-out',
                    'disabled:cursor-not-allowed disabled:opacity-45',
                  )}
                  style={{ background: 'var(--accent-color)' }}
                >
                  {converting ? t('processing') : t('processFiles')}
                </button>
              }
            />
          </section>
        </div>
      )}

      <ErrorModal
        isOpen={errorFiles.length > 0}
        errorFiles={errorFiles}
        onClose={() => setErrorFiles([])}
      />
    </div>
  );
}
