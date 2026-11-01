import { useEffect, useRef, useState } from 'react';
import { invoke } from '../../lib/invoke';
import { toastError } from '../../components/ErrorOverlay';
import { TerminalPane } from '../../components/TerminalPane';
import type { Account } from '../../types/account';
import type { TerminalInfo } from '../../types/terminal';
import styles from '../Factory.module.css';

/** The definition session's terminal per project. The PTY outlives this
 *  component, so going to another step and back finds the same conversation. */
const sessions = new Map<string, string>();

/** Types a message into the project's open brain session — the amendment
 *  one first — and says whether there was one. */
export function sendToBrainSession(projectId: string, text: string): boolean {
  const id = sessions.get(`${projectId}:amend`) ?? sessions.get(`${projectId}:define`);
  if (!id) return false;
  invoke('write_terminal', { id, data: `${text.replace(/\s*\n\s*/g, ' ')}\n` }).catch((e) => toastError(String(e)));
  return true;
}

/**
 * «Conversación aquí»: a real Hangar terminal in the project, with the CLI
 * told what the conversation is for. «Generar BRAIN.md» asks it to write the
 * file where Hangar looks, and the file comes back through `onGenerated` —
 * the same import every other way in uses.
 */
export function BrainSession({
  projectId,
  base,
  amend,
  onGenerated,
}: {
  projectId: string;
  /** A draft to complete, when coming from an import with sections missing. */
  base?: string | null;
  /** Start from the sealed version and the inbox, for vN+1. */
  amend?: boolean;
  onGenerated: (markdown: string) => void;
}) {
  const key = `${projectId}:${amend ? 'amend' : 'define'}`;
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [accountId, setAccountId] = useState('');
  const [terminalId, setTerminalId] = useState<string | null>(sessions.get(key) ?? null);
  const [opening, setOpening] = useState(false);
  const [waiting, setWaiting] = useState(false);
  const onGeneratedRef = useRef(onGenerated);
  onGeneratedRef.current = onGenerated;

  useEffect(() => {
    invoke<Account[]>('list_accounts')
      .then((all) => {
        setAccounts(all);
        setAccountId((all.find((a) => a.provider === 'claude') ?? all[0])?.id ?? '');
      })
      .catch(() => setAccounts([]));
  }, []);

  // While the session writes the file, look for it every couple of seconds.
  useEffect(() => {
    if (!waiting) return;
    const started = Date.now();
    const timer = setInterval(() => {
      invoke<string | null>('factory_brain_read_generated', { projectId })
        .then((text) => {
          if (text) {
            setWaiting(false);
            onGeneratedRef.current(text);
          } else if (Date.now() - started > 15 * 60_000) {
            setWaiting(false);
          }
        })
        .catch(() => undefined);
    }, 2000);
    return () => clearInterval(timer);
  }, [waiting, projectId]);

  const open = async () => {
    if (!accountId) return;
    setOpening(true);
    try {
      const info = await invoke<TerminalInfo>('factory_brain_session', { projectId, accountId, base: base ?? null, amend: !!amend });
      sessions.set(key, info.id);
      setTerminalId(info.id);
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };

  const generate = async () => {
    if (!terminalId) return;
    try {
      await invoke('factory_brain_generate', { terminalId });
      setWaiting(true);
    } catch (e) {
      // The terminal was closed elsewhere: start over.
      sessions.delete(key);
      setTerminalId(null);
      toastError(String(e));
    }
  };

  const account = accounts.find((a) => a.id === accountId);

  return (
    <div className={`${styles.cardFlat} ${styles.chat}`}>
      <div className={styles.cardHead} style={{ borderBottom: '1px solid var(--line)' }}>
        <span className={styles.sectionLabel}>{amend ? 'SESIÓN DE ENMIENDA' : 'SESIÓN DE DEFINICIÓN'} · terminal</span>
        <span className={styles.spacer} />
        {terminalId ? (
          <span className={styles.chip}>{account ? `${account.name} · lee el repo` : 'en marcha'}</span>
        ) : (
          <select className={styles.select} style={{ maxWidth: 220 }} value={accountId} onChange={(e) => setAccountId(e.target.value)}>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name} · {a.provider}
              </option>
            ))}
          </select>
        )}
      </div>
      {terminalId ? (
        <TerminalPane terminalId={terminalId} className={styles.terminal} />
      ) : (
        <div className={styles.sessionEmpty}>
          <span className={styles.hint}>
            Una terminal real en el proyecto. La IA lee el repo y recorre contigo, sección a sección, lo que el proyecto necesita de principio a producción. Hablas con
            ella con libertad; al final, «Generar BRAIN.md».
          </span>
          <button className={styles.btnPrimary} disabled={!accountId || opening} onClick={open}>
            {opening ? 'abriendo…' : amend ? 'Abrir la sesión de enmienda' : 'Abrir la sesión'}
          </button>
          {accounts.length === 0 && <span className={`${styles.hint} ${styles.warn}`}>No hay cuentas en Hangar: añade una para conversar con la IA.</span>}
        </div>
      )}
      <div className={styles.chatFoot}>
        <span className={styles.hint}>
          {waiting ? 'esperando a que la IA escriba .hangar/brain/BRAIN.md…' : 'Cuando esté todo hablado, la IA escribe el archivo y Hangar lo importa.'}
        </span>
        <span className={styles.spacer} />
        <button className={styles.btnPrimary} disabled={!terminalId || waiting} onClick={generate}>
          {waiting ? 'generando…' : 'Generar BRAIN.md'}
        </button>
      </div>
    </div>
  );
}
