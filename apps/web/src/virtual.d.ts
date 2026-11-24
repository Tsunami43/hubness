declare module 'virtual:file-icons' {
  type Map = Record<string, string>;
  export const manifest: { ext: Map; names: Map; folders: Map; light: { ext: Map; names: Map; folders: Map } };
  export const urls: Record<string, string>;
}
