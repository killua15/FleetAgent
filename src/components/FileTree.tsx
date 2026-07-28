import { fileIcon } from '../lib/fileIcon';
import { FILE_STATUS_COLOR, FILE_STATUS_LABEL } from '../lib/fileStatus';
import type { FileTreeNode } from '../types/session';
import styles from './FileTree.module.css';

/** The file tree, shared by the agent panel and the Files screen.
 *
 * Extracted rather than copied: two trees would drift the first time one of them
 * learned something the other didn't (a status colour, a collapsed-state rule),
 * and the tree is the part of both screens people actually navigate by.
 */
export function FileTreeRows({
  nodes,
  selectedPath,
  collapsedPaths,
  onToggleCollapsed,
  onSelectFile,
  depth = 0,
}: {
  nodes: FileTreeNode[];
  selectedPath: string | null;
  collapsedPaths: Set<string>;
  onToggleCollapsed: (path: string) => void;
  onSelectFile: (path: string) => void;
  depth?: number;
}) {
  return (
    <>
      {nodes.map((node) => {
        const isCollapsed = node.isDir && collapsedPaths.has(node.path);
        const icon = node.isDir ? null : fileIcon(node.name);
        return (
          <div key={node.path}>
            <button
              type="button"
              className={styles.treeRow}
              onClick={() => (node.isDir ? onToggleCollapsed(node.path) : onSelectFile(node.path))}
              title={node.status ? `${node.path} — ${FILE_STATUS_LABEL[node.status]}` : node.path}
              style={{ background: selectedPath === node.path ? 'var(--panel2)' : 'none' }}
            >
              {Array.from({ length: depth }).map((_, i) => (
                <span key={i} className={styles.indentGuide} />
              ))}
              {node.isDir ? (
                <span className={styles.treeChevron}>{isCollapsed ? '▸' : '▾'}</span>
              ) : (
                <span className={styles.fileIconBadge} style={{ color: icon!.color }}>
                  {icon!.letter}
                </span>
              )}
              <span className={styles.treeName} style={{ color: node.isDir ? 'var(--mut)' : 'var(--txt)' }}>
                {node.name}
              </span>
              {!node.isDir && node.status && <span className={styles.statusDot} style={{ background: FILE_STATUS_COLOR[node.status] }} />}
            </button>
            {node.isDir && !isCollapsed && node.children.length > 0 && (
              <FileTreeRows
                nodes={node.children}
                selectedPath={selectedPath}
                collapsedPaths={collapsedPaths}
                onToggleCollapsed={onToggleCollapsed}
                onSelectFile={onSelectFile}
                depth={depth + 1}
              />
            )}
          </div>
        );
      })}
    </>
  );
}
