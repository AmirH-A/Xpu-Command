import { defineConfig } from 'vite';
import { resolve } from 'node:path';

export default defineConfig({
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: '127.0.0.1' },
  build: {
    target: 'es2022',
    sourcemap: false,
    rollupOptions: { input: { main: resolve(__dirname, 'index.html'), overlay: resolve(__dirname, 'overlay.html'), frame: resolve(__dirname, 'frame.html') } },
  },
});
