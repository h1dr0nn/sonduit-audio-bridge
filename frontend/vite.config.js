import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Version has a single owner. The desktop shell never declares its own; it
// reads whatever the Tauri config was generated with.
const tauriConfigPath = fileURLToPath(new URL('../src-tauri/tauri.conf.json', import.meta.url));
const { version } = JSON.parse(readFileSync(tauriConfigPath, 'utf8'));

export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(version),
  },
  server: {
    port: 5173,
  },
});
