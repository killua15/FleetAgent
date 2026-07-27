import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { openUrl, revealItemInDir } from '@tauri-apps/plugin-opener';
import { Button } from '../components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '../components/ui/dialog';
import { Spinner } from '../components/Spinner';
import { toastError, toastSuccess } from '../components/ErrorOverlay';
import { useNavigation } from '../nav/NavigationProvider';
import { GitIdentityForm } from '../components/GitIdentityForm';
import type { ProjectGitOverview, WorktreeGitStatus } from '../types/gitPanel';
import type { PrComment, ProjectPrInbox } from '../types/pullRequest';
import styles from './GitPanel.module.css';

/** One in-flight git action, keyed by path+verb — lets each worktree's own
 * buttons show a spinner independently instead of freezing the whole panel
 * while any single action is running. */
type BusyKey = `${string}:${'fetch' | 'pull' | 'push'}`;

export function GitPanel() {
  const { navigate, back, canGoBack } = useNavigation();
  const [overview, setOverview] = useState<ProjectGitOverview[]>([]);
  const [prInbox, setPrInbox] = useState<ProjectPrInbox[]>([]);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [busyKey, setBusyKey] = useState<BusyKey | null>(null);
  const [openCommentsForPr, setOpenCommentsForPr] = useState<string | null>(null);

  const load = async () => {
    try {
      const [result, inbox] = await Promise.all([
        invoke<ProjectGitOverview[]>('get_git_overview'),
        invoke<ProjectPrInbox[]>('list_pr_review_inbox').catch(() => []),
      ]);
      setOverview(result);
      setPrInbox(inbox);
      setSelectedProjectId((prev) => (prev && result.some((p) => p.projectId === prev) ? prev : (result[0]?.projectId ?? null)));
    } catch (e) {
      toastError(String(e));
    } finally {
      setLoading(false);
    }
  };

  // Flat lookup from a PR's id to its still-pending review comments —
  // `list_pr_review_inbox` already groups by project/PR, this just
  // re-indexes it by PR id so `WorktreeCard` can look itself up regardless
  // of which project it belongs to.
  const commentsForPr = (prId: string): PrComment[] => {
    for (const project of prInbox) {
      const entry = project.entries.find((e) => e.pullRequest.id === prId);
      if (entry) return entry.comments;
    }
    return [];
  };

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const VERB_LABEL: Record<'fetch' | 'pull' | 'push', string> = { fetch: 'Fetched', pull: 'Pulled', push: 'Pushed' };

  const runAction = async (path: string, verb: 'fetch' | 'pull' | 'push', command: string) => {
    const key: BusyKey = `${path}:${verb}`;
    setBusyKey(key);
    try {
      await invoke(command, { path });
      await load();
      toastSuccess(VERB_LABEL[verb]);
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusyKey((prev) => (prev === key ? null : prev));
    }
  };

  const selected = overview.find((p) => p.projectId === selectedProjectId);

  return (
    <div className={styles.screen}>
      <div className={styles.header}>
        {canGoBack && (
          <Button variant="outline" className={styles.backBtn} onClick={back}>
            ← back
          </Button>
        )}
        <span className={styles.title}>Git</span>
        <span className={styles.subtitle}>Every project's repo, plus the worktrees currently in use.</span>
        <span className={styles.spacer} />
        <Button variant="outline" size="sm" disabled={loading} onClick={load}>
          {loading ? <Spinner label="refreshing…" /> : '↻ refresh'}
        </Button>
      </div>

      <div className={styles.body}>
        <div className={styles.projectList}>
          <span className={styles.sectionLabel}>PROJECTS</span>
          {loading && overview.length === 0 && (
            <div className={styles.dimText}>
              <Spinner label="checking repos…" />
            </div>
          )}
          {overview.map((p) => {
            const isSel = p.projectId === selectedProjectId;
            const activeWorktrees = p.worktrees.length;
            return (
              <button
                key={p.projectId}
                className={isSel ? styles.projectRowActive : styles.projectRow}
                onClick={() => setSelectedProjectId(p.projectId)}
              >
                <span className={styles.projectName}>{p.projectName}</span>
                <span className={styles.spacer} />
                {p.currentBranch && <span className={styles.branchBadge}>⑃ {p.currentBranch}</span>}
                {p.dirty && <span className={styles.dirtyDot} title="Uncommitted changes" />}
                {activeWorktrees > 0 && <span className={styles.countBadge}>{activeWorktrees}</span>}
              </button>
            );
          })}
          {!loading && overview.length === 0 && <span className={styles.dimText}>No projects yet.</span>}
        </div>

        <div className={styles.detail}>
          {selected ? (
            <>
              <RepoCard
                title={selected.projectName}
                subtitle={selected.repoOwner && selected.repoName ? `${selected.repoOwner}/${selected.repoName}` : selected.projectPath}
                branch={selected.currentBranch}
                ahead={selected.ahead}
                behind={selected.behind}
                dirty={selected.dirty}
                path={selected.projectPath}
                busy={busyKey}
                onFetch={() => runAction(selected.projectPath, 'fetch', 'git_fetch')}
                onPull={() => runAction(selected.projectPath, 'pull', 'git_pull')}
                onPush={() => runAction(selected.projectPath, 'push', 'git_push')}
                onHistory={() => navigate('git-history', { path: selected.projectPath, label: selected.projectName })}
              />

              {/* Right under the repo it applies to: which identity commits
                  here, and which credential pushes. */}
              <GitIdentityForm projectId={selected.projectId} projectName={selected.projectName} />

              <span className={styles.sectionLabel}>ACTIVE WORKTREES</span>
              {selected.worktrees.length === 0 && <span className={styles.dimText}>No active worktrees for this project.</span>}
              {selected.worktrees.map((wt) => (
                <WorktreeCard
                  key={wt.sessionId}
                  worktree={wt}
                  busy={busyKey}
                  pendingComments={wt.linkedPr ? commentsForPr(wt.linkedPr.id) : []}
                  onFetch={() => runAction(wt.path, 'fetch', 'git_fetch')}
                  onPull={() => runAction(wt.path, 'pull', 'git_pull')}
                  onPush={() => runAction(wt.path, 'push', 'git_push')}
                  onHistory={() => navigate('git-history', { path: wt.path, label: wt.sessionName })}
                  onOpenSession={() => navigate('agent-panel', { sessionId: wt.sessionId })}
                  onOpenPr={wt.linkedPr ? () => openUrl(wt.linkedPr!.url) : undefined}
                  onOpenComments={wt.linkedPr ? () => setOpenCommentsForPr(wt.linkedPr!.id) : undefined}
                />
              ))}
            </>
          ) : loading ? (
            <div className={styles.dimText}>
              <Spinner label="loading…" />
            </div>
          ) : (
            <span className={styles.dimText}>Select a project to see its repo status.</span>
          )}
        </div>
      </div>

      <PrCommentsDialog
        comments={openCommentsForPr ? commentsForPr(openCommentsForPr) : []}
        open={openCommentsForPr !== null}
        onOpenChange={(open) => !open && setOpenCommentsForPr(null)}
        onResolved={() => {
          setOpenCommentsForPr(null);
          load();
        }}
      />
    </div>
  );
}

function AheadBehind({ ahead, behind }: { ahead: number; behind: number }) {
  if (ahead === 0 && behind === 0) return null;
  return (
    <span className={styles.aheadBehind}>
      {ahead > 0 && <span className={styles.ahead}>↑{ahead}</span>}
      {behind > 0 && <span className={styles.behind}>↓{behind}</span>}
    </span>
  );
}

function GitActions({
  path,
  busy,
  onFetch,
  onPull,
  onPush,
  onHistory,
}: {
  path: string;
  busy: BusyKey | null;
  onFetch: () => void;
  onPull: () => void;
  onPush: () => void;
  onHistory: () => void;
}) {
  const isBusy = (verb: 'fetch' | 'pull' | 'push') => busy === `${path}:${verb}`;
  const anyBusy = busy?.startsWith(`${path}:`) ?? false;
  return (
    <div className={styles.actions}>
      <Button size="sm" variant="outline" disabled={anyBusy} onClick={onFetch}>
        {isBusy('fetch') ? <Spinner label="fetching…" /> : 'Fetch'}
      </Button>
      <Button size="sm" variant="outline" disabled={anyBusy} onClick={onPull}>
        {isBusy('pull') ? <Spinner label="pulling…" /> : 'Pull'}
      </Button>
      <Button size="sm" variant="outline" disabled={anyBusy} onClick={onPush}>
        {isBusy('push') ? <Spinner label="pushing…" /> : 'Push'}
      </Button>
      <Button size="sm" variant="outline" onClick={onHistory}>
        History
      </Button>
      <Button size="sm" variant="outline" onClick={() => revealItemInDir(path)}>
        Open in Finder
      </Button>
      <Button size="sm" variant="outline" onClick={() => invoke('open_path_in_terminal', { path })}>
        Open in Terminal
      </Button>
    </div>
  );
}

function RepoCard({
  title,
  subtitle,
  branch,
  ahead,
  behind,
  dirty,
  path,
  busy,
  onFetch,
  onPull,
  onPush,
  onHistory,
}: {
  title: string;
  subtitle: string;
  branch: string | null;
  ahead: number;
  behind: number;
  dirty: boolean;
  path: string;
  busy: BusyKey | null;
  onFetch: () => void;
  onPull: () => void;
  onPush: () => void;
  onHistory: () => void;
}) {
  return (
    <div className={styles.repoCard}>
      <div className={styles.repoCardHeader}>
        <span className={styles.repoTitle}>{title}</span>
        <span className={styles.repoSubtitle}>{subtitle}</span>
        <span className={styles.spacer} />
        {branch && <span className={styles.branchBadge}>⑃ {branch}</span>}
        <AheadBehind ahead={ahead} behind={behind} />
        {dirty && <span className={styles.dirtyDot} title="Uncommitted changes" />}
      </div>
      <GitActions path={path} busy={busy} onFetch={onFetch} onPull={onPull} onPush={onPush} onHistory={onHistory} />
    </div>
  );
}

function WorktreeCard({
  worktree,
  busy,
  pendingComments,
  onFetch,
  onPull,
  onPush,
  onHistory,
  onOpenSession,
  onOpenPr,
  onOpenComments,
}: {
  worktree: WorktreeGitStatus;
  busy: BusyKey | null;
  pendingComments: PrComment[];
  onFetch: () => void;
  onPull: () => void;
  onPush: () => void;
  onHistory: () => void;
  onOpenSession: () => void;
  onOpenPr?: () => void;
  onOpenComments?: () => void;
}) {
  return (
    <div className={styles.repoCard}>
      <div className={styles.repoCardHeader}>
        <button className={styles.sessionLink} onClick={onOpenSession} title="Open this session in Hangar">
          {worktree.sessionName}
        </button>
        {onOpenPr && (
          <button className={styles.prLink} onClick={onOpenPr} title="Open the linked PR on GitHub">
            PR #{worktree.linkedPr!.number} ↗
          </button>
        )}
        {onOpenComments && pendingComments.length > 0 && (
          <button
            className={styles.countBadge}
            style={{ border: 'none', cursor: 'pointer' }}
            onClick={onOpenComments}
            title="Review comments on this PR"
          >
            {pendingComments.length}
          </button>
        )}
        <span className={styles.spacer} />
        {worktree.branch && <span className={styles.branchBadge}>⑃ {worktree.branch}</span>}
        <AheadBehind ahead={worktree.ahead} behind={worktree.behind} />
        {worktree.dirty && <span className={styles.dirtyDot} title="Uncommitted changes" />}
      </div>
      <GitActions path={worktree.path} busy={busy} onFetch={onFetch} onPull={onPull} onPush={onPush} onHistory={onHistory} />
    </div>
  );
}

/** Compact "review comments" modal for a single worktree's linked PR — the
 * `PRModal.dc.html` mockup's second half. `PrReviews.tsx` (the full
 * cross-project inbox) and `AgentPanel.tsx`'s inline banner both cover the
 * same actions already; this is just a quick way to reach them without
 * leaving the git panel for a worktree you're already looking at. */
function PrCommentsDialog({
  comments,
  open,
  onOpenChange,
  onResolved,
}: {
  comments: PrComment[];
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onResolved: () => void;
}) {
  const [resolving, setResolving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleResolveAll = async () => {
    if (comments.length === 0 || resolving) return;
    setResolving(true);
    setError(null);
    try {
      await invoke('send_pr_comments_to_session', { commentIds: comments.map((c) => c.id) });
      onResolved();
    } catch (e) {
      setError(String(e));
    } finally {
      setResolving(false);
    }
  };

  const handleDismissAll = async () => {
    if (comments.length === 0 || resolving) return;
    setResolving(true);
    setError(null);
    try {
      await invoke('dismiss_pr_comments', { commentIds: comments.map((c) => c.id) });
      onResolved();
    } catch (e) {
      setError(String(e));
    } finally {
      setResolving(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Review comments</DialogTitle>
          <DialogDescription>{comments.length} pending — sends each comment to the session as a task.</DialogDescription>
        </DialogHeader>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10, maxHeight: 380, overflow: 'auto' }}>
          {comments.map((c) => (
            <div key={c.id} style={{ padding: '10px 12px', borderRadius: 9, background: 'var(--bg2)', border: '1px solid var(--line)' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 5, font: '600 11px var(--font-ui)' }}>
                <span>{c.author}</span>
                {c.path && (
                  <span style={{ font: '400 9.5px var(--font-mono)', color: 'var(--dim)' }}>
                    {c.path}
                    {c.line != null ? `:${c.line}` : ''}
                  </span>
                )}
              </div>
              <p style={{ font: '400 11.5px var(--font-ui)', color: 'var(--mut)', lineHeight: 1.55, margin: 0 }}>{c.body}</p>
            </div>
          ))}
        </div>
        {error && <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>{error}</span>}
        <DialogFooter>
          <Button variant="outline" disabled={resolving} onClick={handleDismissAll}>
            Dismiss all
          </Button>
          <Button disabled={resolving} onClick={handleResolveAll}>
            {resolving ? 'Sending…' : '⚡ Resolve all with agent'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
