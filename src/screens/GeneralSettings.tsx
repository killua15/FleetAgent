import { useEffect, useState, type ReactNode } from 'react';
import { useAuth } from '../auth/AuthProvider';
import { summarizePlan } from '../auth/plan';
import { invoke } from '../lib/invoke';
import { toastError } from '../components/ErrorOverlay';
import { relaunch } from '@tauri-apps/plugin-process';
import { runUpdateCheck, useUpdateStatus } from '../lib/updates';
import { PairPhoneDialog } from '../components/PairPhoneDialog';
import { getVersion } from '@tauri-apps/api/app';
import { SettingsTabs } from '../components/SettingsTabs';
import { RequirementsPanel } from '../components/RequirementsPanel';
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
import type { AgentPreferences, AppSettings } from '../types/settings';
import { LANGUAGES, systemLanguage } from '../lib/languages';
import styles from './GeneralSettings.module.css';

/** Los grupos del raíl. General era un cajón de sastre de once secciones; esto
 * es lo que tienen en común, no un reparto por tamaño. */
const PANES = [
  // First, and named for the person rather than for the thing: "Cuentas" below
  // is the CLI logins Hangar drives, which is a different subject entirely.
  { id: 'profile', label: 'Perfil' },
  { id: 'machine', label: 'Máquina' },
  { id: 'appearance', label: 'Apariencia' },
  { id: 'accounts', label: 'Cuentas' },
  { id: 'agents', label: 'Agentes' },
  { id: 'phone', label: 'Teléfono' },
  { id: 'advanced', label: 'Avanzado' },
] as const;

type PaneId = (typeof PANES)[number]['id'];

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
  // Arranca en Máquina: es lo que alguien recién instalado necesita, y lo único
  // de esta pantalla que puede estar en rojo.
  const [pane, setPane] = useState<PaneId>('machine');
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
    notifyUpdateAvailable: false,
    notifyTaskChanges: false,
    keepInMenuBar: true,
    agentTreesEnabled: false,
    includeOwnPrComments: false,
    panelWarnThreshold: 4,
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
      <div className={styles.split}>
        <nav className={styles.rail} aria-label="Secciones de General">
          {PANES.map((p) => (
            <button
              key={p.id}
              className={p.id === pane ? styles.railItemActive : styles.railItem}
              aria-current={p.id === pane}
              onClick={() => setPane(p.id)}
            >
              {p.label}
            </button>
          ))}
        </nav>
        <div className={styles.body}>
          <Pane id="profile" active={pane}>
          <section className={styles.section}>
            <span className={styles.sectionLabel}>TU CUENTA DE HANGAR</span>
            <AccountRow />
          </section>
          </Pane>

          <Pane id="machine" active={pane}>
          <section className={styles.sectionWide}>
            <span className={styles.sectionLabel}>WHAT HANGAR NEEDS</span>
            <RequirementsPanel />
          </section>
          </Pane>

          <Pane id="appearance" active={pane}>
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
          </Pane>

          <Pane id="appearance" active={pane}>
          <section className={styles.section}>
            <span className={styles.sectionLabel}>MENU BAR</span>
            <div className={styles.notifList}>
              <NotificationRow
                label="Keep Hangar in the menu bar when the window is closed"
                description="Closing the window hides it instead of quitting, so your agents and the PR and task checks keep running. Quit from the menu-bar icon or with ⌘Q."
                checked={appSettings.keepInMenuBar}
                saving={savingNotif === 'keepInMenuBar'}
                onChange={(v) => toggleNotification('keepInMenuBar', v)}
              />
            </div>
          </section>
          </Pane>

          <Pane id="accounts" active={pane}>
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
          </Pane>

          <Pane id="accounts" active={pane}>
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
          </Pane>

          <Pane id="accounts" active={pane}>
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
          </Pane>

          <Pane id="agents" active={pane}>
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
                label="Pull requests"
                description="A new PR shows up for one of your projects, or one Hangar opened gets new review comments — checked every 5 minutes while Hangar is open."
                checked={appSettings.notifyPrComments}
                saving={savingNotif === 'notifyPrComments'}
                onChange={(v) => toggleNotification('notifyPrComments', v)}
              />
              <NotificationRow
                label="Update available"
                description="A new version of Hangar is ready to install."
                checked={appSettings.notifyUpdateAvailable}
                saving={savingNotif === 'notifyUpdateAvailable'}
                onChange={(v) => toggleNotification('notifyUpdateAvailable', v)}
              />
              <NotificationRow
                label="Tasks"
                description="A task is assigned to you or changes status on one of your boards — read every 5 minutes while Hangar is open."
                checked={appSettings.notifyTaskChanges}
                saving={savingNotif === 'notifyTaskChanges'}
                onChange={(v) => toggleNotification('notifyTaskChanges', v)}
              />
            </div>
          </section>
          </Pane>

          <Pane id="agents" active={pane}>
          <section className={styles.section}>
            <span className={styles.sectionLabel}>IDIOMA DE LOS AGENTES</span>
            <AgentLanguageRow />
          </section>
          </Pane>

          <Pane id="agents" active={pane}>
          <section className={styles.section}>
            <span className={styles.sectionLabel}>MODO DE LAS TERMINALES</span>
            <TerminalModeRow />
          </section>
          <section className={styles.section}>
            <span className={styles.sectionLabel}>PANELES EN PANTALLA</span>
            <PanelWarnRow />
          </section>
          </Pane>

          <Pane id="phone" active={pane}>
          <section className={styles.section}>
            <span className={styles.sectionLabel}>TELÉFONO</span>
            <PhoneRow />
          </section>
          </Pane>

          <Pane id="machine" active={pane}>
          <section className={styles.section}>
            <span className={styles.sectionLabel}>UPDATES</span>
            <UpdatesRow />
          </section>
          </Pane>

          <Pane id="advanced" active={pane}>
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
          </Pane>
        </div>
      </div>
    </div>
  );
}

/**
 * Un panel del raíl: sus secciones solo existen cuando está elegido.
 *
 * Se desmonta en vez de esconderse con CSS, para que el scroll de un panel no
 * se lleve al siguiente y para no montar once secciones para enseñar una.
 */
function Pane({ id, active, children }: { id: PaneId; active: PaneId; children: ReactNode }) {
  return id === active ? <>{children}</> : null;
}

/**
 * Who is signed in to Hangar, on which plan, and the way out.
 *
 * This used to be a button in the top bar that showed a name and signed you out
 * when clicked. Signing out on a stray click is a poor trade for a bar you
 * click all day, and one line had nowhere to say what the plan actually means —
 * so identity moved here, where there is room for it and where the paid tiers
 * will eventually be explained.
 */
function AccountRow() {
  const { email, displayName, avatarUrl, entitlement, signOut, recheck } = useAuth();
  const [busy, setBusy] = useState(false);
  const plan = summarizePlan(entitlement);

  // No email while a recent sign-in is coasting offline — the grace window in
  // `access.ts`. Nothing about the person is known then, so say that instead of
  // drawing an empty profile.
  if (!email) {
    return (
      <div className={styles.profileRow}>
        <div className={styles.profileText}>
          <span className={styles.profileName}>Sin conexión</span>
          <span className={styles.hint}>
            Hangar sigue abierto con una sesión reciente, pero no ha podido hablar con el servidor. Se pondrá al día solo en cuanto vuelva la red.
          </span>
        </div>
        <span className={styles.profileSpacer} />
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            try {
              await recheck();
            } finally {
              setBusy(false);
            }
          }}
        >
          {busy ? 'comprobando…' : 'reintentar'}
        </Button>
      </div>
    );
  }

  const initials = (displayName ?? email)
    .split(/[\s@.]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase() ?? '')
    .join('');

  return (
    <>
      <div className={styles.profileRow}>
        {avatarUrl ? (
          <img className={styles.avatar} src={avatarUrl} alt="" referrerPolicy="no-referrer" />
        ) : (
          <span className={styles.avatarFallback} aria-hidden="true">
            {initials}
          </span>
        )}
        <div className={styles.profileText}>
          <span className={styles.profileName}>{displayName ?? email.split('@')[0]}</span>
          <span className={styles.profileEmail}>{email}</span>
        </div>
        <span className={styles.profileSpacer} />
        <span className={plan.needsAttention ? styles.planBadgeAttention : plan.paid ? styles.planBadgePaid : styles.planBadge}>{plan.label}</span>
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          onClick={async () => {
            setBusy(true);
            try {
              await signOut();
            } finally {
              setBusy(false);
            }
          }}
        >
          cerrar sesión
        </Button>
      </div>
      {plan.note && <span className={styles.hint}>{plan.note}</span>}
      {!plan.paid && (
        // Deliberately not a call to action: there is nothing to sell yet. It
        // names what the plan gates today so the badge above means something.
        <span className={styles.hint}>El plan gratuito incluye todo salvo el emparejamiento con el teléfono.</span>
      )}
    </>
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

/** Los idiomas por defecto de toda la app, que cada proyecto puede sobreescribir.
 *
 * Existe porque pedir algo en la consola y recibirlo en inglés —y tener que
 * pedir que se reescriba, cada vez— era el estado normal: Hangar tenía ajustes
 * y ninguno llegaba nunca al CLI. Esto es el primer ajuste que sí llega.
 *
 * Dos campos y no uno porque son preferencias distintas: conversar en español
 * sobre un repo cuyos comentarios están en inglés es el caso común, no el raro. */
function AgentLanguageRow() {
  const [prefs, setPrefs] = useState<AgentPreferences>({ conversationLanguage: null, commentLanguage: null });
  const [saving, setSaving] = useState(false);
  const suggestion = systemLanguage();

  useEffect(() => {
    invoke<AgentPreferences>('get_global_agent_preferences').then(setPrefs).catch(() => {});
  }, []);

  const save = async (next: AgentPreferences) => {
    const previous = prefs;
    setPrefs(next);
    setSaving(true);
    try {
      setPrefs(await invoke<AgentPreferences>('set_global_agent_preferences', { input: next }));
    } catch (e) {
      setPrefs(previous);
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className={styles.notifList}>
      <div className={styles.updateRow}>
        <LanguageSelect
          label="Conversación"
          value={prefs.conversationLanguage}
          disabled={saving}
          onChange={(v) => save({ ...prefs, conversationLanguage: v })}
        />
        <LanguageSelect
          label="Código y commits"
          value={prefs.commentLanguage}
          disabled={saving}
          onChange={(v) => save({ ...prefs, commentLanguage: v })}
        />
      </div>
      <span className={styles.updateHint}>
        El primero es en el que te responde. El segundo es el de los comentarios, los nombres y los mensajes de commit — puedes
        conversar en uno y escribir el código en otro. Cada proyecto puede sobreescribir esto en <strong>Settings → proyectos</strong>.
      </span>
      {/* Misma advertencia que el modo, y por la misma razón: las preferencias
          se le pasan al CLI al arrancarlo. */}
      <span className={styles.updateHint}>
        Se aplica a las terminales y turnos que <strong>empiecen a partir de ahora</strong>. Hoy llega a Claude, Kimi y Grok; Codex no
        tiene forma de recibirlo, así que sus terminales siguen igual.
      </span>
      {prefs.conversationLanguage === null && suggestion && (
        <span className={styles.updateHint}>
          Sin configurar no se le dice nada al agente.{' '}
          <button className={styles.inlineLink} onClick={() => save({ ...prefs, conversationLanguage: suggestion.label })}>
            Usar {suggestion.label}, el de este Mac
          </button>
        </span>
      )}
    </div>
  );
}

/** Un idioma o ninguno. "Sin preferencia" es un valor real y es el que hace que
 * Hangar no pase ninguna bandera, en vez de pasar una vacía. */
function LanguageSelect({
  label,
  value,
  disabled,
  onChange,
}: {
  label: string;
  value: string | null;
  disabled?: boolean;
  onChange: (value: string | null) => void;
}) {
  return (
    <label className={styles.langField}>
      <span className={styles.langLabel}>{label}</span>
      <select
        className={styles.langSelect}
        value={value ?? ''}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value || null)}
      >
        <option value="">sin preferencia</option>
        {LANGUAGES.map((l) => (
          <option key={l.code} value={l.label}>
            {l.label}
          </option>
        ))}
      </select>
    </label>
  );
}

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

/** Cuántos paneles abiertos antes de avisar.
 *
 * Sigue siendo un consejo y no un tope: nada se bloquea y ningún panel se pliega
 * solo, que es lo que el propio texto del aviso promete. Configurable porque el
 * número cómodo depende del tamaño de la pantalla, y un aviso que aparece antes
 * de tiempo se aprende a ignorar — que es peor que no tenerlo. */
function PanelWarnRow() {
  const [threshold, setThreshold] = useState<number | null>(null);

  useEffect(() => {
    invoke<AppSettings>('get_app_settings')
      .then((s) => setThreshold(s.panelWarnThreshold))
      .catch((e) => toastError(String(e)));
  }, []);

  const choose = async (next: number) => {
    // Optimista: el select se queda donde lo dejó el usuario y no salta atrás
    // mientras el backend confirma.
    setThreshold(next);
    try {
      const current = await invoke<AppSettings>('get_app_settings');
      const saved = await invoke<AppSettings>('set_app_settings', { input: { ...current, panelWarnThreshold: next } });
      setThreshold(saved.panelWarnThreshold);
    } catch (e) {
      toastError(String(e));
    }
  };

  return (
    <div className={styles.notifList}>
      <label className={styles.notifRow}>
        <span className={styles.notifLabel}>Avisar a partir de</span>
        <span className={styles.spacer} />
        <select
          value={threshold === null ? '' : String(threshold)}
          disabled={threshold === null}
          onChange={(e) => choose(Number(e.target.value))}
        >
          {[2, 3, 4, 5, 6, 8].map((n) => (
            <option key={n} value={n}>
              {n} paneles
            </option>
          ))}
          {/* 0 es «nunca» — ver `AppSettings::panel_warn_threshold`. */}
          <option value={0}>nunca</option>
        </select>
      </label>
      <span className={styles.updateHint}>
        Solo es un aviso: las terminales nunca se bloquean, y las que ocultes de la barra lateral siguen corriendo igual.
      </span>
    </div>
  );
}

function PhoneRow() {
  // El companion móvil (emparejar, ver, dirigir y escribir en terminales
  // desde el teléfono — todo lo que trae `feat/mobile-companion`) es una
  // función Pro: emparejar exige `entitlement.plan !== 'free'`. Un teléfono
  // ya emparejado antes de bajar de plan sigue funcionando — esto solo
  // bloquea *empezar* un emparejamiento nuevo, no revoca uno existente.
  const { entitlement } = useAuth();
  const isPro = entitlement != null && entitlement.plan !== 'free';

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
          <Button size="sm" onClick={() => setPairing(true)} disabled={!isPro} title={isPro ? undefined : 'El companion móvil es una función Pro'}>
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
      ) : isPro ? (
        <span className={styles.updateHint}>
          Ningún teléfono emparejado. La clave de cifrado viaja en el QR y solo ahí, así que emparejar exige tener este Mac delante.
        </span>
      ) : (
        <span className={styles.updateHint}>
          Ver, dirigir y escribir en tus terminales desde el teléfono es una función Pro. Hace falta una cuenta con plan Pro o Team para emparejar uno.
        </span>
      )}

      {pairing && isPro && <PairPhoneDialog onClose={() => setPairing(false)} onPaired={load} />}
    </div>
  );
}

/** The running version, a way to check by hand, and what the background
 * updater is doing — the same state the banner shows (`useUpdateStatus`).
 * There is nothing to configure: updates come on the signed-in session. */
function UpdatesRow() {
  const [version, setVersion] = useState('');
  const status = useUpdateStatus();

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  const busy = status.phase === 'checking' || status.phase === 'installing';
  const line =
    status.phase === 'checking'
      ? 'Checking…'
      : status.phase === 'installing'
        ? `Downloading v${status.version}…`
        : status.phase === 'ready'
          ? `v${status.version} is installed — restart Hangar to run it.`
          : status.phase === 'upToDate'
            ? "You're on the latest version."
            : status.phase === 'failed'
              ? `Update failed: ${status.error}`
              : null;

  return (
    <div className={styles.notifList}>
      <div className={styles.updateRow}>
        <span className={styles.updateHint}>{version ? `Hangar v${version}` : 'Hangar'}</span>
        {status.phase === 'ready' ? (
          <Button size="sm" onClick={() => relaunch()}>
            Restart
          </Button>
        ) : (
          <Button size="sm" variant="outline" disabled={busy} onClick={() => void runUpdateCheck()}>
            {busy ? 'checking…' : 'Check for updates'}
          </Button>
        )}
      </div>
      {line && <span className={styles.updateHint}>{line}</span>}
      <span className={styles.updateHint}>
        New versions download and install on their own; restarting is the only step left to you.
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
