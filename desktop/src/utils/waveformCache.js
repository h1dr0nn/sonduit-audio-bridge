/**
 * Waveform Cache Utility
 * Caches waveform peaks data in localStorage to avoid regenerating waveforms
 */

const CACHE_PREFIX = 'waveform_';
const CACHE_DURATION_MS = 7 * 24 * 60 * 60 * 1000; // 7 days
const MAX_CACHE_ENTRIES = 100; // Limit cache size

/**
 * Generate a unique cache key from file metadata
 */
export const generateFileKey = (file) => {
  try {
    const path = file.path || file.name;
    const size = file.size || file.sizeBytes || 0;
    const modified = file.lastModified || 0;
    
    // Create a simple hash-like key (encode to handle Unicode)
    const rawKey = `${path}_${size}_${modified}`;
    const encoded = encodeURIComponent(rawKey).replace(/[^a-zA-Z0-9]/g, '').substring(0, 80);
    return `${CACHE_PREFIX}${encoded}`;
  } catch (e) {
    console.warn('[WaveformCache] Failed to generate key:', e);
    return null;
  }
};

/**
 * Downsample peaks array to reduce storage size
 * Only keep essential points for visual representation
 */
const downsamplePeaks = (peaks, targetLength = 1000) => {
  if (peaks.length <= targetLength) return peaks;
  
  const blockSize = Math.floor(peaks.length / targetLength);
  const downsampled = [];
  
  for (let i = 0; i < targetLength; i++) {
    const start = i * blockSize;
    const end = start + blockSize;
    const slice = peaks.slice(start, end);
    // Find max absolute value and the element in one pass
    let max = 0;
    let maxElement = slice[0];
    for (let j = 0; j < slice.length; j++) {
      const abs = Math.abs(slice[j]);
      if (abs > max) {
        max = abs;
        maxElement = slice[j];
      }
    }
    downsampled.push(maxElement);
  }
  
  return downsampled;
};

/**
 * Save waveform peaks to localStorage
 */
export const savePeaks = (file, peaks) => {
  try {
    const key = generateFileKey(file);
    if (!key) return false;

    // Convert peaks to array if needed and downsample
    const peaksArray = Array.isArray(peaks) ? peaks : Array.from(peaks);
    const downsampledPeaks = downsamplePeaks(peaksArray, 1000); // Only keep 1000 samples
    
    const data = {
      peaks: downsampledPeaks,
      timestamp: Date.now(),
      fileName: file.name || 'unknown'
    };

    localStorage.setItem(key, JSON.stringify(data));
    console.log('[WaveformCache] Saved peaks for:', file.name, `(${downsampledPeaks.length} samples)`);
    return true;
  } catch (e) {
    // Quota exceeded - try clearing old caches
    if (e.name === 'QuotaExceededError') {
      console.warn('[WaveformCache] Quota exceeded, clearing old entries');
      clearOldCaches();
      
      // Try again after clearing
      try {
        const key = generateFileKey(file);
        const peaksArray = Array.isArray(peaks) ? peaks : Array.from(peaks);
        const downsampledPeaks = downsamplePeaks(peaksArray, 1000);
        
        const data = {
          peaks: downsampledPeaks,
          timestamp: Date.now(),
          fileName: file.name || 'unknown'
        };
        localStorage.setItem(key, JSON.stringify(data));
        console.log('[WaveformCache] Saved peaks after cleanup:', file.name);
        return true;
      } catch (retryError) {
        console.error('[WaveformCache] Failed to save even after cleanup:', retryError);
        return false;
      }
    }
    
    console.error('[WaveformCache] Failed to save peaks:', e);
    return false;
  }
};

/**
 * Load waveform peaks from localStorage
 */
export const loadPeaks = (file) => {
  try {
    const key = generateFileKey(file);
    if (!key) return null;

    const stored = localStorage.getItem(key);
    if (!stored) {
      console.log('[WaveformCache] No cache found for:', file.name);
      return null;
    }

    const data = JSON.parse(stored);
    
    // Check if cache is still valid
    const age = Date.now() - data.timestamp;
    if (age > CACHE_DURATION_MS) {
      console.log('[WaveformCache] Cache expired for:', file.name);
      localStorage.removeItem(key);
      return null;
    }

    console.log('[WaveformCache] Cache hit for:', file.name, `(${Math.round(age / 1000 / 60)} mins old)`);
    return data.peaks;
  } catch (e) {
    console.error('[WaveformCache] Failed to load peaks:', e);
    return null;
  }
};

/**
 * Clear old cache entries to free up space
 */
export const clearOldCaches = () => {
  try {
    const keys = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && key.startsWith(CACHE_PREFIX)) {
        keys.push(key);
      }
    }

    // Sort by timestamp (oldest first)
    const entries = keys.map(key => {
      try {
        const data = JSON.parse(localStorage.getItem(key));
        return { key, timestamp: data.timestamp || 0 };
      } catch (e) {
        return { key, timestamp: 0 };
      }
    }).sort((a, b) => a.timestamp - b.timestamp);

    // Remove oldest entries if we have too many
    if (entries.length > MAX_CACHE_ENTRIES) {
      const toRemove = entries.slice(0, entries.length - MAX_CACHE_ENTRIES);
      toRemove.forEach(({ key }) => {
        localStorage.removeItem(key);
      });
      console.log('[WaveformCache] Cleared', toRemove.length, 'old entries');
    }

    // Also remove expired entries
    const now = Date.now();
    entries.forEach(({ key }) => {
      try {
        const data = JSON.parse(localStorage.getItem(key));
        if (now - data.timestamp > CACHE_DURATION_MS) {
          localStorage.removeItem(key);
        }
      } catch (e) {
        // Invalid entry, remove it
        localStorage.removeItem(key);
      }
    });
  } catch (e) {
    console.error('[WaveformCache] Failed to clear old caches:', e);
  }
};

/**
 * Clear all waveform caches
 */
export const clearAllCaches = () => {
  try {
    const keys = [];
    for (let i = 0; i < localStorage.length; i++) {
      const key = localStorage.key(i);
      if (key && key.startsWith(CACHE_PREFIX)) {
        keys.push(key);
      }
    }

    keys.forEach(key => localStorage.removeItem(key));
    console.log('[WaveformCache] Cleared all caches:', keys.length, 'entries');
  } catch (e) {
    console.error('[WaveformCache] Failed to clear all caches:', e);
  }
};
