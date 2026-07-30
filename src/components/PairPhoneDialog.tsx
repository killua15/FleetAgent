import { useEffect, useRef, useState } from 'react';
import QRCode from 'qrcode';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { toastError } from './ErrorOverlay';
import { Button } from './ui/button';
import styles from './PairPhoneDialog.module.css';

type Level = 'watch' | 'steer' | 'launch';

interface PairingOffer {
  machineId: string;
  machineName: string;
  key: string;
  level: Level;
  expiresIn: number;
}

const LEVELS: Array<{ id: Level; label: string; blurb: string }> = [
  { id: 'watch', label: 'Mirar', blurb: 'Estado, cuota, diffs. No escribe nada.' },
  { id: 'steer', label: 'Dirigir', blurb: 'Prompts, responder permisos, parar un agente.' },
  { id: 'launch', label: 'Lanzar', blurb: 'Además abre worktrees y manda PRs a revisar.' },
];

/**
 * Empareja un teléfono con este Mac.
 *
 * **La clave viaja en el QR y solo ahí** — un canal que se mira con los ojos y
 * no atraviesa ningún servidor, que es lo que permite que el resto sea cifrado
 * extremo a extremo sin negociar claves por red.
 *
 * **Y escanear no es autorizar**: aunque el código sea correcto, hay que
 * aceptar aquí. Un QR fotografiado por encima del hombro no basta.
 */
export function PairPhoneDialog({ onClose, onPaired }: { onClose: () => void; onPaired: () => void }) {
  const [level, setLevel] = useState<Level>('steer');
  const [offer, setOffer] = useState<PairingOffer | null>(null);
  const [secondsLeft, setSecondsLeft] = useState(0);
  const [asking, setAsking] = useState<string | null>(null);
  const canvas = useRef<HTMLCanvasElement>(null);

  const open = async (chosen: Level) => {
    try {
      const created = await invoke<PairingOffer>('bridge_begin_pairing', { level: chosen });
      setOffer(created);
      setSecondsLeft(created.expiresIn);
    } catch (e) {
      toastError(String(e));
    }
  };

  // El QR lleva lo mínimo para conectar y descifrar. Nada más: cada campo de
  // aquí es algo que queda en una foto.
  const payload = offer ? JSON.stringify({ m: offer.machineId, n: offer.machineName, k: offer.key, lvl: offer.level }) : '';

  useEffect(() => {
    if (!offer || !canvas.current) return;
    void QRCode.toCanvas(canvas.current, payload, { width: 260, margin: 1 });
  }, [offer, payload]);

  // Caduca a la vista, no solo en el backend: un QR en pantalla que ya no sirve
  // es peor que ninguno.
  useEffect(() => {
    if (!offer || secondsLeft <= 0) return;
    const timer = window.setTimeout(() => setSecondsLeft((n) => n - 1), 1000);
    return () => window.clearTimeout(timer);
  }, [offer, secondsLeft]);

  useEffect(() => {
    const unlisten = listen<{ device: string }>('bridge-pairing-request', (event) => setAsking(event.payload.device));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // Cerrar sin emparejar cierra también la oferta: dejar una clave viva porque
  // alguien pulsó Escape es dejar una puerta abierta sin que se vea.
  const close = () => {
    void invoke('bridge_cancel_pairing').catch(() => {});
    onClose();
  };

  const accept = async () => {
    try {
      await invoke('bridge_accept_pairing', { name: asking ?? 'teléfono' });
      onPaired();
      onClose();
    } catch (e) {
      toastError(String(e));
    }
  };

  const expired = Boolean(offer) && secondsLeft <= 0;

  return (
    <div className={styles.backdrop} onClick={close}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <span className={styles.title}>Emparejar un teléfono</span>

        {!offer && (
          <>
            <span className={styles.hint}>Qué podrá hacer este dispositivo. Se puede revocar en cualquier momento.</span>
            <div className={styles.levels}>
              {LEVELS.map((option) => (
                <button key={option.id} className={level === option.id ? styles.levelActive : styles.level} onClick={() => setLevel(option.id)}>
                  <span className={styles.levelLabel}>{option.label}</span>
                  <span className={styles.levelBlurb}>{option.blurb}</span>
                </button>
              ))}
            </div>
            <div className={styles.actions}>
              <Button size="sm" variant="outline" onClick={close}>
                cancelar
              </Button>
              <Button size="sm" onClick={() => open(level)}>
                Mostrar el QR
              </Button>
            </div>
          </>
        )}

        {offer && !asking && (
          <>
            <canvas ref={canvas} className={expired ? styles.qrExpired : styles.qr} />
            {expired ? (
              <>
                <span className={styles.expired}>El código caducó. Una captura de pantalla no debe seguir sirviendo mañana.</span>
                <Button size="sm" onClick={() => open(offer.level)}>
                  Generar otro
                </Button>
              </>
            ) : (
              <>
                <span className={styles.hint}>
                  Escanéalo desde la app · caduca en {secondsLeft}s · nivel <strong>{offer.level}</strong>
                </span>
                {/* Para probar el puente sin app: el mismo contenido, en texto.
                    No expone nada nuevo — quien pueda fotografiar el QR ya tiene
                    la clave — y es lo que permitió terminar el escritorio antes
                    de que existiera un cliente. */}
                <button className={styles.copyLink} onClick={() => void navigator.clipboard.writeText(payload)}>
                  copiar para el script de prueba
                </button>
              </>
            )}
          </>
        )}

        {asking && (
          <div className={styles.request}>
            <span className={styles.requestName}>{asking}</span>
            <span className={styles.hint}>
              quiere emparejarse con nivel <strong>{offer?.level}</strong>. Escaneó el código correcto — aceptar es lo que le da acceso.
            </span>
            <div className={styles.actions}>
              <Button size="sm" variant="outline" onClick={() => setAsking(null)}>
                rechazar
              </Button>
              <Button size="sm" onClick={accept}>
                Aceptar
              </Button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
