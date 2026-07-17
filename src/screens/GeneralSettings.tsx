import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { SettingsTabs } from '../components/SettingsTabs';
import { UsageBar } from '../components/UsageBar';
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

export function GeneralSettings() {
  const { theme, setTheme } = useTheme();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [usage, setUsage] = useState<Record<string, AccountUsageSummary>>({});
  const [caps, setCaps] = useState<Record<string, string>>({});
  const [savingCap, setSavingCap] = useState<string | null>(null);
  const [appSettings, setAppSettingsState] = useState<AppSettings>({ reviewerEnabled: false, reviewerAccountId: null, notifyNeedsApproval: false, notifySessionWaiting: false, notifyAccountExhausted: false, notifySpendCap: false });
  const [savingReviewer, setSavingReviewer] = useState(false);

  useEffect(() => {
    invoke<Account[]>('list_accounts').then((accs) => {
      setAccounts(accs);
      setCaps(Object.fromEntries(accs.map((a) => [a.id, a.monthlyCap != null ? String(a.monthlyCap) : ''])));
    });
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
            Real macOS notifications for the events below — off by default, so turning one on is the moment Fleet
            first asks the OS for permission.
          </span>
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
          </div>
        </section>
      </div>
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
