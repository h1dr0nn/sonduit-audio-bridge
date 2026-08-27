/**
 * Metadata Formatting Utilities
 * Format audio metadata for user-friendly display
 */

/**
 * Format bitrate to human-readable string
 */
export const formatBitrate = (bitrate) => {
  if (!bitrate || bitrate === 0) return '--';
  const kbps = Math.round(bitrate / 1000);
  return `${kbps} kbps`;
};

/**
 * Format channel count to descriptive string
 */
export const formatChannels = (channels) => {
  if (!channels || channels === 0) return '--';
  if (channels === 1) return 'Mono';
  if (channels === 2) return 'Stereo';
  if (channels === 6) return '5.1';
  if (channels === 8) return '7.1';
  return `${channels}ch`;
};

/**
 * Format sample rate to kHz
 */
export const formatSampleRate = (sampleRate) => {
  if (!sampleRate || sampleRate === 0) return '--';
  const khz = sampleRate / 1000;
  return `${khz} kHz`;
};

/**
 * Format codec name for display
 */
export const formatCodec = (codec) => {
  if (!codec || codec === 'unknown') return '--';
  
  // Map common codec names to friendly versions
  const codecMap = {
    'mp3': 'MP3',
    'aac': 'AAC',
    'flac': 'FLAC',
    'alac': 'ALAC',
    'opus': 'Opus',
    'vorbis': 'Vorbis',
    'wav': 'WAV',
    'pcm_s16le': 'PCM',
    'pcm_s24le': 'PCM 24-bit'
  };
  
  const normalized = codec.toLowerCase();
  return codecMap[normalized] || codec.toUpperCase();
};

/**
 * Get audio quality rating based on bitrate and sample rate
 */
export const getQualityRating = (bitrate, sampleRate) => {
  if (!bitrate || bitrate === 0) return null;
  
  const kbps = bitrate / 1000;
  const khz = sampleRate ? sampleRate / 1000 : 44.1;
  
  // Lossless formats
  if (kbps > 800 || khz >= 96) {
    return { level: 'Lossless', color: 'emerald' };
  }
  
  // High quality
  if (kbps >= 256 && khz >= 44.1) {
    return { level: 'High', color: 'blue' };
  }
  
  // Medium quality
  if (kbps >= 128) {
    return { level: 'Medium', color: 'amber' };
  }
  
  // Low quality
  return { level: 'Low', color: 'red' };
};
