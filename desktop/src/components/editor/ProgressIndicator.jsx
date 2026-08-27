import React, { useEffect, useState, useRef } from 'react';
import { FiCheck } from 'react-icons/fi';
import { useTranslation } from '../../i18n';

// Animated number component for smooth transitions
function AnimatedNumber({ value }) {
  const [displayValue, setDisplayValue] = useState(value);
  const prevValueRef = useRef(value);
  
  useEffect(() => {
    // Don't animate if value hasn't changed or if it's just initial mount
    if (prevValueRef.current === value) {
      return;
    }
    
    // Don't animate small changes (< 1%)
    if (Math.abs(value - prevValueRef.current) < 1) {
      setDisplayValue(value);
      prevValueRef.current = value;
      return;
    }
    
    prevValueRef.current = value;
    
    const duration = 300; // ms
    const steps = 20;
    const stepValue = (value - displayValue) / steps;
    const stepDuration = duration / steps;
    
    let currentStep = 0;
    const interval = setInterval(() => {
      currentStep++;
      if (currentStep >= steps) {
        setDisplayValue(value);
        clearInterval(interval);
      } else {
        setDisplayValue(prev => prev + stepValue);
      }
    }, stepDuration);
    
    return () => clearInterval(interval);
  }, [value, displayValue]);
  
  return <span>{Math.round(displayValue)}</span>;
}

export function ProgressIndicator({ progress, status, currentFile }) {
  const { t } = useTranslation();
  
  // Calculate progress from status text if it contains "Processing X/Y"
  let progressPercent = progress || 0;
  
  // If status is "Complete", force 100%
  if (status === 'Complete') {
    progressPercent = 100;
  } else if (status && status.includes('Processing')) {
    const match = status.match(/Processing (\d+)\/(\d+)/);
    if (match) {
      const current = parseInt(match[1]);
      const total = parseInt(match[2]);
      if (total > 0) {
        progressPercent = (current / total) * 100;
      }
    }
  }
  
  progressPercent = Math.min(100, Math.max(0, progressPercent));
  const isProcessing = progressPercent > 0 && progressPercent < 100;
  const isComplete = progressPercent === 100;
  
  // Calculate gradient color based on progress
  const getProgressGradient = () => {
    if (isComplete) {
      return 'linear-gradient(90deg, #10b981 0%, #059669 100%)'; // Green
    } else if (isProcessing) {
      // Blue to purple gradient
      return `linear-gradient(90deg, #3b82f6 0%, #8b5cf6 ${progressPercent}%)`;
    }
    return '#94a3b8'; // Gray for idle
  };

  return (
    <div className="space-y-3 rounded-card border border-slate-200 bg-white p-5 shadow-soft transition duration-smooth dark:border-white/10 dark:bg-white/10">
      <div className="flex items-center justify-between">
        <div>
          <p className="text-xs uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">{t('progressLabel')}</p>
          <h3 className="text-lg font-semibold text-slate-900 dark:text-slate-100">
            {status || t('statusIdle')}
          </h3>
        </div>
        <span 
          className={`flex items-center gap-2 rounded-full px-4 py-2 text-sm font-semibold shadow-md transition-all duration-300 ${
            isComplete 
              ? 'bg-emerald-500 text-white scale-110' 
              : 'bg-accent text-white'
          }`}
        >
          {isComplete && <FiCheck className="h-4 w-4" />}
          <span className="tabular-nums">
            {Math.round(progressPercent)}%
          </span>
        </span>
      </div>

      {currentFile && (
        <div className="min-w-0">
          <p className="truncate text-xs text-slate-600 dark:text-slate-300" title={currentFile}>
            {t('processingFile')} {currentFile.split(/[/\\]/).pop()}
          </p>
        </div>
      )}

      {/* Enhanced progress bar */}
      <div className="relative h-3 overflow-hidden rounded-full bg-slate-200/50 shadow-inner dark:bg-slate-700/30">
        <div
          className={`h-full rounded-full transition-all duration-500 ease-out ${
            isProcessing ? 'animate-pulse-glow' : ''
          }`}
          style={{
            width: `${progressPercent}%`,
            background: getProgressGradient(),
            boxShadow: isProcessing 
              ? '0 0 15px rgba(139, 92, 246, 0.5)' 
              : isComplete 
              ? '0 0 15px rgba(16, 185, 129, 0.6)'
              : 'none',
          }}
        />
        
        {/* Shimmer effect for processing */}
        {isProcessing && (
          <div className="absolute inset-0 overflow-hidden">
            <div className="h-full w-1/3 animate-shimmer bg-gradient-to-r from-transparent via-white/30 to-transparent" />
          </div>
        )}
      </div>

      <p className={`text-xs transition-colors duration-300 ${
        isComplete 
          ? 'font-semibold text-emerald-600 dark:text-emerald-400' 
          : 'text-slate-500 dark:text-slate-400'
      }`}>
        {progressPercent === 0 && t('readyToProcess')}
        {isProcessing && t('processingFiles')}
        {isComplete && t('complete')}
      </p>
    </div>
  );
}
