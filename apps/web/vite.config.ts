import { readdirSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import tailwindcss from '@tailwindcss/vite';

const backend = `http://127.0.0.1:${process.env.HUBNESS_PORT ?? 8787}`;

/**
 * virtual:file-icons — Material Icon Theme icons (MIT): the mapping of file/folder names
 * to icons (from the package manifest) and the URLs of the SVGs themselves. In dev — direct
 * file paths (/@fs/…), in the build — assets. Not 1200 module imports: otherwise
 * the browser hits the connection limit and API requests stall. Open-folder icons
 * are not stored: they are always `<icon>-open`. The module is loaded lazily.
 */
function fileIcons(): Plugin {
  const id = 'virtual:file-icons';
  let dev = false;
  return {
    name: 'hubness-file-icons',
    configResolved: (c) => void (dev = c.command === 'serve'),
    resolveId: (s) => (s === id ? '\0' + id : undefined),
    load(s) {
      if (s !== '\0' + id) return;
      const require = createRequire(import.meta.url);
      const pkg = dirname(require.resolve('material-icon-theme/package.json'));
      const { generateManifest } = require('material-icon-theme') as typeof import('material-icon-theme');
      const m = generateManifest();
      const manifest = {
        ext: m.fileExtensions ?? {},
        names: m.fileNames ?? {},
        folders: m.folderNames ?? {},
        light: { ext: m.light?.fileExtensions ?? {}, names: m.light?.fileNames ?? {}, folders: m.light?.folderNames ?? {} },
      };
      const dir = join(pkg, 'icons');
      const entries = readdirSync(dir)
        .filter((f) => f.endsWith('.svg'))
        .map((f) => {
          const name = JSON.stringify(f.slice(0, -4));
          if (dev) return `${name}: ${JSON.stringify('/@fs' + join(dir, f))}`;
          const ref = this.emitFile({ type: 'asset', name: `icons/${f}`, source: readFileSync(join(dir, f)) });
          return `${name}: import.meta.ROLLUP_FILE_URL_${ref}`;
        });
      return `export const manifest = ${JSON.stringify(manifest)};\nexport const urls = {${entries.join(',')}};`;
    },
  };
}

export default defineConfig({
  plugins: [react(), tailwindcss(), fileIcons()],
  server: {
    port: Number(process.env.HUBNESS_WEB_PORT ?? 5173),
    host: process.env.HUBNESS_WEB_HOST ?? '127.0.0.1',
    proxy: { '/api': { target: backend, changeOrigin: true } },
  },
});
