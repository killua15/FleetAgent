import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { CodeView } from '../components/CodeView';
import { FileTreeRows } from '../components/FileTree';
import { Spinner } from '../components/Spinner';
import { toastError } from '../components/ErrorOverlay';
import type { FileTreeNode, Project } from '../types/session';
import styles from './Files.module.css';

/** What `read_file` returns: the file as it is now, plus `HEAD`'s version when
 * git has one. Both in one round trip so opening a modified file doesn't blink
 * through a diff-less state. */
interface FileContents {
  text: string;
  headText: string | null;
}

/** Browse and read any project's files, without needing an agent session open.
 *
 * The same tree the agent panel has, at the project level — which is where you
 * are when you want to check what the fleet did. Read-only on purpose: this is
 * for reviewing an agent's work, not for becoming an editor.
 */
export function Files() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [projectId, setProjectId] = useState<string | null>(null);
  const [tree, setTree] = useState<FileTreeNode[]>([]);
  const [loadingTree, setLoadingTree] = useState(false);
  const [collapsedPaths, setCollapsedPaths] = useState<Set<string>>(new Set());
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [contents, setContents] = useState<FileContents | null>(null);
  const [loadingFile, setLoadingFile] = useState(false);
  const [showDiff, setShowDiff] = useState(true);

  useEffect(() => {
    invoke<Project[]>('list_projects')
      .then((list) => {
        setProjects(list);
        setProjectId((prev) => prev ?? list[0]?.id ?? null);
      })
      .catch((e) => toastError(String(e)));
  }, []);

  // Switching project resets the file pane rather than leaving another
  // project's file on screen under the new tree.
  useEffect(() => {
    if (!projectId) return;
    setSelectedPath(null);
    setContents(null);
    setLoadingTree(true);
    invoke<FileTreeNode[]>('list_files', { target: { kind: 'project', id: projectId } })
      .then(setTree)
      .catch((e) => {
        setTree([]);
        toastError(String(e));
      })
      .finally(() => setLoadingTree(false));
  }, [projectId]);

  useEffect(() => {
    if (!projectId || !selectedPath) return;
    setLoadingFile(true);
    invoke<FileContents>('read_file', { target: { kind: 'project', id: projectId }, path: selectedPath })
      .then(setContents)
      .catch((e) => {
        setContents(null);
        toastError(String(e));
      })
      .finally(() => setLoadingFile(false));
  }, [projectId, selectedPath]);

  const toggleCollapsed = (path: string) =>
    setCollapsedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const project = projects.find((p) => p.id === projectId);
  const changed = contents?.headText != null && contents.headText !== contents.text;

  return (
    <div className={styles.screen}>
      <AppTopBar />
      <div className={styles.body}>
        <div className={styles.projectList}>
          <span className={styles.sectionLabel}>PROJECTS</span>
          {projects.map((p) => (
            <button key={p.id} className={p.id === projectId ? styles.projectRowActive : styles.projectRow} onClick={() => setProjectId(p.id)}>
              <span className={styles.projectName}>{p.name}</span>
              <span className={styles.projectPath}>{p.path}</span>
            </button>
          ))}
          {projects.length === 0 && <span className={styles.dimText}>No projects yet.</span>}
        </div>

        <div className={styles.tree}>
          <span className={styles.sectionLabel}>{project ? project.name.toUpperCase() : 'FILES'}</span>
          {loadingTree && <div className={styles.dimText}><Spinner label="reading…" /></div>}
          {!loadingTree && tree.length === 0 && <span className={styles.dimText}>no files</span>}
          {!loadingTree && (
            <FileTreeRows
              nodes={tree}
              selectedPath={selectedPath}
              collapsedPaths={collapsedPaths}
              onToggleCollapsed={toggleCollapsed}
              onSelectFile={setSelectedPath}
            />
          )}
        </div>

        <div className={styles.content}>
          {selectedPath ? (
            <>
              <div className={styles.contentHeader}>
                <span className={styles.filePath}>{selectedPath}</span>
                <span className={styles.spacer} />
                {/* Only offered when there's a committed version to compare
                    against — a new file has nothing to diff. */}
                {changed && (
                  <div className={styles.tabs}>
                    <button className={showDiff ? styles.tabActive : styles.tab} onClick={() => setShowDiff(true)}>
                      diff
                    </button>
                    <button className={!showDiff ? styles.tabActive : styles.tab} onClick={() => setShowDiff(false)}>
                      file
                    </button>
                  </div>
                )}
                {!changed && contents && <span className={styles.cleanTag}>{contents.headText == null ? 'untracked' : 'unchanged'}</span>}
              </div>
              {loadingFile && <div className={styles.dimText}><Spinner label="loading…" /></div>}
              {!loadingFile && contents && (
                <CodeView
                  // Keyed so switching file or view rebuilds the editor rather
                  // than reusing one configured for the previous document.
                  key={`${selectedPath}:${showDiff && changed}`}
                  path={selectedPath}
                  text={contents.text}
                  original={showDiff && changed ? contents.headText : null}
                />
              )}
            </>
          ) : (
            <span className={styles.dimText}>Pick a file to read it. Modified files open on their diff against HEAD.</span>
          )}
        </div>
      </div>
    </div>
  );
}
