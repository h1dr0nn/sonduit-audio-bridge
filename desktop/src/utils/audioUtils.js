/**
 * Audio utility functions for file validation and metadata extraction
 */

const AUDIO_EXTENSIONS = ['.mp3', '.wav', '.ogg', '.flac', '.aac', '.m4a', '.wma', '.aiff', '.opus'];

/**
 * Check if a file is a valid audio file based on extension
 */
export const isAudioFile = (file) => {
  if (!file || !file.name) return false;
  const fileName = file.name.toLowerCase();
  return AUDIO_EXTENSIONS.some(ext => fileName.endsWith(ext));
};

/**
 * Format file size in bytes to human-readable string
 */
export const formatFileSize = (bytes) => {
  if (!bytes || bytes === 0) return '0 B';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
};

/**
 * Format duration in seconds to MM:SS format
 */
export const formatDuration = (seconds) => {
  if (!seconds || isNaN(seconds)) return '00:00';
  const mins = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  return `${mins.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
};

/**
 * Get audio duration from file using Web Audio API
 */
export const getAudioDuration = async (file) => {
  try {
    // Create audio context
    const audioContext = new (window.AudioContext || window.webkitAudioContext)();
    
    // Read file as ArrayBuffer
    const arrayBuffer = await file.arrayBuffer();
    
    // Decode audio data
    const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);
    
    // Close context to free resources
    audioContext.close();
    
    // Return duration in seconds
    return audioBuffer.duration;
  } catch (error) {
    console.warn('Failed to get audio duration:', error);
    return 0;
  }
};

/**
 * Extract file metadata for display
 */
export const getFileMetadata = async (file) => {
  const { size, name } = file;
  const path = file.path; // Tauri provides this on dropped files
  
  // Extract format from extension
  // Extract format from extension
  const parts = name.split('.');
  let format = parts.length > 1 ? parts.pop().toUpperCase() : 'FILE';
  
  // Sanitize format (max 4 chars, alphanumeric)
  format = format.replace(/[^A-Z0-9]/g, '').substring(0, 4);
  if (!format) format = 'FILE';
  
  // Get duration (only for real File objects, not path-only objects)
  let duration = 0;
  if (file instanceof File || file.arrayBuffer) {
    try {
      duration = await getAudioDuration(file);
    } catch (error) {
      console.warn('Failed to get duration for', name, error);
    }
  }
  
  return {
    id: crypto.randomUUID(),
    file,
    name,
    path: path || name, // Fallback if path not available
    format,
    size: formatFileSize(size),
    sizeBytes: size, // Raw bytes for filtering
    duration: formatDuration(duration),
    status: 'loading', // Start with loading status
    error: null,
    output: null
  };
};

/**
 * Get file extension from filename
 */
export const getFileExtension = (filename) => {
  const parts = filename.split('.');
  return parts.length > 1 ? parts.pop().toLowerCase() : '';
};

/**
 * Validate if output folder path is valid (non-empty)
 */
export const isValidOutputFolder = (path) => {
  return typeof path === 'string' && path.trim().length > 0;
};
