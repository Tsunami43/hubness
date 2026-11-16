// Project files: tree, content, search, writes, live changes; server folder browsing.

// ---- Files ----

export interface FsEntry {
  name: string;
  path: string;
  dir: boolean;
  size?: number;
  /** Ignored by git (.gitignore) or an internal folder (node_modules…). */
  ignored?: boolean;
}

/** Files changed on disk in a project folder (GET /api/watch, batched). */
export interface FileChangeMsg {
  projectId: string;
  /** Changed paths relative to the project root ('/'-separated): files or folders. */
  paths: string[];
  /** HEAD, index or refs changed (commit, checkout, staging). */
  git: boolean;
  /** Too many changes at once (or the stream reconnected): reload everything. */
  overflow?: boolean;
}

export interface FileContent {
  path: string;
  size: number;
  binary: boolean;
  truncated: boolean;
  content: string;
  /** sha256 of the file bytes (text files that fit entirely) — base for a conflict-safe save. */
  hash?: string;
}

/** Text search across project files (Search view). */
export interface SearchOptions {
  q: string;
  regex?: boolean;
  caseSensitive?: boolean;
  /** Whole word. */
  word?: boolean;
  /** Comma-separated globs: files to include / exclude (e.g. "src/**, *.ts"). */
  include?: string;
  exclude?: string;
}

export interface SearchMatch {
  /** 1-based line number. */
  line: number;
  /** The line (cut to a preview length). */
  text: string;
  /** Match ranges inside `text` (UTF-16 indices, end exclusive). */
  ranges: [number, number][];
}

export interface SearchFile {
  path: string;
  matches: SearchMatch[];
}

export interface SearchResult {
  files: SearchFile[];
  total: number;
  /** The match limit or the timeout was hit — not every match is listed. */
  truncated: boolean;
}

/** Saving a file from the editor. */
export interface FileWrite {
  path: string;
  content: string;
  /** Hash the edit started from; if the file changed since, the server answers 409. Omit to overwrite. */
  baseHash?: string;
}

export interface BrowseResponse {
  path: string;
  parent: string | null;
  entries: FsEntry[];
  isGitRepo: boolean;
}
