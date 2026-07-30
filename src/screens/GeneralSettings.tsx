import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { toastError } from '../components/ErrorOverlay';
import { check } from '@tauri-apps/plugin-updater';
import { PairPhoneDialog } from '../components/PairPhoneDialog';
import { getVersion } from '@tauri-apps/api/app';
import { SettingsTabs } from '../components/SettingsTabs';
import { UsageBar } from '../components/UsageBar';
import { PickConfigDirDialog } from '../components/PickConfigDirDialog';
import { Button } from '../components/ui/button';
import { Input } from '../components/ui/input';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Checkbox } from '../components/ui/checkbox';
import { contextSeverity, contextSeverityColor, formatUsd } from '../lib/tokenBudget';
import { useRealUsage } from '../lib/useRealUsage';
import { useTheme, type Theme } from '../theme/ThemeProvider';
import type { Account } from '../types/account';
import type { AccountUsageSummary } from '../types/session';
import type { AppSettings } from '../types/settings';
import styles from './GeneralSettings.module.css';

const THEME_OPTS: Theme[] = ['dark', 'light', 'glass', 'system'];

/** `/Users/killua/.claude-work` → `~/.claude-work`. macOS-only, like the rest
 * of Hangar — the home prefix is the same on every row, so it's noise. */
function abbreviateHome(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, '~');
}

/** Hangar's own login wizard parks a login in `pending-logins/<uuid>` and keeps
 * that as the account's home. It's the signature of an account not using one
 * of the user's own `~/.claude-*` directories — the reason its terminal
 * command shows an unrecognizable path. */
function isWizardConfigDir(path: string): boolean {
  return path.includes('/pending-logins/');
}

export function GeneralSettings() {
  const { theme, resolvedTheme, setTheme, unfocusedTint, setUnfocusedTint } = useTheme();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [usage, setUsage] = useState<Record<string, AccountUsageSummary>>({});
  const [caps, setCaps] = useState<Record<string, string>>({});
  const [savingCap, setSavingCap] = useState<string | null>(null);
  const [appSettings, setAppSettingsState] = useState<AppSettings>({
    reviewerEnabled: false,
    reviewerAccountId: null,
    notifyNeedsApproval: false,
    notifySessionWaiting: false,
    notifyAccountExhausted: false,
    notifySpendCap: false,
    notifyPrComments: false,
    agentTreesEnabled: false,
    includeOwnPrComments: false,
  });
  const [savingReviewer, setSavingReviewer] = useState(false);

  const loadAccounts = () =>
    invoke<Account[]>('list_accounts').then((accs) => {
      setAccounts(accs);
      setCaps(Object.fromEntries(accs.map((a) => [a.id, a.monthlyCap != null ? String(a.monthlyCap) : ''])));
    });

  useEffect(() => {
    loadAccounts();
    invoke<AppSettings>('get_app_settings').then(setAppSettingsState);
  }, []);

  useEffect(() => {
    if (accounts.length === 0) return;
    const sevenDaysAgo = new Date(Date.now() - 7 * 24 * 60 * 60 * 1000).toISOString();
    Promise.all(
      accounts.map((acc) =>
        invoke<AccountUsageSummary>('get_account_usage_summary', { accountId: acc.id, sinceIso: sevenDaysAgo })
          .then((summary) => [acc.id, summary] as const)
          .catch(() => [acc.id, { inputTokens: 0, outputTokens: 0, costUsd: 0 }] as const),
      ),
    ).then((entries) => setUsage(Object.fromEntries(entries)));
  }, [accounts]);

  const saveCap = async (accountId: string) => {
    const raw = caps[accountId]?.trim() ?? '';
    const monthlyCap = raw === '' ? null : Number(raw);
    if (monthlyCap != null && Number.isNaN(monthlyCap)) return;
    setSavingCap(accountId);
    try {
      const updated = await invoke<Account>('set_account_monthly_cap', { id: accountId, monthlyCap });
      setAccounts((prev) => prev.map((a) => (a.id === accountId ? updated : a)));
    } finally {
      setSavingCap(null);
    }
  };

  const saveReviewer = async (next: AppSettings) => {
    setSavingReviewer(true);
    try {
      const saved = await invoke<AppSettings>('set_app_settings', { input: next });
      setAppSettingsState(saved);
    } finally {
      setSavingReviewer(false);
    }
  };

  const [savingNotif, setSavingNotif] = useState<keyof AppSettings | null>(null);
  const toggleNotification = async (key: keyof AppSettings, value: boolean) => {
    setSavingNotif(key);
    try {
      const saved = await invoke<AppSettings>('set_app_settings', { input: { ...appSettings, [key]: value } });
      setAppSettingsState(saved);
    } finally {
      setSavingNotif(null);
    }
  };

  return (
    <div className={styles.screen}>
      <SettingsTabs active="general-settings" title="General" />
      <div className={styles.body}>
        <section className={styles.section}>
          <span className={styles.sectionLabel}>APPEARANCE</span>
          <div className={styles.themeRow}>
            {THEME_OPTS.map((t) => (
              <button
                key={t}
                className={t === theme ? styles.themeBtnActive : styles.themeBtn}
                onClick={() => setTheme(t)}
              >
                {t}
              </button>
            ))}
          </div>
          {/* Only meaningful in glass, and hiding it elsewhere is better than
              showing a slider that does nothing. */}
          {resolvedTheme === 'glass' && (
            <div className={styles.tintRow}>
              <span className={styles.tintLabel}>Transparency when Hangar isn't focused</span>
              <input
                type="range"
                min={0.1}
                max={1}
                step={0.02}
                value={unfocusedTint}
                onChange={(e) => setUnfocusedTint(Number(e.target.value))}
                className={styles.tintSlider}
              />
              <span className={styles.tintValue}>{unfocusedTint >= 0.99 ? 'no fade' : `${Math.round((1 - unfocusedTint) * 100)}% see-through`}</span>
            </div>
          )}
          {resolvedTheme === 'glass' && (
            <span className={styles.hint}>
              The window fades to this while you're in another app and comes back when you return. Popovers, dialogs and the
              command palette stay solid at every setting — anything you have to read shouldn't have your wallpaper behind it.
            </span>
          )}
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>ACCOUNT LOGIN — CONFIG DIRECTORY</span>
          <span className={styles.hint}>
            Each subscription account runs the CLI against an isolated <code>CLAUDE_CONFIG_DIR</code>. Point it at a login you
            already have (e.g. <code>~/.claude-work</code>) instead of the one Hangar created — nothing is moved, and the
            account keeps its sessions and history.
          </span>
          <div className={styles.capList}>
            {accounts
              .filter((acc) => acc.provider === 'claude' && acc.billing === 'sub')
              .map((acc) => (
                <div className={styles.capRow} key={acc.id}>
                  <ProviderBadge account={acc} />
                  <span className={styles.capName}>{acc.name}</span>
                  <code
                    className={styles.dimText}
                    title={acc.configDirPath}
                    style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontFamily: 'var(--font-mono)' }}
                  >
                    {abbreviateHome(acc.configDirPath)}
                  </code>
                  <PickConfigDirDialog
                    trigger={
                      <Button
                        variant="outline"
                        size="sm"
                        style={isWizardConfigDir(acc.configDirPath) ? { color: 'var(--amber)', borderColor: 'var(--amber)' } : undefined}
                        title="Point this account at a login you already have"
                      >
                        change…
                      </Button>
                    }
                    title={`Config directory for ${acc.name}`}
                    description="Pick a login already on this machine. Hangar adopts it in place — nothing is moved or copied, and the account's sessions and history stay intact."
                    onPick={async (login) => {
                      await invoke('set_account_config_dir', { id: acc.id, configDirPath: login.path });
                      await loadAccounts();
                    }}
                  />
                </div>
              ))}
            {accounts.filter((acc) => acc.provider === 'claude' && acc.billing === 'sub').length === 0 && (
              <span className={styles.empty}>No subscription accounts yet.</span>
            )}
          </div>
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>SPEND CAP PER ACCOUNT — MONTHLY</span>
          <div className={styles.capList}>
            {accounts.map((acc) => (
              <AccountCapRow
                key={acc.id}
                account={acc}
                usage={usage[acc.id]}
                capInput={caps[acc.id] ?? ''}
                onCapInputChange={(v) => setCaps((prev) => ({ ...prev, [acc.id]: v }))}
                onSave={() => saveCap(acc.id)}
                saving={savingCap === acc.id}
              />
            ))}
            {accounts.length === 0 && <span className={styles.empty}>No accounts yet.</span>}
          </div>
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>PREFERRED REVIEWER ACCOUNT</span>
          <span className={styles.hint}>
            Used by the "Review now" button on any session — spawns a plan-mode session with this account to report
            issues without editing anything.
          </span>
          <label className={styles.reviewerToggle}>
            <Checkbox
              checked={appSettings.reviewerEnabled}
              onCheckedChange={(checked) => saveReviewer({ ...appSettings, reviewerEnabled: checked === true })}
              disabled={savingReviewer}
            />
            Enable reviewer
          </label>
          <Select
            value={appSettings.reviewerAccountId ?? ''}
            onValueChange={(v) => saveReviewer({ ...appSettings, reviewerAccountId: v || null })}
            disabled={!appSettings.reviewerEnabled || savingReviewer}
          >
            <SelectTrigger className={styles.reviewerSelect}>
              <SelectValue placeholder="select a reviewer account…" />
            </SelectTrigger>
            <SelectContent>
              {accounts.map((a) => (
                <SelectItem key={a.id} value={a.id}>
                  {a.name} — {a.provider}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>NOTIFICATIONS</span>
          <span className={styles.hint}>
            Real macOS notifications for the events below. If they're on and nothing arrives, macOS is dropping them —
            check System Settings → Notifications → Hangar, and use the test below to see the actual error.
          </span>
          <NotificationTestRow />
          <div className={styles.notifList}>
            <NotificationRow
              label="Needs your approval"
              description="Ask Edits sessions blocked on a permission you haven't answered yet."
              checked={appSettings.notifyNeedsApproval}
              saving={savingNotif === 'notifyNeedsApproval'}
              onChange={(v) => toggleNotification('notifyNeedsApproval', v)}
            />
            <NotificationRow
              label="Session finished"
              description="A session ran a turn and is back to waiting for you."
              checked={appSettings.notifySessionWaiting}
              saving={savingNotif === 'notifySessionWaiting'}
              onChange={(v) => toggleNotification('notifySessionWaiting', v)}
            />
            <NotificationRow
              label="Account rate-limited"
              description="An account hit a rate limit and can't run turns until it clears."
              checked={appSettings.notifyAccountExhausted}
              saving={savingNotif === 'notifyAccountExhausted'}
              onChange={(v) => toggleNotification('notifyAccountExhausted', v)}
            />
            <NotificationRow
              label="Spend cap exceeded"
              description="An API-key account passed its estimated weekly budget."
              checked={appSettings.notifySpendCap}
              saving={savingNotif === 'notifySpendCap'}
              onChange={(v) => toggleNotification('notifySpendCap', v)}
            />
            <NotificationRow
              label="New PR comments"
              description="A pull request Hangar opened got new review comments — checked every 5 minutes while Hangar is open."
              checked={appSettings.notifyPrComments}
              saving={savingNotif === 'notifyPrComments'}
              onChange={(v) => toggleNotification('notifyPrComments', v)}
            />
          </div>
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>MODO DE LAS TERMINALES</span>
          <TerminalModeRow />
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>TELÉFONO</span>
          <PhoneRow />
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>UPDATES</span>
          <UpdateAccessRow />
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>EXPERIMENTAL</span>
          <div className={styles.notifList}>
            {/* "Hierarchy trees" is hidden for now — the agent-tree feature is
              * parked pending a rethink. The setting itself still exists; only
              * its toggle is removed, so nothing that reads agentTreesEnabled
              * breaks, and restoring this row brings it back. */}
            <NotificationRow
              label="Include my own PR comments"
              description="PR review comments normally hide anything you wrote yourself, since the feature is meant to surface other reviewers' feedback. Turn this on to test the review flow solo, by commenting on your own PRs."
              checked={appSettings.includeOwnPrComments}
              saving={savingNotif === 'includeOwnPrComments'}
              onChange={(v) => toggleNotification('includeOwnPrComments', v)}
            />
          </div>
        </section>
      </div>
    </div>
  );
}

/**
 * The token that lets this machine see updates.
 *
 * Hangar's releases are in a private repository, and GitHub serves nothing
 * from one without authentication. The token goes to the OS keychain rather
 * than into the binary: a copy of the app then carries no credential, and
 * handing someone the app doesn't hand them read access to the repo.
 */
/** El teléfono emparejado con este Mac, y el nombre con el que lo verá.
 *
 * Un solo dispositivo por máquina: emparejar otro expulsa al anterior. Esa
 * restricción declarada de entrada es lo que evita tener que resolver quién
 * escribe cuando dos quieren dirigir la misma terminal. */
const MODES: Array<{ id: string; label: string; blurb: string }> = [
  { id: 'plan', label: 'Plan', blurb: 'Solo lee y propone. No edita ni ejecuta.' },
  { id: 'ask_edits', label: 'Preguntar', blurb: 'Pide permiso antes de editar o ejecutar.' },
  { id: 'auto_edit', label: 'Editar solo', blurb: 'Edita sin preguntar; sigue pidiendo para lo demás.' },
  { id: 'yolo', label: 'Yolo', blurb: 'No pregunta nada. Lo que te protege es el worktree y git.' },
];

/** El modo con el que nacen las terminales de esta máquina.
 *
 * Global por decisión explícita, así que la consecuencia se dice donde se
 * activa y no en un documento: con yolo, **todas** las terminales de este Mac
 * ejecutan sin preguntar. Es lo que permite una app móvil sin notificaciones —
 * si no hay permisos que aprobar, no hace falta que nada te despierte — y es
 * también el modo en el que un agente puede destruir algo. */
function TerminalModeRow() {
  const [mode, setMode] = useState<string>('ask_edits');

  useEffect(() => {
    invoke<string>('get_default_terminal_mode').then(setMode).catch(() => {});
  }, []);

  const choose = async (next: string) => {
    try {
      setMode(await invoke<string>('set_default_terminal_mode', { mode: next }));
    } catch (e) {
      toastError(String(e));
    }
  };

  return (
    <div className={styles.notifList}>
      <div className={styles.modeRow}>
        {MODES.map((option) => (
          <button key={option.id} className={mode === option.id ? styles.modeActive : styles.mode} onClick={() => choose(option.id)}>
            <span className={styles.modeLabel}>{option.label}</span>
            <span className={styles.modeBlurb}>{option.blurb}</span>
          </button>
        ))}
      </div>
      <span className={styles.updateHint}>
        Se aplica a las terminales que <strong>se abran a partir de ahora</strong>. Las que ya están corriendo siguen como estaban: el
        modo se le pasa al CLI al arrancarlo, y cambiarlo después significaría teclearle a ciegas.
      </span>
      {mode === 'yolo' && (
        <span className={styles.modeWarning}>
          Con yolo, cada terminal de este Mac ejecuta sin preguntar — también la del repositorio que más te importa.
        </span>
      )}
    </div>
  );
}

function PhoneRow() {
  const [status, setStatus] = useState<{ machineName: string; device: { name: string; level: string; lastSeenAt: string | null } | null } | null>(null);
  const [name, setName] = useState('');
  const [pairing, setPairing] = useState(false);

  const load = () => {
    invoke<{ machineName: string; device: { name: string; level: string; lastSeenAt: string | null } | null }>('bridge_status')
      .then((found) => {
        setStatus(found);
        setName(found.machineName);
      })
      .catch(() => {});
  };

  useEffect(load, []);

  const revoke = async () => {
    try {
      await invoke('bridge_revoke_device');
      load();
    } catch (e) {
      toastError(String(e));
    }
  };

  return (
    <div className={styles.notifList}>
      <div className={styles.updateRow}>
        <input
          className={styles.updateInput}
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={() => invoke('set_machine_name', { name }).then(load).catch(() => {})}
          placeholder="cómo se llama este Mac en el teléfono"
        />
        {status?.device ? (
          <Button size="sm" variant="outline" onClick={revoke}>
            revocar
          </Button>
        ) : (
          <Button size="sm" onClick={() => setPairing(true)}>
            Emparejar
          </Button>
        )}
      </div>

      {status?.device ? (
        <span className={styles.updateHint}>
          <strong>{status.device.name}</strong> · nivel {status.device.level}
          {status.device.lastSeenAt ? ` · visto ${new Date(status.device.lastSeenAt).toLocaleString()}` : ' · aún no se ha conectado'}
          . Revocar corta el acceso en el siguiente mensaje, sin que el teléfono tenga que cooperar.
        </span>
      ) : (
        <span className={styles.updateHint}>
          Ningún teléfono emparejado. La clave de cifrado viaja en el QR y solo ahí, así que emparejar exige tener este Mac delante.
        </span>
      )}

      {pairing && <PairPhoneDialog onClose={() => setPairing(false)} onPaired={load} />}
    </div>
  );
}

function UpdateAccessRow() {
  const [stored, setStored] = useState(false);
  const [token, setToken] = useState('');
  const [saving, setSaving] = useState(false);
  const [version, setVersion] = useState('');
  // What the last check actually said. The banner stays silent by design — an
  // absent token, no network, or simply being up to date are all normal — but
  // silence made "is this working?" unanswerable, which cost a release cut to
  // find out the answer was "you already installed it".
  const [checked, setChecked] = useState<string | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    invoke<boolean>('has_update_access').then(setStored).catch(() => setStored(false));
    getVersion().then(setVersion).catch(() => {});
  }, []);

  const checkNow = async () => {
    setChecking(true);
    setChecked(null);
    try {
      const headers = await invoke<Record<string, string> | null>('update_request_headers');
      const found = await check(headers ? { headers } : undefined);
      setChecked(found ? `v${found.version} is available — restart Hangar to see the update bar.` : `Up to date. Nothing newer than v${version}.`);
    } catch (e) {
      setChecked(`Check failed: ${e}`);
    } finally {
      setChecking(false);
    }
  };

  const save = async (next: string | null) => {
    setSaving(true);
    try {
      await invoke('set_update_access', { token: next });
      setStored(Boolean(next));
      setToken('');
    } catch (e) {
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className={styles.notifList}>
      <div className={styles.updateRow}>
        <input
          className={styles.updateInput}
          type="password"
          placeholder={stored ? '•••••••• stored — paste a new one to replace it' : 'GitHub token with read access to the releases repo'}
          value={token}
          onChange={(e) => setToken(e.target.value)}
        />
        <Button size="sm" disabled={saving || !token.trim()} onClick={() => save(token.trim())}>
          {saving ? 'saving…' : 'Save'}
        </Button>
        {stored && (
          <Button size="sm" variant="outline" disabled={saving} onClick={() => save(null)}>
            remove
          </Button>
        )}
        <Button size="sm" variant="outline" disabled={checking} onClick={checkNow}>
          {checking ? 'checking…' : 'Check now'}
        </Button>
      </div>
      {checked && <span className={styles.updateHint}>{checked}</span>}
      {version && <span className={styles.updateHint}>Running v{version}.</span>}
      <span className={styles.updateHint}>
        Stored in the macOS keychain, never in the app or the database. A fine-grained token with read-only <code>Contents</code> on the
        releases repository is enough — nothing else. Without one, Hangar simply never finds an update.
      </span>
    </div>
  );
}

/** Asks the notification path whether it works, out loud.
 *
 * The toggles below have been on for months while nothing reached the screen,
 * and there was no way to tell that apart from "no agent finished recently"
 * — the send swallowed its error. This is the interrogation button, the same
 * one the updater needed. */
function NotificationTestRow() {
  const [result, setResult] = useState<string | null>(null);
  const [sending, setSending] = useState(false);

  const send = async () => {
    setSending(true);
    setResult(null);
    try {
      await invoke('send_test_notification');
      setResult('Sent. If nothing appeared, macOS is blocking it — System Settings → Notifications → Hangar.');
    } catch (e) {
      setResult(`Failed: ${e}`);
    } finally {
      setSending(false);
    }
  };

  return (
    <div className={styles.notifList}>
      <div className={styles.updateRow}>
        <Button size="sm" variant="outline" disabled={sending} onClick={send}>
          {sending ? 'sending…' : 'Send a test notification'}
        </Button>
      </div>
      {result && <span className={styles.updateHint}>{result}</span>}
    </div>
  );
}

function NotificationRow({
  label,
  description,
  checked,
  saving,
  onChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  saving: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <label className={styles.notifRow}>
      <Checkbox checked={checked} onCheckedChange={(v) => onChange(v === true)} disabled={saving} />
      <span className={styles.notifInfo}>
        <span className={styles.notifLabel}>{label}</span>
        <span className={styles.notifDescription}>{description}</span>
      </span>
    </label>
  );
}

function AccountCapRow({
  account,
  usage,
  capInput,
  onCapInputChange,
  onSave,
  saving,
}: {
  account: Account;
  usage: AccountUsageSummary | undefined;
  capInput: string;
  onCapInputChange: (v: string) => void;
  onSave: () => void;
  saving: boolean;
}) {
  const isSubscription = account.billing === 'sub';
  const realUsage = useRealUsage(isSubscription ? account.id : undefined, isSubscription);

  if (isSubscription) {
    const pct = realUsage.stats?.weeklyPct ?? null;
    return (
      <div className={styles.capRow}>
        <ProviderBadge account={account} />
        <span className={styles.capName}>{account.name}</span>
        <span className={styles.capBarWrap}>
          {pct != null ? (
            <UsageBar pct={pct} color={contextSeverityColor(contextSeverity(pct))} label={`${pct.toFixed(0)}% / 7d`} width={140} />
          ) : (
            <span className={styles.dimText}>fetching real usage…</span>
          )}
        </span>
        <span className={styles.dimText}>subscription — no $ cap applies</span>
      </div>
    );
  }

  const weeklyCapEstimate = account.monthlyCap ? account.monthlyCap / 4.345 : null;
  const pct = weeklyCapEstimate && usage ? (usage.costUsd / weeklyCapEstimate) * 100 : 0;
  const color = weeklyCapEstimate ? contextSeverityColor(contextSeverity(pct)) : 'var(--teal)';

  return (
    <div className={styles.capRow}>
      <ProviderBadge account={account} />
      <span className={styles.capName}>{account.name}</span>
      <span className={styles.capBarWrap}>
        {usage ? (
          <UsageBar pct={pct} color={color} label={`${formatUsd(usage.costUsd)} / 7d`} width={140} />
        ) : (
          <span className={styles.dimText}>loading…</span>
        )}
      </span>
      <Input
        className={styles.capInput}
        value={capInput}
        onChange={(e) => onCapInputChange(e.target.value)}
        onBlur={onSave}
        onKeyDown={(e) => e.key === 'Enter' && onSave()}
        placeholder="no cap"
        disabled={saving}
      />
    </div>
  );
}

function ProviderBadge({ account }: { account: Account }) {
  return <span className={styles.providerBadge}>{account.provider[0].toUpperCase()}</span>;
}
