import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { SettingsTabs } from '../components/SettingsTabs';
import { Button } from '../components/ui/button';
import { Checkbox } from '../components/ui/checkbox';
import { Input } from '../components/ui/input';
import { Textarea } from '../components/ui/textarea';
import type { GroupMembership, MemoryFile, MemoryFileSummary, MemoryGroupSummary } from '../types/settings';
import styles from './SharedMemorySettings.module.css';

export function SharedMemorySettings() {
  const [groups, setGroups] = useState<MemoryGroupSummary[]>([]);
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [membership, setMembership] = useState<GroupMembership[]>([]);
  const [files, setFiles] = useState<MemoryFileSummary[]>([]);
  const [selectedFileId, setSelectedFileId] = useState<string | null>(null);
  const [openFile, setOpenFile] = useState<MemoryFile | null>(null);
  const [draftContent, setDraftContent] = useState('');
  const [newGroupName, setNewGroupName] = useState('');
  const [savingContent, setSavingContent] = useState(false);
  const [newFileName, setNewFileName] = useState('');

  const loadGroups = () => invoke<MemoryGroupSummary[]>('list_memory_groups').then(setGroups);

  useEffect(() => {
    loadGroups();
  }, []);

  useEffect(() => {
    if (!selectedGroupId && groups.length > 0) setSelectedGroupId(groups[0].id);
  }, [groups, selectedGroupId]);

  useEffect(() => {
    if (!selectedGroupId) return;
    invoke<GroupMembership[]>('list_group_membership', { groupId: selectedGroupId }).then(setMembership);
    invoke<MemoryFileSummary[]>('list_group_memory_files', { groupId: selectedGroupId }).then((list) => {
      setFiles(list);
      setSelectedFileId(list[0]?.id ?? null);
    });
  }, [selectedGroupId]);

  useEffect(() => {
    if (!selectedFileId) {
      setOpenFile(null);
      return;
    }
    invoke<MemoryFile>('get_memory_file', { fileId: selectedFileId }).then((f) => {
      setOpenFile(f);
      setDraftContent(f.contentMd);
    });
  }, [selectedFileId]);

  const toggleMember = async (projectId: string, member: boolean) => {
    if (!selectedGroupId) return;
    setMembership((prev) => prev.map((m) => (m.projectId === projectId ? { ...m, member } : m)));
    await invoke('set_group_membership', { groupId: selectedGroupId, projectId, member });
    invoke<MemoryFileSummary[]>('list_group_memory_files', { groupId: selectedGroupId }).then(setFiles);
    loadGroups();
  };

  const createGroup = async () => {
    const name = newGroupName.trim();
    if (!name) return;
    const group = await invoke<{ id: string }>('create_memory_group', { name });
    setNewGroupName('');
    await loadGroups();
    setSelectedGroupId(group.id);
  };

  const createFile = async () => {
    const filename = newFileName.trim();
    const firstMember = membership.find((m) => m.member);
    if (!filename || !firstMember || !selectedGroupId) return;
    const file = await invoke<MemoryFile>('create_memory_file', { projectId: firstMember.projectId, filename, contentMd: '' });
    setNewFileName('');
    const list = await invoke<MemoryFileSummary[]>('list_group_memory_files', { groupId: selectedGroupId });
    setFiles(list);
    setSelectedFileId(file.id);
    loadGroups();
  };

  const saveContent = async () => {
    if (!openFile) return;
    setSavingContent(true);
    try {
      await invoke('update_memory_file_content', { fileId: openFile.id, contentMd: draftContent });
      setOpenFile({ ...openFile, contentMd: draftContent });
    } finally {
      setSavingContent(false);
    }
  };

  const selectedGroup = groups.find((g) => g.id === selectedGroupId);
  const crossScopeWarn = membership.filter((m) => m.member).length > 1 && new Set(membership.filter((m) => m.member).map((m) => m.projectId)).size > 1;

  return (
    <div className={styles.screen}>
      <SettingsTabs
        active="shared-memory"
        title="Shared memory"
        subtitle="projects in the same group share context & memory files across sessions."
      />
      <div className={styles.body}>
        <div className={styles.groupList}>
          <div className={styles.groupListHeader}>
            <span className={styles.sectionLabel}>MEMORY GROUPS</span>
          </div>
          {groups.map((g) => (
            <button
              key={g.id}
              className={g.id === selectedGroupId ? styles.groupRowActive : styles.groupRow}
              onClick={() => setSelectedGroupId(g.id)}
            >
              <span className={styles.groupDot} />
              <span className={styles.groupInfo}>
                <span className={styles.groupName}>{g.name}</span>
                <span className={styles.groupSub}>
                  {g.projectCount} projects · {g.fileCount} memory files
                </span>
              </span>
            </button>
          ))}
          <div className={styles.newGroupRow}>
            <Input
              value={newGroupName}
              onChange={(e) => setNewGroupName(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && createGroup()}
              placeholder="new group name…"
              className={styles.newGroupInput}
            />
            <Button variant="outline" onClick={createGroup} title="New group">
              +
            </Button>
          </div>
          <div className={styles.spacer} />
          <p className={styles.footnote}>A project can belong to several groups. Agents read the union of its groups' memory.</p>
        </div>

        <div className={styles.membership}>
          {selectedGroup && (
            <>
              <span className={styles.membershipTitle}>{selectedGroup.name}</span>
              <span className={styles.membershipHint}>Projects sharing this memory pool:</span>
              {membership.map((m) => (
                <label key={m.projectId} className={m.member ? styles.memberRowActive : styles.memberRow}>
                  <Checkbox checked={m.member} onCheckedChange={(checked) => toggleMember(m.projectId, checked === true)} />
                  <span className={styles.memberIcon}>{m.projectName[0]?.toUpperCase()}</span>
                  <span className={styles.memberName}>{m.projectName}</span>
                </label>
              ))}
              {crossScopeWarn && (
                <div className={styles.crossWarn}>
                  <span>⚠</span>
                  <span>This group mixes several projects — make sure sharing memory across them is intended.</span>
                </div>
              )}
            </>
          )}
        </div>

        <div className={styles.viewerWrap}>
          <div className={styles.fileList}>
            <span className={styles.sectionLabel}>MEMORY FILES — POOL</span>
            {membership.some((m) => m.member) && (
              <div className={styles.newFileRow}>
                <Input
                  value={newFileName}
                  onChange={(e) => setNewFileName(e.target.value)}
                  onKeyDown={(e) => e.key === 'Enter' && createFile()}
                  placeholder="notes.md"
                  className={styles.newFileInput}
                />
                <Button variant="outline" onClick={createFile} title="New memory file">
                  +
                </Button>
              </div>
            )}
            {files.map((f) => (
              <button
                key={f.id}
                className={f.id === selectedFileId ? styles.fileRowActive : styles.fileRow}
                onClick={() => setSelectedFileId(f.id)}
              >
                <span className={styles.fileGlyph}>M↓</span>
                <span className={styles.fileInfo}>
                  <span className={styles.fileName}>{f.filename}</span>
                  <span className={styles.fileMeta}>
                    {f.projectName} · {new Date(f.updatedAt).toLocaleDateString()}
                  </span>
                </span>
              </button>
            ))}
            {files.length === 0 && <span className={styles.dimText}>No memory files in this group yet.</span>}
          </div>
          <div className={styles.viewer}>
            {openFile ? (
              <>
                <div className={styles.viewerHeader}>
                  <span className={styles.viewerTitle}>
                    {openFile.projectName} / memory / {openFile.filename}
                  </span>
                  <span className={styles.spacer} />
                  <span className={styles.sharedTag}>shared · read/write</span>
                </div>
                <Textarea
                  className={styles.viewerContent}
                  value={draftContent}
                  onChange={(e) => setDraftContent(e.target.value)}
                  onBlur={saveContent}
                  disabled={savingContent}
                />
              </>
            ) : (
              <div className={styles.viewerEmpty}>Select a memory file to view/edit it.</div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
