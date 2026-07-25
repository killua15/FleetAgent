import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Button } from '../components/ui/button';
import { Spinner } from '../components/Spinner';
import { toastError } from '../components/ErrorOverlay';
import { useNavigation } from '../nav/NavigationProvider';
import type { PrComment, PrInboxEntry, ProjectPrInbox } from '../types/pullRequest';
import type { TerminalInfo } from '../types/terminal';
import styles from './PrReviews.module.css';

// Mirrors the background poller's interval in `lib.rs` (`Duration::from_secs(300)`)
// — purely a countdown display, doesn't drive the actual poll.
const AUTO_CHECK_INTERVAL_SECONDS = 300;

function formatCountdown(seconds: number): string {
  const m = Math.floor(seconds / 60);
  const s = seconds % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

export function PrReviews() {
  const { navigate, back, canGoBack } = useNavigation();
  const [inbox, setInbox] = useState<ProjectPrInbox[]>([]);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [checkingNow, setCheckingNow] = useState(false);
  const [busyCommentId, setBusyCommentId] = useState<string | null>(null);
  const [secondsUntilAutoCheck, setSecondsUntilAutoCheck] = useState(AUTO_CHECK_INTERVAL_SECONDS);

  const load = async () => {
    setLoading(true);
    try {
      const result = await invoke<ProjectPrInbox[]>('list_pr_review_inbox');
      setInbox(result);
      setSelectedProjectId((prev) => (prev && result.some((p) => p.projectId === prev) ? prev : (result[0]?.projectId ?? null)));
    } finally {
      setLoading(false);
    }
  };

  // `list_pr_review_inbox` only reads what Fleet already knows about —
  // discovering PRs opened outside Fleet (or new comments) needs a real
  // `gh` round-trip, so both the first load and the manual button run the
  // full check rather than just re-reading the local database.
  const runFullCheck = async () => {
    try {
      await invoke('check_all_open_prs_now');
      setSecondsUntilAutoCheck(AUTO_CHECK_INTERVAL_SECONDS);
    } catch (e) {
      toastError(String(e));
    } finally {
      // Reload from the local DB either way — even if the real `gh` check
      // failed, this at least shows whatever Fleet already had tracked,
      // and clears the loading spinner instead of leaving it stuck.
      await load();
    }
  };

  const handleCheckNow = async () => {
    if (checkingNow) return;
    setCheckingNow(true);
    try {
      await runFullCheck();
    } finally {
      setCheckingNow(false);
    }
  };

  useEffect(() => {
    runFullCheck();
    const unlisten = listen('pr-comments-found', () => {
      load();
      setSecondsUntilAutoCheck(AUTO_CHECK_INTERVAL_SECONDS);
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Purely cosmetic countdown to the background poller's next 5-minute pass
  // — resets on a real check (manual or poller-signaled), floors at 0 rather
  // than going negative if a cycle runs long.
  useEffect(() => {
    const id = setInterval(() => {
      setSecondsUntilAutoCheck((s) => Math.max(0, s - 1));
    }, 1000);
    return () => clearInterval(id);
  }, []);

  const handleSend = async (comment: PrComment) => {
    setBusyCommentId(comment.id);
    try {
      // Opens a real interactive terminal on the PR's own branch (a worktree
      // the backend resolves or creates) under the project's account, running
      // `claude` already working on the comments. 80x24 is the shell's initial
      // size; the pane's first fit() corrects it.
      await invoke<TerminalInfo>('send_pr_comments_to_terminal', { commentIds: [comment.id], cols: 80, rows: 24 });
      await load();
      navigate('terminals');
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusyCommentId(null);
    }
  };

  const handleDismiss = async (comment: PrComment) => {
    setBusyCommentId(comment.id);
    try {
      await invoke('dismiss_pr_comments', { commentIds: [comment.id] });
      await load();
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusyCommentId(null);
    }
  };

  const selected = inbox.find((p) => p.projectId === selectedProjectId);
  const totalPending = (p: ProjectPrInbox) => p.entries.reduce((sum, e) => sum + e.comments.length, 0);

  return (
    <div className={styles.screen}>
      <div className={styles.header}>
        {canGoBack && (
          <Button variant="outline" className={styles.backBtn} onClick={back}>
            ← back
          </Button>
        )}
        <span className={styles.title}>PR reviews</span>
        <span className={styles.subtitle}>Open pull requests by project, with any new review comments from other people.</span>
        <span className={styles.spacer} />
        <span className={styles.countdown}>next auto-check in {formatCountdown(secondsUntilAutoCheck)}</span>
        <Button variant="outline" size="sm" disabled={checkingNow} onClick={handleCheckNow}>
          {checkingNow ? <Spinner label="checking…" /> : '↻ check now'}
        </Button>
      </div>


      <div className={styles.body}>
        <div className={styles.projectList}>
          <span className={styles.sectionLabel}>PROJECTS</span>
          {loading && inbox.length === 0 && (
            <div className={styles.dimText}>
              <Spinner label="checking GitHub for open PRs…" />
            </div>
          )}
          {inbox.map((p) => {
            const isSel = p.projectId === selectedProjectId;
            const pending = totalPending(p);
            const hasError = p.entries.some((e) => e.pullRequest.lastCheckError);
            return (
              <button
                key={p.projectId}
                className={isSel ? styles.projectRowActive : styles.projectRow}
                onClick={() => setSelectedProjectId(p.projectId)}
              >
                <span className={styles.projectName}>{p.projectName}</span>
                <span className={styles.spacer} />
                <span className={styles.prCountDim}>{p.entries.length} open</span>
                {hasError && <span title="A PR check is failing in this project">⚠</span>}
                {pending > 0 && <span className={styles.countBadge}>{pending}</span>}
              </button>
            );
          })}
          {!loading && inbox.length === 0 && <span className={styles.dimText}>No open PRs yet.</span>}
        </div>

        <div className={styles.detail}>
          {selected ? (
            selected.entries.map((entry) => (
              <PrEntryCard
                key={entry.pullRequest.id}
                entry={entry}
                busyCommentId={busyCommentId}
                onOpenPr={() => openUrl(entry.pullRequest.url)}
                onSend={handleSend}
                onDismiss={handleDismiss}
              />
            ))
          ) : loading ? (
            <div className={styles.dimText}>
              <Spinner label="loading…" />
            </div>
          ) : (
            <span className={styles.dimText}>Select a project to see its open PRs.</span>
          )}
        </div>
      </div>
    </div>
  );
}

function PrEntryCard({
  entry,
  busyCommentId,
  onOpenPr,
  onSend,
  onDismiss,
}: {
  entry: PrInboxEntry;
  busyCommentId: string | null;
  onOpenPr: () => void;
  onSend: (comment: PrComment) => void;
  onDismiss: (comment: PrComment) => void;
}) {
  // Collapsible per card — a PR with several long reviews shouldn't force
  // every other PR further down the page; the header (link/session/count)
  // always stays visible, only the body folds away. Starts expanded so
  // nothing pending goes unnoticed on first look.
  const hasBody = entry.comments.length > 0 || Boolean(entry.pullRequest.lastCheckError);
  const [collapsed, setCollapsed] = useState(false);

  return (
    <div className={styles.prCard}>
      <div className={styles.prCardHeader}>
        {hasBody && (
          <button
            className={styles.collapseToggle}
            onClick={() => setCollapsed((v) => !v)}
            title={collapsed ? 'Expand' : 'Collapse'}
          >
            {collapsed ? '▸' : '▾'}
          </button>
        )}
        <button className={styles.prLink} onClick={onOpenPr} title="Open on GitHub">
          {entry.pullRequest.repoOwner}/{entry.pullRequest.repoName} #{entry.pullRequest.number} ↗
        </button>
        <span className={styles.sessionLink}>{entry.sessionName}</span>
        {entry.pullRequest.headBranch && (
          <span className={styles.commentLocation} title="This PR's real branch — the terminal opens in a worktree checked out here">
            ⑃ {entry.pullRequest.headBranch}
          </span>
        )}
        <span className={styles.spacer} />
        {entry.comments.length > 0 && <span className={styles.countBadge}>{entry.comments.length}</span>}
      </div>
      {!collapsed && (
        <>
          {entry.pullRequest.lastCheckError ? (
            <div className={styles.checkError} title={entry.pullRequest.lastCheckError}>
              ⚠ last check failed: {entry.pullRequest.lastCheckError}
            </div>
          ) : (
            entry.comments.length === 0 && <div className={styles.noComments}>no new comments</div>
          )}
          <div className={styles.commentList}>
            {entry.comments.map((c) => (
              <div key={c.id} className={styles.commentRow}>
                <div className={styles.commentMeta}>
                  <span className={styles.commentAuthor}>{c.author}</span>
                  {c.path && (
                    <span className={styles.commentLocation} title="Inline diff comment">
                      {c.path}
                      {c.line != null ? `:${c.line}` : ''}
                    </span>
                  )}
                  <span className={styles.commentDate}>{new Date(c.createdAt).toLocaleString()}</span>
                </div>
                <p className={styles.commentBody}>{c.body}</p>
                <div className={styles.commentActions}>
                  <Button size="sm" disabled={busyCommentId === c.id} onClick={() => onSend(c)} title="Open a terminal on this PR's branch and hand the comments to Claude Code">
                    {busyCommentId === c.id ? 'Opening…' : 'Send to terminal'}
                  </Button>
                  <Button size="sm" variant="outline" disabled={busyCommentId === c.id} onClick={() => onDismiss(c)}>
                    Dismiss
                  </Button>
                </div>
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
