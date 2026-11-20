import { useEffect, useState } from 'react';
import { File, Folder, FolderOpen } from 'lucide-react';
import { isDark, onThemeChange } from '../theme';

/**
 * File or folder icon by name, from Material Icon Theme (MIT), as in
 * VS Code: exact name (package.json, Dockerfile, .gitignore), then compound
 * extension (d.ts, test.tsx, tar.gz), then the plain one; folders by name (src,
 * components, .github). Data is loaded lazily; until then a plain icon is shown.
 */

type Data = typeof import('../lib/fileIconsData');
let data: Data | null = null;
let loading: Promise<Data> | null = null;
const waiters = new Set<() => void>();

function load() {
  if (!loading)
    loading = import('../lib/fileIconsData').then((d) => {
      data = d;
      waiters.forEach((fn) => fn());
      return d;
    });
  return loading;
}

function useIconData() {
  const [, force] = useState(0);
  useEffect(() => {
    const fn = () => force((n) => n + 1);
    waiters.add(fn);
    const off = onThemeChange(fn);
    if (!data) void load();
    return () => {
      waiters.delete(fn);
      off();
    };
  }, []);
  return data;
}

/** Icon name (without extension) for a file/folder; null means the data is not loaded yet. */
export function iconName(name: string, dir: boolean, open = false): string | null {
  if (!data) return null;
  const m = data.manifest;
  const light = !isDark();
  const lower = name.toLowerCase();
  const has = (n: string | undefined) => (n && data!.urls[n] ? n : undefined);
  if (dir) {
    const base = (light && m.light.folders[lower]) || m.folders[lower] || 'folder';
    const icon = open ? `${base}-open` : base;
    return has(icon) ?? (open ? 'folder-open' : 'folder');
  }
  const pick = (map: Record<string, string>, lightMap: Record<string, string>, key: string) => has((light && lightMap[key]) || map[key]);
  const byName = pick(m.names, m.light.names, lower) ?? pick(m.names, m.light.names, name);
  if (byName) return byName;
  // compound extensions: a.test.tsx → test.tsx → tsx
  for (let i = name.indexOf('.'); i >= 0; i = name.indexOf('.', i + 1)) {
    const ext = lower.slice(i + 1);
    if (!ext) continue;
    const icon = pick(m.ext, m.light.ext, ext);
    if (icon) return icon;
  }
  return 'file';
}

export function FileIcon({ name, dir = false, open = false, className = 'size-4' }: { name: string; dir?: boolean; open?: boolean; className?: string }) {
  const d = useIconData();
  const icon = d ? iconName(name, dir, open) : null;
  const url = icon ? d!.urls[icon] : null;
  if (!url) {
    const Fallback = dir ? (open ? FolderOpen : Folder) : File;
    return <Fallback className={`${className} shrink-0 text-muted`} />;
  }
  return <img src={url} alt="" aria-hidden className={`${className} shrink-0`} draggable={false} />;
}
