import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { toastError } from './ErrorOverlay';
import { Button } from './ui/button';
import styles from './UsageModal.module.css';

interface TokenTotals {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  costUsd: number;
  turns: number;
}

interface ProjectAccountSpend {
  projectName: string;
  accountId: string;
  accountName: string;
  provider: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  turns: number;
  costUsd: number;
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

const compact = (n: number) => (n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)}M` : n >= 1_000 ? `${Math.round(n / 1000)}K` : String(n));

const money = (n: number) => (n >= 1 ? `$${n.toFixed(2)}` : `$${n.toFixed(3)}`);

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
  const [rows, setRows] = useState<ProjectAccountSpend[]>([]);
  const [reading, setReading] = useState(false);

  const load = () => {
    invoke<AccountSpend[]>('list_account_spend').then(setAccounts).catch((e) => toastError(String(e)));
    invoke<ProjectAccountSpend[]>('list_project_spend').then(setRows).catch(() => setRows([]));
  };

  useEffect(load, []);

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

          <span className={styles.sectionTitle}>where it went · last 7 days</span>
          {rows.length === 0 && (
            <span className={styles.dim}>
              No turns recorded yet. Numbers appear as each agent finishes a turn — Hangar reads what the CLI writes to its own transcript.
            </span>
          )}

          {rows.length > 0 && (
            <div className={styles.table}>
              <div className={styles.tableHead}>
                <span className={styles.colProject}>project</span>
                <span className={styles.colAccount}>account</span>
                <span className={styles.colNum}>in</span>
                <span className={styles.colNum}>out</span>
                <span className={styles.colNum}>cache</span>
                <span className={styles.colNum}>turns</span>
                <span className={styles.colNum}>cost</span>
              </div>
              {/* One row per project *and* account, because that pair is the
                  unit: "this project cost $12" doesn't say which subscription
                  is about to run out, and "this account is at 80%" doesn't say
                  what to stop doing. */}
              {rows.map((row) => (
                <div key={`${row.projectName}-${row.accountId}`} className={styles.tableRow}>
                  <span className={styles.colProject}>{row.projectName}</span>
                  <span className={styles.colAccount}>
                    {row.accountName} <span className={styles.dim}>{row.provider}</span>
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
