import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/postcss';
import { defineConfig } from 'vite';

export default defineConfig({
  root: 'desktop',
  // The desktop entrypoint shares the app's root public assets (including the logo).
  publicDir: '../public',
  plugins: [react()],
  css: { postcss: { plugins: [tailwindcss()] } },
  build: {
    outDir: '../dist-desktop',
    emptyOutDir: true,
  },
});
