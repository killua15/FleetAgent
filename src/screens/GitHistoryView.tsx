import { useEffect, useMemo, useState } from 'react';
import { invoke } from '../lib/invoke';
import { Button } from '../components/ui/button';
import { Spinner } from '../components/Spinner';
import { toastError, toastSuccess } from '../components/ErrorOverlay';
import { diffLines } from '../lib/diffLines';
import { DiffLinesView } from './AgentPanel';
import { layoutCommitGraph, laneColor, type GraphRow } from '../lib/commitGraph';
import { FILE_STATUS_COLOR, FILE_STATUS_LABEL } from '../lib/fileStatus';
import { useNavigation } from '../nav/NavigationProvider';
import type { BranchInfo, CommitFileChange, CommitFileDiff, CommitInfo, CommitLogPage } from '../types/gitHistory';
import styles from './GitHistoryView.module.css';

const PAGE_SIZE = 50;
const LANE_WIDTH = 16;
const ROW_HEIGHT = 40;

export function GitHistoryView() {
  const { back, canGoBack, current } = useNavigation();
  const params = current.params as { path?: string; label?: string } | undefined;
  const path = params?.path ?? '';
  const label = params?.label ?? path;

  const [branches, setBranches] = useState<BranchInfo[]>([]);
  const [loadingBranches, setLoadingBranches] = useState(true);
  const [selectedBranch, setSelectedBranch] = useState<string | null>(null);
  const [checkingOut, setCheckingOut] = useState<string | null>(null);

  const [commits, setCommits] = useState<CommitInfo[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loadingCommits, setLoadingCommits] = useState(true);
  const [selectedHash, setSelectedHash] = useState<string | null>(null);

  const [commitFiles, setCommitFiles] = useState<CommitFileChange[]>([]);
  const [loadingFiles, setLoadingFiles] = useState(false);
  const [selectedFilePath, setSelectedFilePath] = useState<string | null>(null);
  const [fileDiff, setFileDiff] = useState<CommitFileDiff | null>(null);
  const [loadingDiff, setLoadingDiff] = useState(false);

  const loadBranches = async () => {
    setLoadingBranches(true);
    try {
      setBranches(await invoke<BranchInfo[]>('list_branches', { path }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setLoadingBranches(false);
    }
  };

  useEffect(() => {
    loadBranches();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path]);

  useEffect(() => {
    setSelectedHash(null);
    setLoadingCommits(true);
    invoke<CommitLogPage>('list_commits', { path, branch: selectedBranch, skip: 0, limit: PAGE_SIZE })
      .then((page) => {
        setCommits(page.commits);
        setHasMore(page.hasMore);
      })
      .catch((e) => toastError(String(e)))
      .finally(() => setLoadingCommits(false));
  }, [path, selectedBranch]);

  const loadMore = async () => {
    setLoadingCommits(true);
    try {
      const page = await invoke<CommitLogPage>('list_commits', { path, branch: selectedBranch, skip: commits.length, limit: PAGE_SIZE });
      setCommits((prev) => [...prev, ...page.commits]);
      setHasMore(page.hasMore);
    } catch (e) {
      toastError(String(e));
    } finally {
      setLoadingCommits(false);
    }
  };

  useEffect(() => {
    setSelectedFilePath(null);
    setFileDiff(null);
    if (!selectedHash) {
      setCommitFiles([]);
      return;
    }
    setLoadingFiles(true);
    invoke<CommitFileChange[]>('get_commit_files', { path, hash: selectedHash })
      .then(setCommitFiles)
      .catch((e) => toastError(String(e)))
      .finally(() => setLoadingFiles(false));
  }, [path, selectedHash]);

  useEffect(() => {
    if (!selectedHash || !selectedFilePath) {
      setFileDiff(null);
      return;
    }
    setLoadingDiff(true);
    invoke<CommitFileDiff>('get_commit_file_diff', { path, hash: selectedHash, filePath: selectedFilePath })
      .then(setFileDiff)
      .catch((e) => toastError(String(e)))
      .finally(() => setLoadingDiff(false));
  }, [path, selectedHash, selectedFilePath]);

  const handleCheckout = async (branch: BranchInfo) => {
    setCheckingOut(branch.name);
    try {
      await invoke('checkout_branch', { path, branch: branch.name, isRemote: branch.isRemote });
      await loadBranches();
      toastSuccess(`Checked out ${branch.name}`);
    } catch (e) {
      toastError(String(e));
    } finally {
      setCheckingOut(null);
    }
  };

  const graphRows = useMemo(() => layoutCommitGraph(commits), [commits]);
  const maxLanes = Math.max(1, ...graphRows.map((r) => r.laneCount));
  const graphWidth = maxLanes * LANE_WIDTH;

  const localBranches = branches.filter((b) => !b.isRemote);
  const remoteBranches = branches.filter((b) => b.isRemote);
  const selectedCommit = commits.find((c) => c.hash === selectedHash) ?? null;

  return (
    <div className={styles.screen}>
      <div className={styles.header}>
        {canGoBack && (
          <Button variant="outline" className={styles.backBtn} onClick={back}>
            ← back
          </Button>
        )}
        <span className={styles.title}>{label}</span>
        <span className={styles.subtitle}>{path}</span>
      </div>

      <div className={styles.body}>
        <div className={styles.branchList}>
          <button
            className={selectedBranch === null ? styles.branchRowActive : styles.branchRow}
            onClick={() => setSelectedBranch(null)}
          >
            <span className={styles.branchName}>All branches</span>
          </button>

          <span className={styles.sectionLabel}>LOCAL</span>
          {loadingBranches && <Spinner label="loading…" />}
          {localBranches.map((b) => (
            <BranchRow key={b.name} branch={b} selected={selectedBranch === b.name} checkingOut={checkingOut} onSelect={setSelectedBranch} onCheckout={handleCheckout} />
          ))}

          <span className={styles.sectionLabel}>REMOTE</span>
          {remoteBranches.map((b) => (
            <BranchRow key={b.name} branch={b} selected={selectedBranch === b.name} checkingOut={checkingOut} onSelect={setSelectedBranch} onCheckout={handleCheckout} />
          ))}
        </div>

        <div className={styles.graphList}>
          {loadingCommits && commits.length === 0 ? (
            <div className={styles.dimText}>
              <Spinner label="loading history…" />
            </div>
          ) : (
            commits.map((commit, i) => (
              <CommitRow
                key={commit.hash}
                commit={commit}
                row={graphRows[i]}
                graphWidth={graphWidth}
                selected={commit.hash === selectedHash}
                onSelect={() => setSelectedHash(commit.hash)}
              />
            ))
          )}
          {!loadingCommits && commits.length === 0 && <span className={styles.dimText}>No commits found.</span>}
          {hasMore && (
            <div className={styles.loadMoreRow}>
              <Button size="sm" variant="outline" disabled={loadingCommits} onClick={loadMore}>
                {loadingCommits ? <Spinner label="loading…" /> : 'Load more'}
              </Button>
            </div>
          )}
        </div>

        <div className={styles.detail}>
          {selectedCommit ? (
            <>
              <div className={styles.commitDetailHeader}>
                <span className={styles.commitSubjectFull}>{selectedCommit.subject}</span>
                <span className={styles.commitMetaLine}>
                  {selectedCommit.author} · {new Date(selectedCommit.date).toLocaleString()}
                </span>
                <span className={styles.commitHashFull}>{selectedCommit.hash}</span>
              </div>

              <span className={styles.sectionLabel}>FILES CHANGED</span>
              {loadingFiles && (
                <div className={styles.dimText}>
                  <Spinner label="loading files…" />
                </div>
              )}
              {!loadingFiles && commitFiles.length === 0 && <span className={styles.dimText}>No file changes (or a merge commit's combined diff, which isn't shown).</span>}
              <div className={styles.fileList}>
                {commitFiles.map((f) => (
                  <button
                    key={f.path}
                    className={f.path === selectedFilePath ? styles.fileRowActive : styles.fileRow}
                    onClick={() => setSelectedFilePath(f.path)}
                    title={FILE_STATUS_LABEL[f.status]}
                  >
                    <span className={styles.fileStatusDot} style={{ background: FILE_STATUS_COLOR[f.status] }} />
                    <span className={styles.filePath}>{f.path}</span>
                  </button>
                ))}
              </div>

              {selectedFilePath && (
                <div className={styles.diffPanel}>
                  {loadingDiff ? (
                    <div className={styles.dimText}>
                      <Spinner label="loading diff…" />
                    </div>
                  ) : fileDiff ? (
                    <DiffLinesView lines={diffLines(fileDiff.oldText, fileDiff.newText)} />
                  ) : null}
                </div>
              )}
            </>
          ) : (
            <span className={styles.dimText}>Select a commit to see its changed files.</span>
          )}
        </div>
      </div>
    </div>
  );
}

function BranchRow({
  branch,
  selected,
  checkingOut,
  onSelect,
  onCheckout,
}: {
  branch: BranchInfo;
  selected: boolean;
  checkingOut: string | null;
  onSelect: (name: string) => void;
  onCheckout: (branch: BranchInfo) => void;
}) {
  return (
    <div className={selected ? styles.branchRowActive : styles.branchRow}>
      <button className={styles.branchRowMain} onClick={() => onSelect(branch.name)}>
        {branch.isCurrent && <span className={styles.currentDot} title="Current branch" />}
        <span className={styles.branchName}>{branch.name}</span>
        {(branch.ahead > 0 || branch.behind > 0) && (
          <span className={styles.aheadBehind}>
            {branch.ahead > 0 && <span className={styles.ahead}>↑{branch.ahead}</span>}
            {branch.behind > 0 && <span className={styles.behind}>↓{branch.behind}</span>}
          </span>
        )}
      </button>
      {!branch.isCurrent && (
        <Button size="sm" variant="outline" disabled={checkingOut === branch.name} onClick={() => onCheckout(branch)}>
          {checkingOut === branch.name ? <Spinner label="…" /> : 'Checkout'}
        </Button>
      )}
    </div>
  );
}

function CommitRow({
  commit,
  row,
  graphWidth,
  selected,
  onSelect,
}: {
  commit: CommitInfo;
  row: GraphRow | undefined;
  graphWidth: number;
  selected: boolean;
  onSelect: () => void;
}) {
  const cx = (lane: number) => lane * LANE_WIDTH + LANE_WIDTH / 2;
  const midY = ROW_HEIGHT / 2;

  return (
    <button className={selected ? styles.commitRowActive : styles.commitRow} onClick={onSelect}>
      <svg width={graphWidth} height={ROW_HEIGHT} className={styles.graphSvg}>
        {row?.passthroughLanes.map((lane) => (
          <line key={`pt-${lane}`} x1={cx(lane)} y1={0} x2={cx(lane)} y2={ROW_HEIGHT} stroke={laneColor(lane)} strokeWidth={1.5} />
        ))}
        {row?.mergeFromLanes.map((lane) => (
          <line key={`mf-${lane}`} x1={cx(lane)} y1={0} x2={cx(row.lane)} y2={midY} stroke={laneColor(lane)} strokeWidth={1.5} />
        ))}
        {row?.parentLanes.map((lane) => (
          <line key={`pl-${lane}`} x1={cx(row.lane)} y1={midY} x2={cx(lane)} y2={ROW_HEIGHT} stroke={laneColor(row.lane)} strokeWidth={1.5} />
        ))}
        {row && <circle cx={cx(row.lane)} cy={midY} r={4} fill={laneColor(row.lane)} />}
      </svg>
      <span className={styles.commitHash}>{commit.shortHash}</span>
      <span className={styles.commitSubject}>{commit.subject}</span>
      {commit.refs.map((ref) => (
        <span key={ref} className={styles.refPill}>
          {ref}
        </span>
      ))}
      <span className={styles.spacer} />
      <span className={styles.commitAuthor}>{commit.author}</span>
      <span className={styles.commitDate}>{new Date(commit.date).toLocaleDateString()}</span>
    </button>
  );
}
