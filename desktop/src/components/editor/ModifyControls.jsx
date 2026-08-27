import React from 'react';
import { cn } from '../../utils/cn';
import { formatDuration } from '../../utils/audioUtils';
import { useTranslation } from '../../i18n';

export function ModifyControls({ parameters, onParametersChange, duration = 0 }) {
  const { t } = useTranslation();
  // parameters: { speed, pitch, cutStart, cutEnd, isCutEnabled }

  const handleChange = (key, value) => {
    onParametersChange({ ...parameters, [key]: value });
  };

  // Calculate time values based on percentage and duration
  const startTime = (parameters.cutStart / 100) * duration;
  const endTime = (parameters.cutEnd / 100) * duration;

  return (
    <div className="space-y-6">
      {/* Speed Control */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <label className="text-xs uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">
            {t('playbackSpeed')}
          </label>
          <span className="text-xs font-semibold text-slate-700 dark:text-slate-200">
            {parameters.speed.toFixed(2)}x
          </span>
        </div>
        <input
          type="range"
          min="0.5"
          max="2.0"
          step="0.05"
          value={parameters.speed}
          onChange={(e) => handleChange('speed', parseFloat(e.target.value))}
          className="h-2 w-full appearance-none rounded-full bg-white/50 dark:bg-white/10"
        />
        <div className="flex justify-between text-[10px] text-slate-400">
          <span>0.5x</span>
          <span>1.0x</span>
          <span>2.0x</span>
        </div>
      </div>

      {/* Pitch Control */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <label className="text-xs uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">
            {t('pitchShift')}
          </label>
          <span className="text-xs font-semibold text-slate-700 dark:text-slate-200">
            {parameters.pitch > 0 ? '+' : ''}{parameters.pitch} {t('semitones')}
          </span>
        </div>
        <input
          type="range"
          min="-12"
          max="12"
          step="1"
          value={parameters.pitch}
          onChange={(e) => handleChange('pitch', parseInt(e.target.value))}
          className="h-2 w-full appearance-none rounded-full bg-white/50 dark:bg-white/10"
        />
        <div className="flex justify-between text-[10px] text-slate-400">
          <span>-12</span>
          <span>0</span>
          <span>+12</span>
        </div>
      </div>

      {/* Cut Audio */}
      <div className="space-y-2">
        <div className="flex items-center justify-between">
          <label className="text-xs uppercase tracking-[0.18em] text-slate-500 dark:text-slate-400">
            {t('cutAudio')}
          </label>
          <span className="text-xs font-semibold text-slate-700 dark:text-slate-200">
            {formatDuration(endTime - startTime)}
          </span>
        </div>
        
        {/* Dual Slider */}
        <div className="relative h-10 pt-4">
          {/* Track Background (Unselected Area) */}
          <div className="absolute top-1/2 h-2 w-full -translate-y-1/2 rounded-full bg-slate-200/50 dark:bg-slate-700/50" />
          
          {/* Selected Range (Matches Pitch Shift Track) */}
          <div
            className="absolute top-1/2 h-2 -translate-y-1/2 rounded-full bg-white/50 dark:bg-white/10"
            style={{
              left: `${parameters.cutStart}%`,
              right: `${100 - parameters.cutEnd}%`
            }}
          />

          {/* Start Thumb Input */}
          <input
            type="range"
            min="0"
            max="100"
            step="1"
            value={parameters.cutStart}
            onChange={(e) => {
              const val = Math.min(parseInt(e.target.value), parameters.cutEnd - 1);
              handleChange('cutStart', val);
            }}
            className="pointer-events-none absolute top-1/2 -mt-2 h-4 w-full appearance-none bg-transparent [&::-webkit-slider-thumb]:pointer-events-auto [&::-webkit-slider-thumb]:h-4 [&::-webkit-slider-thumb]:w-4 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-white [&::-webkit-slider-thumb]:shadow-md"
          />

          {/* End Thumb Input */}
          <input
            type="range"
            min="0"
            max="100"
            step="1"
            value={parameters.cutEnd}
            onChange={(e) => {
              const val = Math.max(parseInt(e.target.value), parameters.cutStart + 1);
              handleChange('cutEnd', val);
            }}
            className="pointer-events-none absolute top-1/2 -mt-2 h-4 w-full appearance-none bg-transparent [&::-webkit-slider-thumb]:pointer-events-auto [&::-webkit-slider-thumb]:h-4 [&::-webkit-slider-thumb]:w-4 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:bg-white [&::-webkit-slider-thumb]:shadow-md"
          />
        </div>

        <div className="flex justify-between text-[10px] text-slate-400">
          <span>{formatDuration(startTime)}</span>
          <span>{formatDuration(endTime)}</span>
        </div>
      </div>
    </div>
  );
}
