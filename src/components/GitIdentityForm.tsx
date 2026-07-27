import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { toastError } from './ErrorOverlay';
import { Button } from './ui/button';
import styles from './GitIdentityForm.module.css';

/** What the backend returns. Note what's missing: the token. It's stored in the
 * OS keychain and never travels back, so it can't be rendered into the DOM,
 * logged, or end up in a screenshot. */
interface GitIdentity {
  projectId: string;
  userName: string | null;
  userEmail: string | null;
  authUser: string | null;
  hasToken: boolean;
}

/**
 * Which git identity and credential this project's repo needs.
 *
 * Exists because the default on a machine is one person, and a repo belonging
 * to another account needs another — and until this existed Hangar committed
 * with whatever `git config --global user.name` said, into permanent history.
 *
 * Three things, kept apart because they fail differently: the identity decides
 * who authored a commit, the token authenticates the push, and the same token
 * is what every PR feature needs, since those all run through `gh` and it has a
 * single logged-in account.
 */
export function GitIdentityForm({ projectId, projectName }: { projectId: string; projectName: string }) {
  const [identity, setIdentity] = useState<GitIdentity | null>(null);
  const [open, setOpen] = useState(false);
  const [userName, setUserName] = useState('');
  const [userEmail, setUserEmail] = useState('');
  const [authUser, setAuthUser] = useState('');
  const [token, setToken] = useState('');
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<GitIdentity | null>('get_project_git_identity', { projectId })
      .then((loaded) => {
        setIdentity(loaded);
        setUserName(loaded?.userName ?? '');
        setUserEmail(loaded?.userEmail ?? '');
        setAuthUser(loaded?.authUser ?? '');
        setToken('');
      })
      .catch(() => setIdentity(null));
  }, [projectId]);

  const save = async () => {
    setSaving(true);
    try {
      const saved = await invoke<GitIdentity>('set_project_git_identity', {
        projectId,
        userName: userName || null,
        userEmail: userEmail || null,
        authUser: authUser || null,
        // Empty means "leave the stored one alone", not "delete it" — the form
        // never receives the token back, so an empty field is the normal state
        // whenever someone edits the email.
        token: token || null,
      });
      setIdentity(saved);
      setToken('');
      setOpen(false);
    } catch (e) {
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const clear = async () => {
    try {
      await invoke('clear_project_git_identity', { projectId });
      setIdentity(null);
      setUserName('');
      setUserEmail('');
      setAuthUser('');
      setToken('');
      setOpen(false);
    } catch (e) {
      toastError(String(e));
    }
  };

  const summary = identity?.userEmail ?? identity?.userName;

  if (!open) {
    return (
      <button className={styles.summary} onClick={() => setOpen(true)}>
        <span className={styles.summaryLabel}>git identity</span>
        {summary ? (
          <span className={styles.summaryValue}>
            {summary}
            {identity?.hasToken && <span className={styles.tokenTag}>token stored</span>}
          </span>
        ) : (
          <span className={styles.summaryEmpty}>this machine's default — set one if {projectName} belongs to another account</span>
        )}
      </button>
    );
  }

  return (
    <div className={styles.form}>
      <span className={styles.title}>Git identity for {projectName}</span>
      <div className={styles.row}>
        <label className={styles.field}>
          name
          <input className={styles.input} value={userName} onChange={(e) => setUserName(e.target.value)} placeholder="Ada Lovelace" />
        </label>
        <label className={styles.field}>
          email
          <input className={styles.input} value={userEmail} onChange={(e) => setUserEmail(e.target.value)} placeholder="ada@work.example" />
        </label>
      </div>
      <span className={styles.hint}>Written into each worktree's own git config, so the agent's commits carry it too — not just Hangar's.</span>

      <div className={styles.row}>
        <label className={styles.field}>
          username for pushes
          <input className={styles.input} value={authUser} onChange={(e) => setAuthUser(e.target.value)} placeholder="your-github-login" />
        </label>
        <label className={styles.field}>
          token
          <input
            className={styles.input}
            type="password"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            placeholder={identity?.hasToken ? '•••••••• (stored — leave blank to keep)' : 'personal access token'}
          />
        </label>
      </div>
      <span className={styles.hint}>
        Stored in the macOS keychain, never in the database and never in <code>.git/config</code> or the remote URL — a token in the
        URL ends up in <code>git remote -v</code> and in every error message that quotes it back. Also used as <code>GH_TOKEN</code>,
        which is what the PR inbox runs on.
      </span>

      <div className={styles.actions}>
        {identity && (
          <button className={styles.clearLink} onClick={clear}>
            remove identity and token
          </button>
        )}
        <span className={styles.spacer} />
        <Button size="sm" variant="outline" onClick={() => setOpen(false)}>
          cancel
        </Button>
        <Button size="sm" disabled={saving} onClick={save}>
          {saving ? 'saving…' : 'Save'}
        </Button>
      </div>
    </div>
  );
}
