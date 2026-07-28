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
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    invoke<AccountSpend[]>('list_account_spend').then(setAccounts).catch((e) => toastError(String(e)));
  }, []);

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
          <span className={styles.subtitle}>this 5-hour window and the last 7 days, per account</span>
          <span className={styles.spacer} />
          <Button size="sm" variant="outline" onClick={onClose}>
            close
          </Button>
        </div>

        <div className={styles.body}>
          {!accounts && <span className={styles.dim}>reading…</span>}
          {accounts?.length === 0 && <span className={styles.dim}>No accounts yet.</span>}

          {accounts?.map((account) => {
            const subscription = account.billing === 'sub';
            const open = expanded === account.accountId;
            return (
              <div key={account.accountId} className={styles.account}>
                <button className={styles.accountHead} onClick={() => setExpanded(open ? null : account.accountId)}>
                  <span className={styles.name}>{account.accountName}</span>
                  <span className={styles.provider}>{account.provider}</span>

                  {subscription ? (
                    <>
                      {/* Quota leads: it's what actually stops you working. */}
                      <span className={styles.window}>
                        session
                        {account.sessionPct === null ? <span className={styles.dim}>—</span> : <Bar pct={account.sessionPct} />}
                        <span className={styles.pct}>{account.sessionPct === null ? 'no reading' : `${Math.round(account.sessionPct)}%`}</span>
                      </span>
                      <span className={styles.window}>
                        week
                        {account.weeklyPct === null ? <span className={styles.dim}>—</span> : <Bar pct={account.weeklyPct} />}
                        <span className={styles.pct}>{account.weeklyPct === null ? 'no reading' : `${Math.round(account.weeklyPct)}%`}</span>
                      </span>
                    </>
                  ) : (
                    <span className={styles.window}>
                      <span className={styles.pct}>{money(account.week.costUsd)}</span>
                      <span className={styles.dim}>billed this week</span>
                    </span>
                  )}

                  {account.hoursLeft !== null && <span className={styles.hours}>~{account.hoursLeft.toFixed(1)} h left</span>}
                  <span className={styles.chevron}>{open ? '▾' : '▸'}</span>
                </button>

                <div className={styles.totals}>
                  <span className={styles.totalsLabel}>session</span>
                  <span className={styles.tokens}>
                    {compact(account.session.inputTokens)} in · {compact(account.session.outputTokens)} out · {account.session.turns} turns
                  </span>
                  <span className={styles.spacer} />
                  <span className={styles.cost}>
                    {money(account.session.costUsd)}
                    {subscription && <span className={styles.equivalent}> equivalent</span>}
                  </span>
                </div>

                <div className={styles.totals}>
                  <span className={styles.totalsLabel}>7 days</span>
                  <span className={styles.tokens}>
                    {compact(account.week.inputTokens)} in · {compact(account.week.outputTokens)} out · {account.week.turns} turns
                  </span>
                  <span className={styles.spacer} />
                  <span className={styles.cost}>
                    {money(account.week.costUsd)}
                    {subscription && <span className={styles.equivalent}> equivalent</span>}
                  </span>
                </div>

                {subscription && (
                  <div className={styles.calibration}>
                    {account.weekPctEquivalent !== null && account.calibration ? (
                      <>
                        ≈ <strong>{account.weekPctEquivalent.toFixed(1)}%</strong> of your weekly quota, measured at{' '}
                        {account.calibration.pctPerDollar.toFixed(1)}% per $ from {account.calibration.spans} reading
                        {account.calibration.spans === 1 ? '' : 's'} of your own
                      </>
                    ) : (
                      // Said plainly, because an invented conversion would be
                      // believed: nobody publishes how tokens map to a plan's
                      // quota, so this waits for two readings in one window.
                      <>calibrating — needs two quota readings in the same window before tokens can be expressed as %</>
                    )}
                  </div>
                )}

                {open && (
                  <div className={styles.breakdown}>
                    <div className={styles.column}>
                      <span className={styles.columnTitle}>by project</span>
                      {account.byProject.length === 0 && <span className={styles.dim}>nothing yet</span>}
                      {account.byProject.map((row) => (
                        <div key={row.projectName} className={styles.row}>
                          <span className={styles.rowName}>{row.projectName}</span>
                          <span className={styles.rowValue}>
                            {money(row.costUsd)} · {row.turns}
                          </span>
                        </div>
                      ))}
                    </div>
                    <div className={styles.column}>
                      <span className={styles.columnTitle}>by model</span>
                      {account.byModel.length === 0 && <span className={styles.dim}>nothing yet</span>}
                      {account.byModel.map((row) => (
                        <div key={row.model} className={styles.row}>
                          <span className={styles.rowName}>{row.model}</span>
                          <span className={styles.rowValue}>
                            {money(row.costUsd)} · {row.turns}
                          </span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            );
          })}

          {accounts && accounts.every((a) => a.week.turns === 0) && (
            <span className={styles.dim}>
              No turns recorded yet. Numbers appear as each agent finishes a turn — Hangar reads what the CLI writes to its own transcript.
            </span>
          )}
        </div>
      </div>
    </div>
  );
}
