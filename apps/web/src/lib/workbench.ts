import { createContext, useContext } from 'react';
import type { OpenOpts } from './layout';

/** Workbench actions available to panels inside it (e.g. Git opens a diff as a tab). */
export interface WorkbenchApi {
  openFile: (repo: string, path: string, opts?: OpenOpts) => void;
  openTool: (id: string) => void;
  /** Open an agent session as its own tab; 'new' opens a new-session draft. */
  openSession: (id: string, opts?: OpenOpts) => void;
  /** The tree renamed/moved a file or folder — retarget open tabs. */
  renamed: (repo: string, from: string, to: string) => void;
  /** The tree deleted a file or folder — close its tabs. */
  deleted: (repo: string, path: string) => void;
}

export const WorkbenchContext = createContext<WorkbenchApi | null>(null);

/** null outside a workbench (the panel then falls back to its own behaviour). */
export const useWorkbench = () => useContext(WorkbenchContext);
