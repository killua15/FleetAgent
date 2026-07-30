import { useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
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
 * Se apaga cuando no queda ninguna: abrir un canal para nadie es gastar una
 * conexión y anunciarse como presente donde no hace falta estar.
 */
export function useBridge(): void {
  const bridge = useRef<Bridge | null>(null);
  const reasons = useRef({ pairing: false, device: false });

  useEffect(() => {
    let cancelled = false;

    // Arrancar y parar son asíncronos y los disparan eventos que pueden llegar
    // pegados (aceptar emparejamiento manda dos). En cadena, para que no haya
    // dos transiciones a medias peleándose por `bridge.current`.
    let settling: Promise<void> = Promise.resolve();
    const settle = () => {
      settling = settling
        .then(async () => {
          const wanted = !cancelled && (reasons.current.pairing || reasons.current.device);
          if (wanted === Boolean(bridge.current)) return;

          if (wanted) {
            const status = await invoke<BridgeStatus>('bridge_status');
            const pump = new Bridge();
            await pump.start(status.machineId);
            bridge.current = pump;
          } else {
            const pump = bridge.current;
            bridge.current = null;
            await pump?.stop();
          }
        })
        // En silencio: sin sesión de Supabase, sin red, o sin emparejar son los
        // tres estados normales de una instalación que no usa el móvil.
        .catch(() => {});
    };

    invoke<BridgeStatus>('bridge_status')
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
      for (const subscription of subscriptions) void subscription.then((off) => off());
      settle();
    };
  }, []);
}
