import { invoke } from '../lib/invoke';
import { supabase } from '../lib/supabase';
import type { RealtimeChannel } from '@supabase/supabase-js';

/** El sobre. Nada más viaja en claro — ver `MOBILE_BRIDGE_TRD.md` §4. */
export interface Envelope {
  v: number;
  id: string;
  kind: 'hello' | 'invoke' | 'result' | 'error' | 'event';
  nonce: string;
  payload: string;
}

/** Lo que Rust dice que hay que hacer con un mensaje entrante. */
type Decision =
  | { action: 'invoke'; id: string; name: string; args: unknown }
  | { action: 'reply'; envelope: Envelope }
  | { action: 'ignore' };

/** Los eventos de Hangar que el móvil recibe.
 *
 * `oauth-callback` no está y no puede estar: es local del escritorio y lleva un
 * código de autorización. */
const FORWARDED_EVENTS = [
  'terminal-state',
  'terminal-accounts-updated',
  'session-status',
  'session-ended',
  'session-output',
  'pr-comments-found',
  'queue-updated',
] as const;

/**
 * El puente, desde el lado del webview.
 *
 * **Esto es una tubería tonta y tiene que seguir siéndolo.** No sabe qué
 * transporta: recibe un sobre, se lo da a Rust, y hace lo que Rust le diga. El
 * cifrado, la lista blanca y el permiso están en Rust, del otro lado de un
 * `invoke`, porque el webview es la parte del sistema que un día ejecutará algo
 * que no escribimos nosotros.
 *
 * Vive aquí y no en Rust por una razón práctica: **no hay SDK de Supabase para
 * Rust y sí para JavaScript**, y ya está instalado. Escribir un cliente de
 * canales Phoenix en Rust serían días para reimplementar lo que el proyecto ya
 * tiene. El precio es que el puente solo vive mientras Hangar esté abierto —
 * aceptable, porque los agentes tampoco corren con la app cerrada.
 */
export class Bridge {
  private channel: RealtimeChannel | null = null;
  private unlisteners: Array<() => void> = [];

  async start(machineId: string): Promise<void> {
    if (this.channel) return;

    // `private: true` es lo que hace que Supabase consulte las políticas de
    // `realtime.messages` antes de dejar entrar. Sin esto el canal es público y
    // RLS se salta entero: cualquiera que averiguase el `machine_id` podría
    // suscribirse. El cifrado protege el contenido; esto protege el acceso.
    this.channel = supabase.channel(`hangar:${machineId}`, { config: { private: true, broadcast: { self: false } } });

    this.channel.on('broadcast', { event: 'msg' }, ({ payload }) => {
      void this.handle(payload as Envelope);
    });

    try {
      await this.subscribed(this.channel);
    } catch (e) {
      // Sin esto queda un canal medio abierto registrado en el cliente, y el
      // siguiente intento chocaría contra su propio tema.
      await this.stop();
      throw e;
    }
    await this.forwardEvents();
  }

  /**
   * Espera a estar suscrito de verdad.
   *
   * `subscribe()` devuelve el canal, no una promesa: hacerle `await` seguía
   * adelante al instante y `start()` resolvía con el canal todavía abriéndose.
   * Un broadcast que llega en ese hueco **se pierde y no se reintenta**, y es
   * justo el hueco del emparejamiento — el QR aparece en pantalla y el teléfono
   * saluda enseguida.
   *
   * Fallar también importa: un canal que no llegó a abrirse tiene que ser un
   * error, no un puente que se cree encendido y no recibe nada.
   */
  private subscribed(channel: RealtimeChannel): Promise<void> {
    return new Promise((resolve, reject) => {
      channel.subscribe((status) => {
        if (status === 'SUBSCRIBED') resolve();
        if (status === 'CHANNEL_ERROR' || status === 'TIMED_OUT' || status === 'CLOSED') {
          reject(new Error(`el canal del puente no se pudo abrir: ${status}`));
        }
      });
    });
  }

  async stop(): Promise<void> {
    for (const off of this.unlisteners) off();
    this.unlisteners = [];
    if (this.channel) {
      await supabase.removeChannel(this.channel);
      this.channel = null;
    }
  }

  /** Un mensaje entrante, de principio a fin. */
  private async handle(envelope: Envelope): Promise<void> {
    let decision: Decision;
    try {
      decision = await invoke<Decision>('bridge_receive', { envelope });
    } catch {
      // Rust rechazó el sobre antes de descifrarlo. No hay a quién contestar de
      // forma segura, así que se descarta en silencio: responder a un mensaje
      // que no se pudo autenticar es contestarle a cualquiera.
      return;
    }

    if (decision.action === 'ignore') return;
    if (decision.action === 'reply') {
      await this.publish(decision.envelope);
      return;
    }

    // El `invoke` de Tauri acepta el nombre como cadena, así que aquí no hay
    // tabla de despacho: **por esto un comando nuevo de Hangar llega al móvil
    // añadiendo una línea a `allowlist.json`**, sin tocar este fichero.
    try {
      const data = await invoke<unknown>(decision.name, (decision.args ?? {}) as Record<string, unknown>);
      await this.publish(await invoke<Envelope>('bridge_reply', { id: decision.id, ok: true, data }));
    } catch (e) {
      // El error del comando es lo único útil que hay: viaja tal cual.
      await this.publish(await invoke<Envelope>('bridge_reply', { id: decision.id, ok: false, data: String(e) }));
    }
  }

  /** Los eventos que ya mueven la interfaz del escritorio, reenviados. */
  private async forwardEvents(): Promise<void> {
    const { listen } = await import('@tauri-apps/api/event');

    for (const name of FORWARDED_EVENTS) {
      const unlisten = await listen(name, async (event) => {
        const envelope = await invoke<Envelope | null>('bridge_event', { name, data: event.payload });
        if (envelope) await this.publish(envelope);
      });
      this.unlisteners.push(unlisten);
    }
  }

  private async publish(envelope: Envelope): Promise<void> {
    await this.channel?.send({ type: 'broadcast', event: 'msg', payload: envelope });
  }
}
