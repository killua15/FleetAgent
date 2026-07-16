import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { Button } from './ui/button';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
import type { Project } from '../types/session';
// Shared with NewSessionControl/NewMultiSessionControl — both live in MainView, so this
// reuses that screen's own field/select/input styling rather than duplicating it.
import styles from '../screens/MainView.module.css';

interface ProjectPickerProps {
  projects: Project[];
  projectId: string;
  onProjectIdChange: (id: string) => void;
  newProjectName: string;
  onNewProjectNameChange: (value: string) => void;
  newProjectPath: string;
  onNewProjectPathChange: (value: string) => void;
}

/** Existing-project select + the "new project" name/path/Browse… sub-form, extracted so
 * both the single-session and bulk-session creation flows share the exact same picker. */
export function ProjectPicker({
  projects,
  projectId,
  onProjectIdChange,
  newProjectName,
  onNewProjectNameChange,
  newProjectPath,
  onNewProjectPathChange,
}: ProjectPickerProps) {
  const handleBrowse = async () => {
    const selected = await openDialog({ directory: true, multiple: false, title: 'Choose project folder' });
    if (typeof selected === 'string') {
      onNewProjectPathChange(selected);
    }
  };

  return (
    <>
      <div className={styles.field}>
        <Label className={styles.label}>PROJECT</Label>
        <Select value={projectId} onValueChange={onProjectIdChange}>
          <SelectTrigger className={styles.select}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="__new__">+ new project…</SelectItem>
            {projects.map((p) => (
              <SelectItem key={p.id} value={p.id}>
                {p.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {projectId === '__new__' && (
        <>
          <div className={styles.field}>
            <Label className={styles.label}>NAME</Label>
            <Input
              className={styles.textInput}
              value={newProjectName}
              onChange={(e) => onNewProjectNameChange(e.target.value)}
              placeholder="my-project"
            />
          </div>
          <div className={styles.field}>
            <Label className={styles.label}>PATH</Label>
            <div style={{ display: 'flex', gap: 6 }}>
              <Input
                className={styles.textInput}
                style={{ flex: 1 }}
                value={newProjectPath}
                onChange={(e) => onNewProjectPathChange(e.target.value)}
                placeholder="/Users/you/code/my-project"
              />
              <Button variant="outline" className={styles.createBtn} style={{ padding: '7px 10px' }} onClick={handleBrowse}>
                Browse…
              </Button>
            </div>
          </div>
        </>
      )}
    </>
  );
}
