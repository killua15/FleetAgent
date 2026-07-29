import { useEffect, useRef } from 'react';
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
 * Se conecta **solo si hay un dispositivo emparejado**: abrir un canal para
 * nadie es gastar una conexión y anunciarse como presente en un sitio donde no
 * hace falta estar.
 */
export function useBridge(): void {
  const bridge = useRef<Bridge | null>(null);

  useEffect(() => {
    let cancelled = false;

    invoke<BridgeStatus>('bridge_status')
      .then(async (status) => {
        if (cancelled || !status.device) return;
        bridge.current = new Bridge();
        await bridge.current.start(status.machineId);
      })
      // En silencio: sin sesión de Supabase, sin red, o sin emparejar son los
      // tres estados normales de una instalación que no usa el móvil.
      .catch(() => {});

    return () => {
      cancelled = true;
      void bridge.current?.stop();
      bridge.current = null;
    };
  }, []);
}
