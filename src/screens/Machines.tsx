import { useEffect, useMemo, useRef, useState } from 'react';
import { AppTopBar } from '../components/AppTopBar';
import { Button } from '../components/ui/button';
import { Spinner } from '../components/Spinner';
import { TerminalPane } from '../components/TerminalPane';
import { remoteTransport } from '../components/terminalTransport';
import { registerThisMachine, syncMachines, type MachineIdentity, type MachineRow } from '../bridge/machines';
import { closeRemoteMachine, remoteMachine, RemoteError, type RemoteMachine } from '../bridge/remote';
import { TERMINAL_STATE_META, type TerminalActivity, type TerminalInfo } from '../types/terminal';
import styles from './Machines.module.css';

type Link =
  | { state: 'connecting' }
  | { state: 'online' }
  | { state: 'disabled' }
  | { state: 'unreachable'; message: string };

/**
 * The account's other machines, and their terminals, from this one.
 *
 * Nothing to pair: every machine signed in to the same account publishes a
 * public key in `public.machines`, and two machines derive their shared key
 * from each other's (`bridge/peers.rs`). The other machine has to have
 * "Permitir que mis otras máquinas controlen esta" turned on, in front of it.
 * Free on every plan: these are the user's own machines (TRD §12.14).
 */
export function Machines() {
  const [self, setSelf] = useState<MachineIdentity | null>(null);
  const [machines, setMachines] = useState<MachineRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const opened = useRef(new Set<string>());

  const load = async () => {
    setError(null);
    try {
      // Publishing this machine's key first: without it the other machine
      // cannot derive a key with this one and never answers.
      setSelf(await registerThisMachine());
      setMachines(await syncMachines());
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    }
  };

  useEffect(() => {
    void load();
    const connections = opened.current;
    return () => {
      for (const id of connections) void closeRemoteMachine(id);
    };
  }, []);

  const choose = (id: string) => {
    opened.current.add(id);
    setSelected(id);
  };

  const machine = machines?.find((m) => m.id === selected) ?? null;

  return (
    <div className={styles.screen}>
      <AppTopBar />
      <div className={styles.body}>
        <aside className={styles.list}>
          <div className={styles.listHeader}>
            <span className={styles.sectionLabel}>MACHINES</span>
            <Button size="sm" variant="ghost" onClick={() => void load()}>
              refresh
            </Button>
          </div>

          {error && <span className={styles.error}>{error}</span>}
          {!error && machines === null && <Spinner label="loading…" />}
          {machines?.length === 0 && (
            <span className={styles.hint}>
              No hay otras máquinas en esta cuenta. Abre Hangar con la misma cuenta en otra máquina y activa allí el acceso remoto en Ajustes.
            </span>
          )}
          {machines?.map((m) => (
            <button key={m.id} className={m.id === selected ? styles.machineActive : styles.machine} onClick={() => choose(m.id)} disabled={!m.fingerprint}>
              <span className={styles.machineName}>{m.name}</span>
              <span className={styles.machineMeta}>
                {m.fingerprint ? m.fingerprint : 'Hangar demasiado viejo: actualízalo para usarlo desde aquí'}
              </span>
              {m.lastSeenAt && <span className={styles.machineMeta}>visto {new Date(m.lastSeenAt).toLocaleString()}</span>}
            </button>
          ))}

          {self && (
            <span className={styles.hint}>
              Esta máquina: <strong>{self.machineName}</strong> · <code>{self.fingerprint}</code>
            </span>
          )}
        </aside>

        <main className={styles.detail}>
          {machine ? <MachineDetail key={machine.id} machine={machine} /> : <span className={styles.hint}>Elige una máquina.</span>}
        </main>
      </div>
    </div>
  );
}

function MachineDetail({ machine }: { machine: MachineRow }) {
  const remote = useMemo(() => remoteMachine(machine.id), [machine.id]);
  const [link, setLink] = useState<Link>({ state: 'connecting' });
  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [activity, setActivity] = useState<Record<string, TerminalActivity>>({});
  const [open, setOpen] = useState<string | null>(null);
  /** Bumped by "retry" to run the connection again. */
  const [attempt, setAttempt] = useState(0);

  const refresh = async (target: RemoteMachine) => {
    const [list, states] = await Promise.all([
      target.call<TerminalInfo[]>('list_terminals'),
      target.call<TerminalActivity[]>('list_terminal_states'),
    ]);
    setTerminals(list);
    setActivity(Object.fromEntries(states.map((a) => [a.id, a])));
  };

  useEffect(() => {
    let cancelled = false;
    setLink({ state: 'connecting' });
    remote
      .connect()
      .then(async (hello) => {
        if (cancelled) return;
        if (hello.remoteAccess === false) {
          setLink({ state: 'disabled' });
          return;
        }
        setLink({ state: 'online' });
        await refresh(remote);
      })
      .catch((e) => {
        if (cancelled) return;
        const message = e instanceof RemoteError ? `${e.code}: ${e.message}` : String(e instanceof Error ? e.message : e);
        setLink({ state: 'unreachable', message });
      });

    const offs = [
      remote.on<TerminalActivity>('terminal-state', (state) => setActivity((prev) => ({ ...prev, [state.id]: state }))),
      // The set of terminals changes with these; re-read the list.
      remote.on('terminal-accounts-updated', () => void refresh(remote).catch(() => {})),
    ];
    return () => {
      cancelled = true;
      for (const off of offs) off();
    };
  }, [remote, attempt]);

  const transport = useMemo(() => remoteTransport(remote), [remote]);

  return (
    <div className={styles.detailInner}>
      <div className={styles.detailHeader}>
        <span className={styles.detailName}>{machine.name}</span>
        <span className={styles.machineMeta}>{machine.fingerprint}</span>
        <span className={styles.linkState} data-state={link.state}>
          {link.state === 'connecting' && 'conectando…'}
          {link.state === 'online' && 'en línea'}
          {link.state === 'disabled' && 'acceso remoto desactivado allí'}
          {link.state === 'unreachable' && 'sin respuesta'}
        </span>
        {link.state === 'online' && (
          <Button size="sm" variant="ghost" onClick={() => void refresh(remote).catch(() => {})}>
            refresh
          </Button>
        )}
        {(link.state === 'unreachable' || link.state === 'disabled') && (
          <Button size="sm" variant="ghost" onClick={() => setAttempt((n) => n + 1)}>
            retry
          </Button>
        )}
      </div>

      {link.state === 'disabled' && (
        <span className={styles.hint}>
          La máquina responde, pero no acepta control remoto. Actívalo en ella: Ajustes → Teléfono → «Permitir que mis otras máquinas controlen esta».
        </span>
      )}
      {link.state === 'unreachable' && (
        <span className={styles.hint}>
          {link.message}. Comprueba que esté despierta, con Hangar abierto, la misma cuenta y el acceso remoto activado.
        </span>
      )}

      {link.state === 'online' && (
        <>
          <div className={styles.terminals}>
            {terminals.length === 0 && <span className={styles.hint}>No hay terminales abiertas allí.</span>}
            {terminals.map((t) => {
              const state = activity[t.id];
              const meta = state ? TERMINAL_STATE_META[state.state] : null;
              return (
                <button key={t.id} className={t.id === open ? styles.terminalActive : styles.terminal} onClick={() => setOpen(t.id === open ? null : t.id)}>
                  {meta && <span className={styles.dot} style={{ background: meta.color }} title={meta.label} />}
                  <span className={styles.terminalTitle}>{state?.title || t.title}</span>
                  {t.accountName && <span className={styles.machineMeta}>{t.accountName}</span>}
                </button>
              );
            })}
          </div>
          {open && (
            <div className={styles.pane}>
              <TerminalPane key={open} terminalId={open} transport={transport} />
            </div>
          )}
        </>
      )}
    </div>
  );
}
