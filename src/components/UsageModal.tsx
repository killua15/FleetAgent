import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { fetchProfileUsage, pushUsage, setUsageSyncEnabled, usageSyncEnabled, type ProfileUsage } from '../lib/usageSync';
import { toastError } from './ErrorOverlay';
import { Button } from './ui/button';
import { compactTokens, money } from '../lib/formatUsage';
import styles from './UsageModal.module.css';

interface TokenTotals {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  costUsd: number;
  turns: number;
}

/** How far back the report reaches. `all` starts at the ledger's own first
 * row, so "since the beginning" is read from the data rather than guessed. */
type UsageWindow = 'week' | 'month' | 'year' | 'all';
type UsageGroup = 'project' | 'account' | 'machine' | 'model';

const WINDOWS: { id: UsageWindow; label: string }[] = [
  { id: 'week', label: 'week' },
  { id: 'month', label: 'month' },
  { id: 'year', label: 'year' },
  { id: 'all', label: 'all' },
];

/** Grouping by machine is what makes two Macs on one profile add up — the
 * ledger records which one spent each turn. */
const GROUPS: { id: UsageGroup; label: string }[] = [
  { id: 'project', label: 'project' },
  { id: 'account', label: 'account' },
  { id: 'machine', label: 'machine' },
  { id: 'model', label: 'model' },
];

interface UsageRow {
  label: string;
  provider: string;
  /** The project or account no longer exists. The spend still does. */
  archived: boolean;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  turns: number;
  costUsd: number;
}

interface UsageReport {
  ledgerStartsAt: string | null;
  from: string;
  rows: UsageRow[];
  total: UsageRow;
}


interface AccountSpend {
  accountId: string;
  accountName: string;
  provider: string;
  /** `sub` or `key` — decides which number leads. */
  billing: string;
  sessionPct: number | null;
  weeklyPct: number | null;
  hoursLeft: number | null;
  session: TokenTotals;
  week: TokenTotals;
  calibration: { pctPerDollar: number; spans: number } | null;
  weekPctEquivalent: number | null;
  byProject: { projectName: string; costUsd: number; turns: number }[];
  byModel: { model: string; costUsd: number; turns: number }[];
}

const compact = compactTokens;

function Bar({ pct }: { pct: number }) {
  // Amber past 70, red past 90 — the same thresholds the queue uses to decide
  // whether an account has room, so the colour here and the decision there
  // can't tell different stories.
  const tone = pct >= 90 ? 'var(--red)' : pct >= 70 ? 'var(--amber)' : 'var(--grn)';
  return (
    <span className={styles.bar}>
      <span className={styles.barFill} style={{ width: `${Math.min(100, pct)}%`, background: tone }} />
    </span>
  );
}

/**
 * What each account has spent — in the unit that account is actually billed in.
 *
 * A subscription burns **quota**, and the dollar figure is a market equivalent
 * nobody charges you; showing it as money would have you worrying about a bill
 * that doesn't exist. A key-billed account is the opposite: the money is real
 * and there is no percentage to show. Same table, two different lead numbers.
 */
export function UsageModal({ onClose }: { onClose: () => void }) {
  const [accounts, setAccounts] = useState<AccountSpend[] | null>(null);
  const [report, setReport] = useState<UsageReport | null>(null);
  const [range, setRange] = useState<UsageWindow>('month');
  const [group, setGroup] = useState<UsageGroup>('project');
  // The profile's spend across every Mac. `null` while unknown — which is also
  // the honest state when sync is off, since this machine cannot know what the
  // other one spent.
  const [profile, setProfile] = useState<ProfileUsage | null>(null);
  const [syncOn, setSyncOn] = useState(usageSyncEnabled());
  const [reading, setReading] = useState(false);

  const load = () => {
    invoke<AccountSpend[]>('list_account_spend').then(setAccounts).catch((e) => toastError(String(e)));
  };

  useEffect(load, []);

  // Re-read whenever the window or the grouping changes: the report is one
  // query over the ledger, so switching is a round trip and not a re-render of
  // something already held.
  useEffect(() => {
    invoke<UsageReport>('usage_report', { window: range, group })
      .then(setReport)
      .catch((e) => toastError(String(e)));
  }, [range, group]);

  // Push first, then read: otherwise the profile total is this machine's work
  // from the last cycle plus the other machine's from now, which reads as a
  // number that went backwards.
  useEffect(() => {
    if (!syncOn) {
      setProfile(null);
      return;
    }
    const days = range === 'week' ? 7 : range === 'month' ? 31 : 365;
    pushUsage()
      .then(() => fetchProfileUsage(days))
      .then(setProfile)
      // Quietly: no session, no network, or sync just turned on are all normal,
      // and none of them is worth interrupting a usage modal about.
      .catch(() => setProfile(null));
  }, [syncOn, range]);

  // Explicit, because reading a Claude account's quota spawns a `claude` in a
  // PTY and takes seconds — three of those firing on open would look like the
  // app had hung. The background poller only samples accounts with a terminal
  // open, which is why an idle account sits at "no reading" until you ask.
  const readQuotas = async () => {
    setReading(true);
    try {
      await invoke<number>('refresh_all_quotas');
      load();
    } catch (e) {
      toastError(String(e));
    } finally {
      setReading(false);
    }
  };

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose();
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div className={styles.backdrop} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.head}>
          <span className={styles.title}>Usage</span>
          <span className={styles.subtitle}>quota per account, spend per project</span>
          <span className={styles.spacer} />
          <Button size="sm" variant="outline" disabled={reading} onClick={readQuotas}>
            {reading ? 'reading quotas…' : '↻ read quotas'}
          </Button>
          <Button size="sm" variant="outline" onClick={onClose}>
            close
          </Button>
        </div>

        <div className={styles.body}>
          <span className={styles.sectionTitle}>accounts</span>
          {!accounts && <span className={styles.dim}>reading…</span>}
          {accounts?.length === 0 && <span className={styles.dim}>No accounts yet.</span>}

          {accounts?.map((account) => {
            const subscription = account.billing === 'sub';
            const noReading = account.sessionPct === null && account.weeklyPct === null;
            return (
              <div key={account.accountId} className={styles.account}>
                <span className={styles.name}>{account.accountName}</span>
                <span className={styles.provider}>{account.provider}</span>

                {subscription ? (
                  <>
                    {/* Quota leads on a subscription: it's what stops you working. */}
                    <span className={styles.window}>
                      session
                      <Bar pct={account.sessionPct ?? 0} />
                      <span className={styles.pct}>{account.sessionPct === null ? '—' : `${Math.round(account.sessionPct)}%`}</span>
                    </span>
                    <span className={styles.window}>
                      week
                      <Bar pct={account.weeklyPct ?? 0} />
                      <span className={styles.pct}>{account.weeklyPct === null ? '—' : `${Math.round(account.weeklyPct)}%`}</span>
                    </span>
                    {account.hoursLeft !== null && <span className={styles.hours}>~{account.hoursLeft.toFixed(1)} h left</span>}
                  </>
                ) : (
                  <span className={styles.window}>
                    <span className={styles.pct}>{money(account.week.costUsd)}</span>
                    <span className={styles.dim}>billed this week</span>
                  </span>
                )}

                <span className={styles.spacer} />
                <span className={styles.cost}>
                  {money(account.week.costUsd)}
                  {subscription && <span className={styles.equivalent}> equivalent · 7d</span>}
                </span>

                {/* Said where it's read, not in a footnote: a dash with no
                    explanation reads as "broken", and the fix is one button
                    away at the top of this very modal. */}
                {subscription && noReading && <span className={styles.note}>no quota reading yet — press “read quotas”</span>}
                {subscription && !noReading && account.weekPctEquivalent !== null && account.calibration && (
                  <span className={styles.note}>
                    ≈ {account.weekPctEquivalent.toFixed(1)}% of the week, at {account.calibration.pctPerDollar.toFixed(1)}%/$ measured from{' '}
                    {account.calibration.spans} of your own readings
                  </span>
                )}
                {subscription && !noReading && account.weekPctEquivalent === null && (
                  <span className={styles.note}>calibrating — needs two quota readings in one window to turn tokens into %</span>
                )}
              </div>
            );
          })}

          <div className={styles.reportHead}>
            <span className={styles.sectionTitle}>where it went</span>
            <span className={styles.spacer} />
            {/* The window is a choice now. It used to be seven days, hardcoded
                in the SQL, which is why nothing older than a week was ever
                visible. */}
            {WINDOWS.map((w) => (
              <button key={w.id} className={w.id === range ? styles.windowActive : styles.window} onClick={() => setRange(w.id)}>
                {w.label}
              </button>
            ))}
            {GROUPS.map((g) => (
              <button key={g.id} className={g.id === group ? styles.windowActive : styles.window} onClick={() => setGroup(g.id)}>
                {g.label}
              </button>
            ))}
          </div>

          {/* Across every Mac on this profile, which is the only number this
              machine cannot work out on its own. Off by default: it is the one
              thing Hangar sends anywhere, so it waits to be asked. */}
          <div className={styles.reportHead}>
            <span className={styles.sectionTitle}>this profile</span>
            <span className={styles.spacer} />
            <button
              className={syncOn ? styles.windowActive : styles.window}
              title={syncOn ? 'Daily totals are shared with your other Macs' : 'Share daily totals with your other Macs — day, project, account and counters only'}
              onClick={() => {
                const next = !syncOn;
                setUsageSyncEnabled(next);
                setSyncOn(next);
              }}
            >
              {syncOn ? 'sync on' : 'sync off'}
            </button>
          </div>

          {!syncOn && (
            <span className={styles.dim}>
              Only this Mac. Turn sync on to add up the machines signed in to your profile — Hangar sends a daily total per project and
              account, never prompts, code or paths.
            </span>
          )}

          {syncOn && profile && (
            <div className={styles.table}>
              <div className={styles.tableTotal}>
                <span className={styles.colProject}>every machine</span>
                <span className={styles.colNum}>{profile.turns}</span>
                <span className={styles.colNum}>{money(profile.costUsd)}</span>
              </div>
              {profile.machines.map((m) => (
                <div key={m.machineId} className={styles.tableRow}>
                  <span className={styles.colProject}>
                    {m.machineName}
                    {m.isThisMachine && <span className={styles.archived}>this one</span>}
                  </span>
                  <span className={styles.colNum}>{m.turns}</span>
                  <span className={styles.colNum}>{money(m.costUsd)}</span>
                </div>
              ))}
            </div>
          )}

          {report && (
            <span className={styles.dim}>
              {range === 'all' && report.ledgerStartsAt
                ? `everything since ${new Date(report.ledgerStartsAt).toLocaleDateString()}`
                : `since ${new Date(report.from).toLocaleDateString()}`}
            </span>
          )}

          {report?.rows.length === 0 && (
            <span className={styles.dim}>
              No turns recorded in this window. Numbers appear as each agent finishes a turn — Hangar reads what the CLI writes to its own transcript.
            </span>
          )}

          {report && report.rows.length > 0 && (
            <div className={styles.table}>
              <div className={styles.tableHead}>
                <span className={styles.colProject}>{GROUPS.find((g) => g.id === group)?.label}</span>
                <span className={styles.colNum}>in</span>
                <span className={styles.colNum}>out</span>
                <span className={styles.colNum}>cache</span>
                <span className={styles.colNum}>turns</span>
                <span className={styles.colNum}>cost</span>
              </div>

              {/* The total first, because it is the number the question was
                  about. Everything under it is how that number was reached. */}
              <div className={styles.tableTotal}>
                <span className={styles.colProject}>total</span>
                <span className={styles.colNum}>{compact(report.total.inputTokens)}</span>
                <span className={styles.colNum}>{compact(report.total.outputTokens)}</span>
                <span className={styles.colNum}>{compact(report.total.cacheReadTokens)}</span>
                <span className={styles.colNum}>{report.total.turns}</span>
                <span className={styles.colNum}>{money(report.total.costUsd)}</span>
              </div>

              {/* Every row, not the top six: the ledger stores names, so a
                  deleted project still appears under its own name instead of
                  collapsing into one anonymous line. The list scrolls with the
                  modal body. */}
              {report.rows.map((row) => (
                <div key={row.label} className={styles.tableRow}>
                  <span className={styles.colProject}>
                    {row.label}
                    {row.archived && <span className={styles.archived}>archived</span>}
                    {row.provider && <span className={styles.dim}> {row.provider}</span>}
                  </span>
                  <span className={styles.colNum}>{compact(row.inputTokens)}</span>
                  <span className={styles.colNum}>{compact(row.outputTokens)}</span>
                  <span className={styles.colNum}>{compact(row.cacheReadTokens)}</span>
                  <span className={styles.colNum}>{row.turns}</span>
                  <span className={styles.colNum}>{money(row.costUsd)}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
