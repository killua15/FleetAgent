import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import type { BranchInfo } from '../types/gitHistory';
import styles from './BaseBranchPicker.module.css';

/** What the picker knows about a project's git state, so the caller can block
 * launching rather than letting the backend fail after the click. */
export interface BaseBranchState {
  /** `null` while loading. */
  branches: BranchInfo[] | null;
  /** Set when the project has no repo — a worktree is a git object, so there's
   * nothing to cut. */
  error: string | null;
  selected: string;
}

/**
 * Picks the branch a new worktree starts from.
 *
 * Naming the base is not a nicety: a worktree cut off whatever happened to be
 * checked out inherits half-finished work, and the agent's diff then contains
 * changes that were never its own. And a project with no repo can't have a
 * worktree at all — better to say so here, before a prompt has been typed, than
 * to fail at launch.
 */
export function BaseBranchPicker({
  projectPath,
  state,
  onChange,
}: {
  projectPath: string | undefined;
  state: BaseBranchState;
  onChange: (next: BaseBranchState) => void;
}) {
  if (!projectPath) return null;

  if (state.error) {
    return <span className={styles.error}>⚠ {state.error}</span>;
  }
  if (!state.branches) {
    return <span className={styles.loading}>reading branches…</span>;
  }

  return (
    <label className={styles.row}>
      <span className={styles.label}>from</span>
      <select className={styles.select} value={state.selected} onChange={(e) => onChange({ ...state, selected: e.target.value })}>
        {state.branches.map((b) => (
          <option key={b.name} value={b.name}>
            {b.name}
            {b.isCurrent ? ' (current)' : ''}
          </option>
        ))}
      </select>
    </label>
  );
}

/** Loads a project's branches, or the reason there are none to load.
 *
 * Local branches only: a worktree has to start from something that exists in
 * this clone, and offering `origin/*` would produce a base git refuses to use
 * without a fetch nobody asked for.
 */
export function useBaseBranch(projectPath: string | undefined): [BaseBranchState, (next: BaseBranchState) => void] {
  const [state, setState] = useState<BaseBranchState>({ branches: null, error: null, selected: '' });

  useEffect(() => {
    if (!projectPath) {
      setState({ branches: null, error: null, selected: '' });
      return;
    }
    let cancelled = false;
    setState({ branches: null, error: null, selected: '' });
    invoke<BranchInfo[]>('list_branches', { path: projectPath })
      .then((all) => {
        if (cancelled) return;
        const local = all.filter((b) => !b.isRemote);
        if (local.length === 0) {
          setState({
            branches: null,
            // A repo with no commits has no branches either — same dead end
            // from the user's side, so the message covers both.
            error: "this project isn't a git repository (or has no commits yet), so Hangar can't cut a worktree for it",
            selected: '',
          });
          return;
        }
        setState({ branches: local, error: null, selected: (local.find((b) => b.isCurrent) ?? local[0]).name });
      })
      .catch(() => {
        if (cancelled) return;
        setState({
          branches: null,
          error: "this project isn't a git repository, so Hangar can't cut a worktree for it — run `git init` there first",
          selected: '',
        });
      });
    return () => {
      cancelled = true;
    };
  }, [projectPath]);

  return [state, setState];
}
