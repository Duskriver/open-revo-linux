import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { execSync } from 'child_process';
import app from './src-tauri/tauri.conf.json' with { type: 'json' };

const getGitCommitHash = (): string => {
  try {
    return execSync('git rev-parse --short HEAD', { encoding: 'utf-8' }).trim();
  } catch {
    return process.env.VITE_GIT_COMMIT || '';
  }
};

const gitHash = getGitCommitHash();
const appVersion = gitHash ? `${app.version}-${gitHash}` : app.version;

export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
    __RAW_VERSION__: JSON.stringify(app.version),
    __GIT_COMMIT__: JSON.stringify(gitHash),
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ['**/src-tauri/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: 'safari13',
    minify: !process.env.TAURI_DEBUG ? 'oxc' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
  },
});
