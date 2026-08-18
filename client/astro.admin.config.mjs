import { defineConfig } from 'astro/config';
import { viteSingleFile } from 'vite-plugin-singlefile';

export default defineConfig({
  srcDir: './src/admin',
  outDir: './dist-admin',
  output: 'static',
  vite: {
    plugins: [viteSingleFile()],
    build: {
      cssCodeSplit: false,
      assetsInlineLimit: 100000000,
    },
  },
});
