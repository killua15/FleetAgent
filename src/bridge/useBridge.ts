import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { supabase } from '../lib/supabase';
import { Bridge } from './pump';

interface BridgeStatus {
  machineId: string;
  machineName: string;
  device: { id: string; name: string; level: string } | null;
}

/**
 * Enciende el puente cuando hay con quién hablar, y lo apaga cuando no.
 *
 * Hay **dos** razones para estar encendido, y hacen falta las dos:
 *
 * - Un teléfono emparejado. La obvia.
 * - Una oferta de emparejamiento abierta. Menos obvia y es la que faltaba: el
 *   `hello` del teléfono llega por este mismo canal, así que esperar a tener un
 *   dispositivo emparejado para abrirlo dejaba el emparejamiento imposible —el
 *   saludo no tenía quién lo contestara, y en el Mac no aparecía ni el diálogo
 *   de aceptar.
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
  const reasons = useRef({ pairing: false, device: false, session: false });

  useEffect(() => {
    let cancelled = false;

    // Arrancar y parar son asíncronos y los disparan eventos que pueden llegar
    // pegados (aceptar emparejamiento manda dos). En cadena, para que no haya
    // dos transiciones a medias peleándose por `bridge.current`.
    let settling: Promise<void> = Promise.resolve();
    const settle = () => {
      settling = settling
        .then(async () => {
          const { pairing, device, session } = reasons.current;
          const wanted = !cancelled && session && (pairing || device);
          if (wanted === Boolean(bridge.current)) return;

          if (wanted) {
            const status = await invoke<BridgeStatus>('bridge_status');
            await register(status);
            const pump = new Bridge();
            await pump.start(status.machineId);
            bridge.current = pump;
          } else {
            const pump = bridge.current;
            bridge.current = null;
            await pump?.stop();
          }
        })
        // En silencio: sin red o sin emparejar son estados normales de una
        // instalación que no usa el móvil.
        .catch(() => {});
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
    ];

    return () => {
      cancelled = true;
      auth.data.subscription.unsubscribe();
      for (const subscription of subscriptions) void subscription.then((off) => off());
      settle();
    };
  }, []);
}

/**
 * Declara este Mac como propiedad de la cuenta que ha iniciado sesión.
 *
 * **Es lo que autoriza el canal.** La política de `realtime.messages` deja
 * entrar en `hangar:<id>` solo a quien tenga esa máquina a su nombre, así que
 * sin esta fila el canal privado rechaza la suscripción.
 *
 * Que falle no se traga: si el `machine_id` ya está a nombre de otra cuenta, la
 * fila no se puede escribir, y arrancar de todas formas daría un canal que no
 * suscribe y un error mucho más difícil de leer.
 */
async function register(status: BridgeStatus): Promise<void> {
  const { data } = await supabase.auth.getUser();
  const owner = data.user?.id;
  if (!owner) throw new Error('sin sesión no se puede registrar la máquina');

  const { error } = await supabase
    .from('machines')
    .upsert({ id: status.machineId, owner, name: status.machineName, last_seen_at: new Date().toISOString() }, { onConflict: 'id' });
  if (error) throw new Error(`no se pudo registrar este Mac: ${error.message}`);
}
