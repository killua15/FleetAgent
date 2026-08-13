import { useEffect, useRef, useState } from 'react';
import { invoke } from '../lib/invoke';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Button } from '../components/ui/button';
import { Spinner } from '../components/Spinner';
import { toastError } from '../components/ErrorOverlay';
import { useNavigation } from '../nav/NavigationProvider';
import { AccountPickerButton } from '../components/AccountPickerButton';
import { ForgePicker } from '../components/ForgePicker';
import { useForgeHost } from '../hooks/useForgeHost';
import { ALL_HOSTS, toForgeHostOptions } from '../lib/forgeHosts';
import type { ForgeHost, PrHostCheck } from '../types/forgeStatus';
import { FORGE_LABEL, FORGE_SIGIL, PR_RELATION_LABEL, type PrComment, type PrInboxEntry, type PrRelation, type ProjectPrInbox } from '../types/pullRequest';
import type { TerminalInfo } from '../types/terminal';
import styles from './PrReviews.module.css';

/** Tab order, and with it the reading order of the screen: what's yours to
 * finish, what someone asked you to look at, then the rest of the repo. */
const RELATION_TABS: PrRelation[] = ['mine', 'review', 'other'];

/**
 * How old what's on screen is.
 *
 * This replaced a countdown to the next background pass. A countdown answered
 * the wrong question: the list is painted from the local database now, so what
 * matters is when this instance was last reached — and, when it wasn't, that
 * being said out loud instead of shown as an empty list.
 */
function freshnessText(check: PrHostCheck | undefined, checking: boolean): string {
  if (checking) return 'checking…';
  if (!check) return 'not checked yet';
  if (check.error) return 'last check failed';
  const seconds = Math.max(0, Math.round((Date.now() - new Date(check.checkedAt).getTime()) / 1000));
  if (seconds < 60) return 'checked just now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `checked ${minutes}m ago`;
  return `checked ${Math.round(minutes / 60)}h ago`;
}

export function PrReviews() {
  const { navigate, back, canGoBack } = useNavigation();
  const [inbox, setInbox] = useState<ProjectPrInbox[]>([]);
  const [hosts, setHosts] = useState<ForgeHost[] | null>(null);
  const [checks, setChecks] = useState<PrHostCheck[]>([]);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [checkingNow, setCheckingNow] = useState(false);
  const [busyCommentId, setBusyCommentId] = useState<string | null>(null);
  const [reviewingPrId, setReviewingPrId] = useState<string | null>(null);
  const [sendingAllPrId, setSendingAllPrId] = useState<string | null>(null);
  const [activeTab, setActiveTab] = useState<PrRelation>('mine');
  const [forgeHost, setForgeHost] = useForgeHost();

  // The stored choice is shared with the git panel, which does offer "All".
  // Here that reads as "nothing chosen yet": this screen fetches per instance,
  // so every provider at once is precisely what it must not do on its own.
  const activeHost = forgeHost === ALL_HOSTS ? null : forgeHost;

  // What the screen is showing *right now*, readable from inside a promise
  // that started before the last switch. A `gh` check can take seconds, and
  // without this the answer for the provider you just left would land on top
  // of the one you just picked.
  const showing = useRef(activeHost);
  showing.current = activeHost;

  /** From the local database — no network, so this is what paints first. */
  const load = async (host: string) => {
    setLoading(true);
    try {
      const result = await invoke<ProjectPrInbox[]>('list_pr_review_inbox', { host });
      const stamps = await invoke<PrHostCheck[]>('list_pr_host_checks');
      if (showing.current !== host) return;
      setInbox(result);
      setSelectedProjectId((prev) => (prev && result.some((p) => p.projectId === prev) ? prev : (result[0]?.projectId ?? null)));
      setChecks(stamps);
    } finally {
      if (showing.current === host) setLoading(false);
    }
  };

  /** Re-read the provider on screen after an action changed something. A no-op
   * without one, which can't happen from the buttons that call it — they only
   * exist once a provider is showing. */
  const reload = () => (activeHost ? load(activeHost) : Promise.resolve());

  // The database only holds what some pass has already fetched — a PR opened
  // on the forge five minutes ago needs a real `gh`/`glab` round trip to show
  // up. So this runs *after* the local read has painted, never in front of it,
  // and only ever for the instance on screen.
  const refreshFromForge = async (host: string) => {
    setCheckingNow(true);
    try {
      await invoke('check_open_prs_now', { host });
    } catch (e) {
      if (showing.current === host) toastError(String(e));
    } finally {
      if (showing.current === host) setCheckingNow(false);
      // Either way: a failed check still leaves whatever was already tracked,
      // and the stamp below is what explains the gap.
      await load(host);
    }
  };

  useEffect(() => {
    invoke<ForgeHost[]>('list_forge_hosts')
      .then(setHosts)
      .catch(() => setHosts([]));
  }, []);

  // Everything below hangs off the chosen instance: no choice, no work. This
  // is the whole point of the screen — opening the tab costs nothing until
  // someone says which provider they mean.
  useEffect(() => {
    if (!activeHost) {
      setInbox([]);
      setSelectedProjectId(null);
      return;
    }
    load(activeHost).then(() => refreshFromForge(activeHost));
    const unlisten = listen('pr-comments-found', () => load(activeHost));
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeHost]);

  const handleCheckNow = async () => {
    if (checkingNow || !activeHost) return;
    await refreshFromForge(activeHost);
  };

  const handleSend = async (comment: PrComment, accountId: string) => {
    setBusyCommentId(comment.id);
    try {
      // Opens a real interactive terminal on the PR's own branch (a worktree
      // the backend resolves or creates) under the chosen account, running
      // `claude` already working on the comments. 80x24 is the shell's initial
      // size; the pane's first fit() corrects it.
      const info = await invoke<TerminalInfo>('send_pr_comments_to_terminal', { commentIds: [comment.id], accountId, cols: 80, rows: 24 });
      await reload();
      // Restores the panel if it was minimized, and scrolls it into view —
      // without this the comment lands in a terminal the user never sees.
      navigate('terminals', { focusTerminalId: info.id });
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusyCommentId(null);
    }
  };

  /** Same command as `handleSend`, just with every pending comment's id
   * instead of one — `send_pr_comments_to_terminal` already formats however
   * many comments it's given into a single prompt and opens (or reuses) one
   * terminal for them (see `format_pr_comments_prompt`), so this needed no
   * backend change, only a button that passes all of them at once instead of
   * one per call. */
  const handleSendAll = async (entry: PrInboxEntry, accountId: string) => {
    setSendingAllPrId(entry.pullRequest.id);
    try {
      const info = await invoke<TerminalInfo>('send_pr_comments_to_terminal', {
        commentIds: entry.comments.map((c) => c.id),
        accountId,
        cols: 80,
        rows: 24,
      });
      await reload();
      navigate('terminals', { focusTerminalId: info.id });
    } catch (e) {
      toastError(String(e));
    } finally {
      setSendingAllPrId(null);
    }
  };

  /** Reviewing a PR — the point of listing other people's at all. Same
   * worktree-on-the-PR's-branch machinery the comments path uses, with Claude
   * Code opening on the diff instead of on feedback to address. */
  const handleReview = async (entry: PrInboxEntry, accountId: string) => {
    setReviewingPrId(entry.pullRequest.id);
    try {
      const info = await invoke<TerminalInfo>('open_pr_review_terminal', { prId: entry.pullRequest.id, accountId, cols: 80, rows: 24 });
      navigate('terminals', { focusTerminalId: info.id });
    } catch (e) {
      toastError(String(e));
    } finally {
      setReviewingPrId(null);
    }
  };

  const handleDismiss = async (comment: PrComment) => {
    setBusyCommentId(comment.id);
    try {
      await invoke('dismiss_pr_comments', { commentIds: [comment.id] });
      await reload();
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusyCommentId(null);
    }
  };

  const hostOptions = toForgeHostOptions(hosts ?? []);
  // No client-side filtering left: `list_pr_review_inbox` was asked for this
  // instance and answered with only its projects.
  const visibleInbox = inbox;
  const activeCheck = checks.find((c) => c.host === activeHost);

  const selected = visibleInbox.find((p) => p.projectId === selectedProjectId);
  // Only your own PRs carry pending comments (they're the only ones polled),
  // so this count keeps meaning "things waiting on you" now that the list is
  // no longer only yours.
  const totalPending = (p: ProjectPrInbox) => p.entries.reduce((sum, e) => sum + e.comments.length, 0);
  const entriesFor = (relation: PrRelation) => selected?.entries.filter((e) => e.pullRequest.relation === relation) ?? [];
  const visibleEntries = entriesFor(activeTab);

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
        {activeHost && <span className={styles.countdown}>{freshnessText(activeCheck, checkingNow)}</span>}
        {activeHost && (
          <Button variant="outline" size="sm" disabled={checkingNow} onClick={handleCheckNow}>
            {checkingNow ? <Spinner label="checking…" /> : '↻ check now'}
          </Button>
        )}
      </div>


      <div className={styles.body}>
        <div className={styles.projectList}>
          <span className={styles.sectionLabel}>PROVIDER</span>
          <ForgePicker options={hostOptions} selected={forgeHost} onSelect={setForgeHost} allowAll={false} countTitle="projects on this instance" />
          {/* Nothing is fetched until a provider is picked — that choice is the
              screen's first step, not a filter over something already loaded. */}
          {!activeHost && hosts !== null && (
            <span className={styles.dimText}>
              {hostOptions.length === 0 ? 'No forge instance yet — add a project with a git remote.' : 'Pick a provider to see its open PRs.'}
            </span>
          )}
          {activeHost && loading && inbox.length === 0 && (
            <div className={styles.dimText}>
              <Spinner label={`reading ${activeHost}…`} />
            </div>
          )}
          {visibleInbox.map((p) => {
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
          {/* "Nothing here" and "never managed to ask" are different answers,
              and the stamp is what tells them apart. */}
          {activeHost && !loading && inbox.length === 0 && (
            <span className={styles.dimText}>
              {checkingNow
                ? `Nothing recorded for ${activeHost} yet — checking…`
                : activeCheck?.error
                  ? `Couldn't reach ${activeHost}: ${activeCheck.error}`
                  : activeCheck
                    ? `No open PRs on ${activeHost}.`
                    : `${activeHost} hasn't been checked yet.`}
            </span>
          )}
        </div>

        <div className={styles.detail}>
          {selected ? (
            <>
              <div className={styles.tabs}>
                {RELATION_TABS.map((relation) => {
                  const count = entriesFor(relation).length;
                  return (
                    <button
                      key={relation}
                      className={relation === activeTab ? styles.tabActive : styles.tab}
                      onClick={() => setActiveTab(relation)}
                    >
                      {PR_RELATION_LABEL[relation]}
                      <span className={styles.tabCount}>{count}</span>
                    </button>
                  );
                })}
              </div>
              {visibleEntries.map((entry) => (
                <PrEntryCard
                  key={entry.pullRequest.id}
                  entry={entry}
                  busyCommentId={busyCommentId}
                  reviewing={reviewingPrId === entry.pullRequest.id}
                  sendingAll={sendingAllPrId === entry.pullRequest.id}
                  onOpenPr={() => openUrl(entry.pullRequest.url)}
                  onSend={handleSend}
                  onSendAll={handleSendAll}
                  onReview={handleReview}
                  onDismiss={handleDismiss}
                />
              ))}
              {visibleEntries.length === 0 && <span className={styles.dimText}>{emptyTabText(activeTab)}</span>}
            </>
          ) : !activeHost ? (
            <span className={styles.dimText}>Pick a provider on the left. Nothing is fetched until you do.</span>
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

/** What an empty tab should say — each one is empty for a different reason,
 * and "nothing here" would hide which. */
function emptyTabText(relation: PrRelation): string {
  switch (relation) {
    case 'mine':
      return 'No open PRs of yours on this project.';
    case 'review':
      return 'Nobody has asked you to review anything on this project.';
    case 'other':
      return 'No other open PRs on this project.';
  }
}

function PrEntryCard({
  entry,
  busyCommentId,
  reviewing,
  sendingAll,
  onOpenPr,
  onSend,
  onSendAll,
  onReview,
  onDismiss,
}: {
  entry: PrInboxEntry;
  busyCommentId: string | null;
  reviewing: boolean;
  sendingAll: boolean;
  onOpenPr: () => void;
  onSend: (comment: PrComment, accountId: string) => void;
  onSendAll: (entry: PrInboxEntry, accountId: string) => void;
  onReview: (entry: PrInboxEntry, accountId: string) => void;
  onDismiss: (comment: PrComment) => void;
}) {
  const isMine = entry.pullRequest.relation === 'mine';
  const resolved = isMine && entry.comments.length === 0 && !entry.pullRequest.lastCheckError;
  // Collapsible per card — a PR with several long reviews shouldn't force
  // every other PR further down the page; the header (link/session/count)
  // always stays visible, only the body folds away. Someone else's PR starts
  // folded: there's nothing pending on it, the title and author in the header
  // are the whole story until you decide to review it.
  const hasBody = entry.comments.length > 0 || Boolean(entry.pullRequest.lastCheckError) || !isMine;
  const [collapsed, setCollapsed] = useState(!isMine);

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
        {/* `!42` on GitLab, `#42` on GitHub — the forge's own notation, since
            `#` names an issue on GitLab, not a merge request. */}
        <button className={styles.prLink} onClick={onOpenPr} title={`Open on ${FORGE_LABEL[entry.pullRequest.forge]}`}>
          {entry.pullRequest.repoOwner}/{entry.pullRequest.repoName} {FORGE_SIGIL[entry.pullRequest.forge]}
          {entry.pullRequest.number} ↗
        </button>
        {entry.pullRequest.title && <span className={styles.prTitle}>{entry.pullRequest.title}</span>}
        {/* Whose PR this is: the session for one of yours, the author for
            anyone else's. */}
        {entry.sessionName && <span className={styles.sessionLink}>{entry.sessionName}</span>}
        {!isMine && entry.pullRequest.authorLogin && <span className={styles.prAuthor}>@{entry.pullRequest.authorLogin}</span>}
        {entry.pullRequest.headBranch && (
          <span className={styles.commentLocation} title="This PR's real branch — the terminal opens in a worktree checked out here">
            ⑃ {entry.pullRequest.headBranch}
          </span>
        )}
        <span className={styles.spacer} />
        {entry.comments.length > 0 ? (
          <span className={styles.countBadge}>{entry.comments.length}</span>
        ) : (
          resolved && (
            <span className={styles.resolvedCheck} title="No pending comments">
              ✓
            </span>
          )
        )}
        {entry.comments.length > 1 && (
          <AccountPickerButton
            label={sendingAll ? 'Opening…' : `Send all ${entry.comments.length}`}
            busy={sendingAll}
            title="Send every pending comment on this PR to one terminal, in a single prompt"
            onPick={(accountId) => onSendAll(entry, accountId)}
          />
        )}
        <AccountPickerButton
          label={reviewing ? 'Opening…' : 'Review'}
          busy={reviewing}
          title="Open a terminal on this PR's branch with Claude Code reading the diff"
          onPick={(accountId) => onReview(entry, accountId)}
        />
      </div>
      {!collapsed && (
        <>
          {entry.pullRequest.lastCheckError ? (
            <div className={styles.checkError} title={entry.pullRequest.lastCheckError}>
              ⚠ last check failed: {entry.pullRequest.lastCheckError}
            </div>
          ) : (
            entry.comments.length === 0 && (
              <div className={styles.noComments}>
                {isMine
                  ? 'no new comments'
                  : `opened by ${entry.pullRequest.authorLogin ?? 'someone else'}${entry.pullRequest.baseBranch ? ` into ${entry.pullRequest.baseBranch}` : ''} — Hangar doesn't track comments on other people's PRs`}
              </div>
            )
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
                  <AccountPickerButton
                    label={busyCommentId === c.id ? 'Opening…' : 'Send to terminal'}
                    busy={busyCommentId === c.id}
                    title="Open a terminal on this PR's branch and hand the comments to Claude Code"
                    onPick={(accountId) => onSend(c, accountId)}
                  />
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
