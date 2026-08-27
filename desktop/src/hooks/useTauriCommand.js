/**
 * Custom React hook for calling Tauri commands with loading/error states
 */

import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

/**
 * Hook for convert_audio Tauri command
 */
export const useConvertAudio = () => {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);

  const convert = async (payload) => {
    setLoading(true);
    setError(null);
    
    try {
      const result = await invoke('convert_audio', { payload });
      return result;
    } catch (e) {
      const errorMessage = typeof e === 'string' ? e : e.message || 'Unknown error';
      setError(errorMessage);
      throw new Error(errorMessage);
    } finally {
      setLoading(false);
    }
  };

  return { convert, loading, error };
};

/**
 * Generic hook for any Tauri command
 */
export const useTauriCommand = (commandName) => {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);

  const execute = async (args = {}) => {
    setLoading(true);
    setError(null);
    
    try {
      const result = await invoke(commandName, args);
      return result;
    } catch (e) {
      const errorMessage = typeof e === 'string' ? e : e.message || 'Unknown error';
      setError(errorMessage);
      throw new Error(errorMessage);
    } finally {
      setLoading(false);
    }
  };

  return { execute, loading, error };
};
