import type { FileStatus } from '../types/session';

/** Mirrors VS Code's file-explorer convention: untracked/added → green,
 * modified → amber, deleted → red. `touched` (no git history to ask, e.g. a
 * copy-kind subsession) gets its own neutral color rather than guessing. */
export const FILE_STATUS_COLOR: Record<FileStatus, string> = {
  added: 'var(--grn)',
  untracked: 'var(--grn)',
  modified: 'var(--amber)',
  deleted: 'var(--red)',
  touched: 'var(--teal)',
};

export const FILE_STATUS_LABEL: Record<FileStatus, string> = {
  added: 'added',
  untracked: 'untracked',
  modified: 'modified',
  deleted: 'deleted',
  touched: 'touched',
};
