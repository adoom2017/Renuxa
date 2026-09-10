import tailwindcss from '@tailwindcss/postcss';
import vinext from 'vinext';
import { defineConfig } from 'vite';

// macOS Seatbelt blocks FSEvents, so sandbox previews use polling for HMR.
const usePolling = process.env.CODEX_SANDBOX === 'seatbelt';

export default defineConfig({
  css: { postcss: { plugins: [tailwindcss()] } },
  server: usePolling ? { watch: { useFsEvents: false, usePolling: true } } : undefined,
  plugins: [vinext()],
});
