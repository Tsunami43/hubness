// Shared hubness API types, one module per domain. Types only — empty at runtime.

export type * from './agents.ts';
export type * from './auth.ts';
export type * from './files.ts';
export type * from './git.ts';
export type * from './jobs.ts';
export type * from './projects.ts';
export type * from './sessions.ts';
export type * from './system.ts';
export type * from './voice.ts';

export interface ApiError {
  error: string;
}
