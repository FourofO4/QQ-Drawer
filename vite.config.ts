import { defineConfig } from 'vitest/config';
import solid from 'vite-plugin-solid';

// Tauri 期望前端固定跑在一个端口上，且不要在端口被占用时静默换端口。
export default defineConfig({
  plugins: [solid()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: { ignored: ['**/src-tauri/**'] },
  },
  build: {
    // WebView2 就是 Edge，可以放心用现代语法；省掉 polyfill 体积。
    target: 'chrome110',
    // 关闭 sourcemap 以压缩安装包体积（PERF-01）。
    sourcemap: false,
    minify: 'esbuild',
    cssMinify: true,
    chunkSizeWarningLimit: 600,
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts', 'tests/**/*.test.ts'],
    reporters: 'default',
  },
});
