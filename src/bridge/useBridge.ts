import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { supabase } from '../lib/supabase';
import { Bridge } from './pump';
import { registerThisMachine, syncMachines } from './machines';

/** How often a host re-reads the account's machines, so a machine added later
 * (or a key that changed) is known without restarting. */
const PEER_REFRESH_MS = 60_000;
/** First wait before retrying a bridge that failed to start; doubles up to the max. */
const RETRY_START_MS = 15_000;
const RETRY_MAX_MS = 60_000;

interface BridgeStatus {
  machineId: string;
  machineName: string;
  device: { id: string; name: string; level: string } | null;
}

/**
 * Enciende el puente cuando hay con quién hablar, y lo apaga cuando no.
 *
 * Hay **tres** razones para estar encendido (basta una):
 *
 * - Un teléfono emparejado. La obvia.
 * - Una oferta de emparejamiento abierta. Menos obvia y es la que faltaba: el
 *   `hello` del teléfono llega por este mismo canal, así que esperar a tener un
 *   dispositivo emparejado para abrirlo dejaba el emparejamiento imposible —el
 *   saludo no tenía quién lo contestara, y en el Mac no aparecía ni el diálogo
 *   de aceptar.
 * - Acceso remoto activado: las otras máquinas de la cuenta llaman a este
 *   mismo canal (`bridge/peers.rs`).
 *
 * Y una condición por encima de las dos: **sesión de Supabase**. El canal es
 * privado, así que sin sesión no hay a quién autorizar. Antes funcionaba sin
 * cuenta porque el canal era público, y eso significaba que cualquiera que
 * averiguase un `machine_id` podía entrar: el cifrado protege el contenido, no
 * el acceso.
 *
 * Se apaga cuando falta cualquiera de las tres: abrir un canal para nadie es
 * gastar una conexión y anunciarse como presente donde no hace falta estar.
 */
export function useBridge(): void {
  const bridge = useRef<Bridge | null>(null);
  const reasons = useRef({ pairing: false, device: false, remoteAccess: false, session: false });

  useEffect(() => {
    let cancelled = false;

    // Arrancar y parar son asíncronos y los disparan eventos que pueden llegar
    // pegados (aceptar emparejamiento manda dos). En cadena, para que no haya
    // dos transiciones a medias peleándose por `bridge.current`.
    let settling: Promise<void> = Promise.resolve();
    // A bridge that failed to start is retried on its own, with backoff: the
    // channel can be refused for a while (Realtime waking up, no network), and
    // a machine whose bridge gave up once used to stay deaf until restarted.
    let retryTimer: number | null = null;
    let retryDelay = RETRY_START_MS;
    const clearRetry = () => {
      if (retryTimer !== null) window.clearTimeout(retryTimer);
      retryTimer = null;
    };
    const settle = () => {
      settling = settling
        .then(async () => {
          const { pairing, device, remoteAccess, session } = reasons.current;
          const wanted = !cancelled && session && (pairing || device || remoteAccess);
          if (!wanted) {
            clearRetry();
            retryDelay = RETRY_START_MS;
          }
          if (wanted === Boolean(bridge.current)) return;

          if (wanted) {
            const status = await invoke<BridgeStatus>('bridge_status');
            await registerThisMachine();
            // Before the channel opens: a machine calling in is only answered
            // once its key is known.
            // Not fatal: a failure here must not take the phone's bridge down.
            if (remoteAccess) await syncMachines().catch((e) => console.warn('[puente] no se pudieron leer las máquinas:', e));
            const pump = new Bridge();
            await pump.start(status.machineId);
            bridge.current = pump;
            clearRetry();
            retryDelay = RETRY_START_MS;
          } else {
            const pump = bridge.current;
            bridge.current = null;
            await pump?.stop();
          }
        })
        // Sin toast: no tener el móvil configurado es un estado normal y no
        // merece una alerta en cada arranque. Pero **no en silencio**: tragarse
        // esto hizo que "sin sesión de Supabase" y "el puente funciona" se
        // vieran exactamente igual, y diagnosticarlo costó mirar la base de
        // datos del servidor.
        .catch((e) => {
          console.warn('[puente] no se pudo abrir el canal:', e);
          if (cancelled || retryTimer !== null) return;
          console.warn(`[puente] reintento en ${retryDelay / 1000} s`);
          retryTimer = window.setTimeout(() => {
            retryTimer = null;
            settle();
          }, retryDelay);
          retryDelay = Math.min(retryDelay * 2, RETRY_MAX_MS);
        });
    };

    // La sesión, ahora y cuando cambie. `setAuth` también en cada refresco: el
    // token que Realtime tiene guardado caduca aunque la sesión siga viva, y un
    // canal con un token caducado se cae en la siguiente reconexión.
    void supabase.auth.getSession().then(({ data }) => {
      reasons.current.session = Boolean(data.session);
      if (data.session) supabase.realtime.setAuth(data.session.access_token);
      settle();
    });

    const auth = supabase.auth.onAuthStateChange((_event, session) => {
      reasons.current.session = Boolean(session);
      if (session) supabase.realtime.setAuth(session.access_token);
      settle();
    });

    void invoke<BridgeStatus>('bridge_status')
      .then((status) => {
        reasons.current.device = Boolean(status.device);
        settle();
      })
      .catch(() => {});

    void invoke<{ remoteAccess: boolean }>('bridge_machine_identity')
      .then((identity) => {
        reasons.current.remoteAccess = identity.remoteAccess;
        settle();
      })
      .catch(() => {});

    // Keeps the host's list of the account's machines fresh while it accepts
    // them. Quiet on failure: being offline is normal.
    const refresh = window.setInterval(() => {
      if (bridge.current && reasons.current.remoteAccess) void syncMachines().catch(() => {});
    }, PEER_REFRESH_MS);

    const subscriptions = [
      listen<{ paired: boolean }>('bridge-device-changed', (event) => {
        reasons.current.device = event.payload.paired;
        settle();
      }),
      listen('bridge-pairing-open', () => {
        reasons.current.pairing = true;
        settle();
      }),
      listen('bridge-pairing-closed', () => {
        reasons.current.pairing = false;
        settle();
      }),
      listen<{ enabled: boolean }>('bridge-remote-access-changed', (event) => {
        reasons.current.remoteAccess = event.payload.enabled;
        // Already open for the phone: the channel stays, but the peer keys
        // have to be loaded now or the first call goes unanswered.
        if (event.payload.enabled && bridge.current) void syncMachines().catch(() => {});
        settle();
      }),
    ];

    return () => {
      cancelled = true;
      clearRetry();
      window.clearInterval(refresh);
      auth.data.subscription.unsubscribe();
      for (const subscription of subscriptions) void subscription.then((off) => off());
      settle();
    };
  }, []);
}
