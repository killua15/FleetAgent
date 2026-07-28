import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { CodeView } from '../components/CodeView';
import { toastError } from '../components/ErrorOverlay';
import { Button } from '../components/ui/button';
import { useNavigation } from '../nav/NavigationProvider';
import type { ChangedFile, WorktreeChanges } from '../types/gitPanel';
import type { TerminalActivity } from '../types/terminal';
import styles from './Changes.module.css';

/** GitHub Desktop's shape, because it's the one that got this right: the list
 * of files on the left, the diff filling everything else. */
const STATUS_LETTER: Record<ChangedFile['status'], string> = {
  added: 'A',
  modified: 'M',
  deleted: 'D',
  untracked: '?',
  touched: '·',
};

export function Changes() {
  const { current, navigate } = useNavigation();
  const path = current.params?.path as string | undefined;
  const label = (current.params?.label as string | undefined) ?? 'worktree';

  const [changes, setChanges] = useState<WorktreeChanges | null>(null);
  const [loading, setLoading] = useState(false);
  // Which file is open, and from which list — the same file can be in both
  // with different content, so the list is part of the identity.
  const [selected, setSelected] = useState<{ file: ChangedFile; committed: boolean } | null>(null);
  const [diff, setDiff] = useState<{ oldText: string; newText: string } | null>(null);

  const load = async () => {
    if (!path) return;
    setLoading(true);
    try {
      const found = await invoke<WorktreeChanges>('get_worktree_changes', { path, base: null });
      setChanges(found);
      // Keep whatever was open across a refresh — this reloads when an agent
      // goes idle, and having the diff jump away mid-read would be worse than
      // not refreshing at all.
      setSelected((prev) => {
        if (prev) return prev;
        const first = found.uncommitted[0] ?? found.committed[0];
        return first ? { file: first, committed: !found.uncommitted[0] } : null;
      });
    } catch (e) {
      toastError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load();
  }, [path]);

  // Refresh when an agent stops working, rather than polling. That's the exact
  // moment the files stop moving, and it's the difference between a panel that
  // asks git every two seconds for six worktrees and one that finds out.
  const reload = useRef(load);
  reload.current = load;
  useEffect(() => {
    const unlisten = listen<TerminalActivity>('terminal-state', (event) => {
      if (event.payload.state === 'idle' || event.payload.state === 'waiting') reload.current();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!path || !selected) {
      setDiff(null);
      return;
    }
    let cancelled = false;
    invoke<{ oldText: string; newText: string }>('get_worktree_file_diff', {
      path,
      filePath: selected.file.path,
      base: null,
      committed: selected.committed,
    })
      .then((found) => {
        if (!cancelled) setDiff(found);
      })
      .catch((e) => toastError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [path, selected?.file.path, selected?.committed]);

  const total = (files: ChangedFile[]) =>
    files.reduce((sum, f) => ({ added: sum.added + f.added, removed: sum.removed + f.removed }), { added: 0, removed: 0 });

  const section = (title: string, hint: string, files: ChangedFile[], committed: boolean) => {
    const sum = total(files);
    return (
      <div className={styles.section}>
        <div className={styles.sectionHead}>
          <span className={styles.sectionTitle}>{title}</span>
          <span className={styles.sectionHint}>{hint}</span>
          <span className={styles.spacer} />
          {files.length > 0 && (
            <span className={styles.counts}>
              <span className={styles.added}>+{sum.added}</span> <span className={styles.removed}>−{sum.removed}</span>
            </span>
          )}
        </div>
        {files.length === 0 && <span className={styles.sectionEmpty}>nothing</span>}
        {files.map((file) => {
          const active = selected?.file.path === file.path && selected.committed === committed;
          return (
            <button
              key={`${committed}-${file.path}`}
              className={active ? styles.fileActive : styles.file}
              onClick={() => setSelected({ file, committed })}
              title={file.path}
            >
              <span className={`${styles.status} ${styles[file.status]}`}>{STATUS_LETTER[file.status]}</span>
              {/* The name is what you scan for; the directory is context and
                  gets truncated from the left, where the useless part is. */}
              <span className={styles.fileName}>{file.path.split('/').pop()}</span>
              <span className={styles.fileDir}>{file.path.split('/').slice(0, -1).join('/')}</span>
              <span className={styles.fileCounts}>
                <span className={styles.added}>+{file.added}</span> <span className={styles.removed}>−{file.removed}</span>
              </span>
            </button>
          );
        })}
      </div>
    );
  };

  return (
    <div className={styles.screen}>
      <AppTopBar />

      <div className={styles.head}>
        <button className={styles.backLink} onClick={() => navigate('git-panel')}>
          ← git
        </button>
        <span className={styles.title}>{label}</span>
        {changes && <span className={styles.base}>vs {changes.base}</span>}
        <span className={styles.spacer} />
        <span className={styles.readOnly}>read-only</span>
        <Button size="sm" variant="outline" disabled={loading} onClick={load}>
          {loading ? 'reading…' : '↻ refresh'}
        </Button>
      </div>

      <div className={styles.body}>
        <div className={styles.list}>
          {changes && section('uncommitted', 'what the agent is doing right now', changes.uncommitted, false)}
          {changes && section('in this branch', `committed on top of ${changes.base}`, changes.committed, true)}
        </div>

        <div className={styles.diff}>
          {!selected && <span className={styles.diffEmpty}>Pick a file.</span>}
          {selected && diff && (
            // `original` turns CodeView into a diff — the same unified merge
            // view that renders commit diffs, so a diff looks like a diff
            // everywhere in the app rather than per screen.
            <CodeView path={selected.file.path} text={diff.newText} original={diff.oldText} />
          )}
        </div>
      </div>
    </div>
  );
}
