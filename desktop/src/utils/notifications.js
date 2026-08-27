/**
 * System notification helpers using Tauri notification API
 */

import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';

/**
 * Ensure notification permissions are granted
 */
const ensurePermission = async () => {
  let permissionGranted = await isPermissionGranted();
  
  if (!permissionGranted) {
    const permission = await requestPermission();
    permissionGranted = permission === 'granted';
  }
  
  return permissionGranted;
};

/**
 * Send a success notification
 */
export const notifySuccess = async (title, body) => {
  try {
    const permitted = await ensurePermission();
    if (permitted) {
      await sendNotification({
        title,
        body,
      });
    }
  } catch (error) {
    console.error('Failed to send notification:', error);
  }
};

/**
 * Send an error notification
 */
export const notifyError = async (title, body) => {
  try {
    const permitted = await ensurePermission();
    if (permitted) {
      await sendNotification({
        title,
        body,
      });
    }
  } catch (error) {
    console.error('Failed to send notification:', error);
  }
};

/**
 * Send an info notification
 */
export const notifyInfo = async (title, body) => {
  try {
    const permitted = await ensurePermission();
    if (permitted) {
      await sendNotification({
        title,
        body,
      });
    }
  } catch (error) {
    console.error('Failed to send notification:', error);
  }
};
