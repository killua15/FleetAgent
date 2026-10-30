import { useEffect, useRef, useState } from 'react';
import { invoke } from '../../lib/invoke';
import { toastError } from '../../components/ErrorOverlay';
import type { Account } from '../../types/account';
import { brainLabel, EXTRA_TESTERS, GROUP_HINT, type FactoryAgent, type TeamView } from '../../types/factory';
import styles from '../Factory.module.css';

/**
 * The team, as wizard step 2 and the Equipo tab both show it: each agent with
 * its account and model, the testers with their command, and the test gate
 * that's always on. Every change is saved at once.
 */
export function TeamEditor({ projectId, onChange }: { projectId: string; onChange?: (view: TeamView) => void }) {
  const [view, setView] = useState<TeamView | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [models, setModels] = useState<Record<string, string[]>>({});
  // In a ref so a new callback each render doesn't reload the team.
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  useEffect(() => {
    invoke<TeamView>('factory_team_get', { projectId })
      .then((v) => {
        setView(v);
        onChangeRef.current?.(v);
      })
      .catch((e) => toastError(String(e)));
    invoke<Account[]>('list_accounts').then(setAccounts).catch(() => setAccounts([]));
    invoke<Record<string, string[]>>('factory_models').then(setModels).catch(() => setModels({}));
  }, [projectId]);

  const save = async (agents: FactoryAgent[]) => {
    setView((v) => (v ? { ...v, agents } : v));
    try {
      const saved = await invoke<TeamView>('factory_team_save', { projectId, agents });
      setView(saved);
      onChangeRef.current?.(saved);
    } catch (e) {
      toastError(String(e));
    }
  };

  if (!view) return null;
  const agents = view.agents;
  const patch = (id: string, change: Partial<FactoryAgent>) => save(agents.map((a) => (a.id === id ? { ...a, ...change } : a)));

  const chooseAccount = (a: FactoryAgent, accountId: string) => {
    const list = models[accountId] ?? [];
    patch(a.id, { accountId, model: list.includes(a.model) || list.length === 0 ? a.model : list[0] });
  };

  const extra = EXTRA_TESTERS.filter((t) => !agents.some((a) => a.name === t.name));
  const addTester = () => {
    const t = extra[0];
    if (!t) return;
    const last = agents.map((a) => a.group).lastIndexOf('Testers');
    const like = agents.find((a) => a.role === 'tester');
    const tester: FactoryAgent = {
      id: crypto.randomUUID(),
      role: 'tester',
      group: 'Testers',
      name: t.name,
      short: t.short,
      duty: t.duty,
      sections: ['cal'],
      accountId: like?.accountId ?? accounts[0]?.id ?? null,
      model: 'haiku',
      active: true,
      required: false,
      command: t.command,
      escalateModel: null,
      note: null,
      position: 0,
    };
    save([...agents.slice(0, last + 1), tester, ...agents.slice(last + 1)]);
  };

  const groups = [...new Set(agents.map((a) => a.group))];

  return (
    <div className={styles.team}>
      {(view.check.problems.length > 0 || view.check.warnings.length > 0) && (
        <div className={styles.col} style={{ gap: 4 }}>
          {view.check.problems.map((p) => (
            <span key={p} className={`${styles.hint} ${styles.bad}`}>
              ✗ {p}
            </span>
          ))}
          {view.check.warnings.map((w) => (
            <span key={w} className={`${styles.hint} ${styles.warn}`}>
              ⚠ {w}
            </span>
          ))}
        </div>
      )}
      {groups.map((g) => (
        <div key={g} className={styles.teamGroup}>
          <div className={styles.teamHead}>
            <span className={styles.sectionLabel}>{g}</span>
            <span className={styles.hint}>{GROUP_HINT[g]}</span>
          </div>
          {g === 'Testers' && (
            <div className={styles.gateCard}>
              <span style={{ fontSize: 16 }}>🧪</span>
              <div style={{ flex: 1 }}>
                <div className={styles.aName}>Puerta de tests · obligatoria</div>
                <div className={styles.hint}>
                  Cada criterio de aceptación se vuelve un test. Corren todos los testers activos; sin la suite en verde la orden vuelve a Fabricación y no se aprueba.
                </div>
              </div>
              <span className={styles.lock}>🔒 siempre activa</span>
            </div>
          )}
          {agents
            .filter((a) => a.group === g)
            .map((a) => {
              const list = (a.accountId && models[a.accountId]) || [a.model];
              return (
                <div key={a.id} className={a.active ? styles.agentRow : `${styles.agentRow} ${styles.agentOff}`}>
                  <button
                    className={a.active ? styles.switchOn : styles.switch}
                    disabled={a.required}
                    title={a.required ? 'obligatorio' : 'activar o desactivar'}
                    aria-pressed={a.active}
                    onClick={() => patch(a.id, { active: !a.active })}
                  />
                  <div>
                    <div className={styles.aName}>{a.name}</div>
                    <div className={styles.aRole}>
                      {a.duty}
                      {a.command && a.note ? ` · ${a.note}` : ''}
                    </div>
                  </div>
                  <span className={styles.aBrain}>brain: {brainLabel(a.sections)}</span>
                  <select className={styles.select} value={a.accountId ?? ''} title="cuenta" onChange={(e) => chooseAccount(a, e.target.value)}>
                    {!a.accountId && <option value="">elige una cuenta</option>}
                    {accounts.map((acc) => (
                      <option key={acc.id} value={acc.id}>
                        {acc.name}
                      </option>
                    ))}
                  </select>
                  <select className={styles.select} value={a.model} title="modelo" onChange={(e) => patch(a.id, { model: e.target.value })}>
                    {!list.includes(a.model) && <option value={a.model}>{a.model}</option>}
                    {list.map((m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ))}
                  </select>
                  {a.role === 'tester' ? (
                    <input
                      className={`${styles.input} ${styles.mono}`}
                      defaultValue={a.command ?? ''}
                      title="comando que corre su parte de la suite"
                      onBlur={(e) => e.target.value !== (a.command ?? '') && patch(a.id, { command: e.target.value })}
                    />
                  ) : a.role === 'builder' ? (
                    <select className={styles.select} value={a.escalateModel ?? ''} title="si falla dos veces seguidas" onChange={(e) => patch(a.id, { escalateModel: e.target.value || null })}>
                      <option value="">sin escalado</option>
                      {list.map((m) => (
                        <option key={m} value={m}>
                          si falla: escala a {m}
                        </option>
                      ))}
                    </select>
                  ) : (
                    <span className={styles.aRole}>{a.note ?? ''}</span>
                  )}
                </div>
              );
            })}
          {g === 'Testers' && extra.length > 0 && (
            <button className={`${styles.btn} ${styles.sm}`} style={{ alignSelf: 'flex-start', marginTop: 8 }} onClick={addTester}>
              + Añadir tester
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
