/**
 * Thin wrapper around Tauri command invocation with loading and error state.
 */

import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

export const useTauriCommand = (commandName) => {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);

  const execute = async (args = {}) => {
    setLoading(true);
    setError(null);

    try {
      return await invoke(commandName, args);
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
