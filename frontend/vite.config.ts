import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import { viteObfuscateFile } from 'vite-plugin-obfuscator';
import path from 'path';
import { execSync } from 'child_process';

function getGitHash(): string {
  try {
    return execSync('git rev-parse --short HEAD').toString().trim();
  } catch {
    return 'unknown';
  }
}

export default defineConfig({
  base: '/ctrl-modem/',
  define: {
    __APP_VERSION__: JSON.stringify(process.env.npm_package_version || '0.0.0'),
    __BUILD_TIME__: JSON.stringify(new Date().toISOString()),
    __GIT_HASH__: JSON.stringify(getGitHash()),
  },
  plugins: [
    react(),
    viteObfuscateFile({
      options: {
        // String protection — encodes string literals into array lookups
        stringArray: true,
        stringArrayRotate: true,
        stringArrayShuffle: true,
        stringArrayThreshold: 0.75,
        // Identifier mangling
        renameGlobals: false,
        identifierNamesGenerator: 'hexadecimal',
        // Keep overhead low for embedded router
        controlFlowFlattening: false,
        deadCodeInjection: false,
        selfDefending: false,
        debugProtection: false,
      },
    }),
  ],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/ctrl-modem/api': {
        target: 'http://192.168.1.1:8080',
        changeOrigin: true,
        ws: true,
      },
    },
  },
  // Strip console/debugger statements from production builds
  esbuild: {
    drop: ['console', 'debugger'],
  },
  build: {
    // Never generate source maps in production
    sourcemap: false,
    rollupOptions: {
      output: {
        // Hash-only filenames — no chunk names that hint at tech stack
        chunkFileNames: 'assets/[hash].js',
        entryFileNames: 'assets/[hash].js',
        assetFileNames: 'assets/[hash][extname]',
        manualChunks: {
          'v': ['react', 'react-dom'],
          'q': ['@tanstack/react-query'],
          'c': ['recharts'],
        },
      },
    },
  },
});
