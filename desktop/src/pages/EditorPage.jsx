import React, { useState, useEffect, useCallback, useRef } from 'react';
import { FiSettings } from 'react-icons/fi';
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
import { ToastMessage } from '../components/editor/ToastMessage';
import { ModeSelector } from '../components/editor/ModeSelector';
import { MasterControls, PRESETS } from '../components/editor/MasterControls';
import { TrimControls } from '../components/editor/TrimControls';
import { ModifyControls } from '../components/editor/ModifyControls';
import { ErrorModal } from '../components/editor/ErrorModal';
import { useTheme } from '../hooks/useTheme';
import { useConvertAudio } from '../hooks/useTauriCommand';
import { designTokens } from '../utils/theme';
import { themeClasses } from '../utils/themeColors';
import { getFileMetadata, formatFileSize, formatDuration, getAudioDuration } from '../utils/audioUtils';
import { notifySuccess, notifyError } from '../utils/notifications';

import { useSettingsContext } from '../context/SettingsContext';
import { useTranslation } from '../i18n';

const formatOptions = ['AAC', 'MP3', 'WAV', 'FLAC', 'OGG', 'M4A'];

export function EditorPage({ onOpenSettings }) {
  // Harmonix lifted this state into App so it survived navigating to the
  // settings screen. Sonduit keeps every screen mounted-on-demand instead, so
  // the editor owns its own queue.
  const [files, setFiles] = useState([]);
  const [outputFolder, setOutputFolder] = useState('');

  const { theme } = useTheme();
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
  const [toast, setToast] = useState(null);

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
          setToast({ 
            type: 'info', 
            message: `${duplicateCount} ${t('duplicateSkipped')}` 
          });
        }
        return;
      }

      const duplicateCount = immediateFiles.length - newUniqueFiles.length;
      if (duplicateCount > 0) {
        setToast({ 
          type: 'info', 
          message: `${duplicateCount} ${t('duplicateSkipped')}` 
        });
      }

      // Add files to UI immediately
      setFiles(prev => [...prev, ...newUniqueFiles]);
      setToast({ type: 'info', message: t('addingFiles', { count: newUniqueFiles.length }) });

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
      setToast({ type: 'error', message: t('failedToAdd') });
    }
  }, [outputFolder, settings, setOutputFolder]);

  // Listen for global events (Dock drop, Menu commands)
  useEffect(() => {
    let unlistenFileOpened;
    let unlistenRequestOpen;

    const setupListeners = async () => {
      // Handle Dock drag & drop
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

      // Handle Menu "Open File..."
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
  }, [handleFilesAdded]); // Dependencies might need review, but handleFilesAdded uses state setters so it's stable? No, handleFilesAdded depends on 'files' state for dedup.
  // Actually handleFilesAdded is a const function inside component, so it changes every render if it uses state.
  // But 'files' is in dependency of useEffect, so it re-subscribes. That's fine.

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
      setToast({ type: 'info', message: t('pleaseAddAnalyze') });
      return;
    }

    try {
      setToast({ type: 'info', message: t('analyzing') });
      
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

        setToast({ 
          type: 'success', 
          message: t('detectedContent', { content: suggestion }) 
        });
      } else {
        throw new Error(result.message || t('analysisFailed'));
      }

    } catch (error) {
      console.error('Analysis error:', error);
      setToast({ type: 'error', message: t('smartAnalysisFailed') });
    }
  };

  // Handle process button
  const handleProcess = async () => {
    if (files.length === 0) {
      setToast({ type: 'error', message: t('pleaseAddProcess') });
      return;
    }

    if (!outputFolder) {
      setToast({ type: 'error', message: t('pleaseSelectOutput') });
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
      
      setToast({ type: 'error', message: userMessage });
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

            setToast({ type: 'success', message: message || t('processingComplete') });
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
            
            setToast({ type: 'error', message: userMessage });
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

  return (
    <div
      className={`h-screen overflow-y-auto bg-gradient-to-br ${themeClasses.pageBackground} text-slate-900 transition duration-smooth dark:text-slate-100`}
      style={{ fontFamily: designTokens.font }}
    >
      <div className="mx-auto flex min-h-full w-full max-w-full flex-col gap-4 overflow-x-hidden p-4 lg:gap-6 lg:p-6">
        {/* Header */}
        <header className={`flex flex-col gap-4 rounded-card border ${themeClasses.card} p-4 shadow-soft backdrop-blur-[32px] transition duration-smooth lg:p-5`}>
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div>
              <p className="text-xs uppercase tracking-[0.2em] text-slate-500 dark:text-slate-400">Sonduit</p>
              <h1 className="text-2xl font-semibold text-slate-900 dark:text-white">{t('audioSuite')}</h1>
              <p className="text-sm text-slate-600 dark:text-slate-300">{t('appDesc')}</p>
            </div>
            <button
              onClick={onOpenSettings}
              className={`flex h-12 w-12 items-center justify-center rounded-full border ${themeClasses.button} shadow-md transition duration-smooth hover:-translate-y-[1px] hover:shadow-lg`}
              aria-label="Settings"
            >
              <FiSettings className="h-6 w-6 text-slate-700 dark:text-slate-300" />
            </button>
          </div>
        </header>

        <div className="flex flex-1 gap-6">
          {/* Sidebar */}
          <aside className={`glass-surface scrollbar-hide hidden min-w-[280px] max-w-[380px] flex-col rounded-card border ${themeClasses.card} p-5 shadow-soft transition duration-smooth lg:flex`}>
            <div className="relative flex flex-1 flex-col space-y-6">
              {/* Mode Selector in Sidebar */}
              <ModeSelector selected={mode} onChange={setMode} />
              
              <div className="space-y-2">
                <p className="text-xs uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">{t('workspace')}</p>
                <h2 className="text-xl font-semibold text-slate-900 dark:text-white">{t('sessionOverview')}</h2>
                <p className="text-sm text-slate-600 dark:text-slate-300">
                  {mode === 'format' && t('modeFormatDesc')}
                  {mode === 'enhance' && t('modeEnhanceDesc')}
                  {mode === 'clean' && t('modeCleanDesc')}
                  {mode === 'modify' && t('modeModifyDesc')}
                </p>
              </div>
              
              <div className={`space-y-3 rounded-2xl border ${themeClasses.surface} p-4`}>
                <div className="flex items-center justify-between text-sm font-semibold text-slate-800 dark:text-slate-100">
                  <span>{t('files')}</span>
                  <span>{sessionSummary.filesCount}</span>
                </div>
                <div className="flex items-center justify-between text-sm font-semibold text-slate-800 dark:text-slate-100">
                  <span>{t('mode')}</span>
                  <span>{sessionSummary.format}</span>
                </div>
                <div className="flex items-center justify-between text-sm font-semibold text-slate-800 dark:text-slate-100">
                  <span>{t('status')}</span>
                  <span>{sessionSummary.status}</span>
                </div>
              </div>
            </div>
          </aside>

          {/* Main Content */}
          <main className="scrollbar-hide flex flex-1 flex-col gap-4 lg:gap-6">
            {/* Drag & Drop + Controls */}
            <section className={`flex flex-col gap-4 rounded-card border ${themeClasses.card} p-5 shadow-soft backdrop-blur-[32px] transition duration-smooth min-[1320px]:flex-row`}>
              <div className="min-[1320px]:min-w-[440px] min-[1320px]:max-w-[600px] min-[1320px]:flex-1" style={{flex: '1 1 520px'}}>
                <DragDropArea onFilesAdded={handleFilesAdded} />
              </div>
              <div className={`flex h-full min-w-0 flex-1 flex-col justify-between gap-6 rounded-card border ${themeClasses.surface} p-4 min-[1320px]:min-w-[440px]`}>
                {mode === 'format' && (
                  <FormatSelector formats={formatOptions} selected={selectedFormat} onSelect={setSelectedFormat} />
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
                    duration={(() => {
                      if (files.length === 0) return 180; // Default 3 mins if no files
                      const durationStr = files[0].duration;
                      if (!durationStr || durationStr === '00:00') return 180;
                      const [mins, secs] = durationStr.split(':').map(Number);
                      return (mins * 60) + secs;
                    })()}
                  />
                )}
                <OutputFolderChooser path={outputFolder} onChoose={setOutputFolder} />
              </div>
            </section>

            {/* File List + Progress */}
            <section className={`flex flex-1 flex-col gap-4 rounded-card border ${themeClasses.card} p-5 shadow-soft backdrop-blur-[32px] transition duration-smooth min-[1320px]:flex-row`}>
              <div className="flex h-full flex-col overflow-hidden min-[1320px]:min-w-[440px] min-[1320px]:max-w-[600px] min-[1320px]:flex-1" style={{flex: '1 1 520px'}}>
                <FileListPanel 
                  files={files} 
                  onClearAll={handleClearAll} 
                  onRemoveFile={handleRemoveFile}
                  onReload={handleReload}
                />
              </div>
              <div className="flex min-w-0 flex-1 flex-col gap-4 min-[1320px]:min-w-[440px]">
                <ProgressIndicator 
                  progress={progress} 
                  status={processingStatus}
                  currentFile={currentFile}
                />
                <button
                  onClick={handleProcess}
                  disabled={!canProcess}
                  className="rounded-full bg-accent px-6 py-3 text-sm font-semibold text-white shadow-md transition duration-smooth hover:-translate-y-[1px] hover:shadow-lg disabled:opacity-50 disabled:cursor-not-allowed disabled:hover:translate-y-0"
                >
                  {converting ? t('processing') : t('processFiles')}
                </button>
                {toast && (
                  <ToastMessage
                    title={toast.type === 'success' ? t('success') : toast.type === 'error' ? t('error') : t('info')}
                    message={toast.message}
                    tone={toast.type}
                  />
                )}
              </div>
            </section>
          </main>
        </div>
      </div>

      {/* Error Modal */}
      <ErrorModal 
        isOpen={errorFiles.length > 0}
        errorFiles={errorFiles}
        onClose={() => setErrorFiles([])}
      />
    </div>
  );
}
