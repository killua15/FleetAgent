# TDD 1 · El puente

**Fase**: 1 · **Lado**: solo Hangar · **Estimación**: 1 semana
**Depende de**: TDD 0, y de la Fase 5 (login con Google) para autenticar el canal.
**Contrato**: TRD §3, §5, §6

---

## Objetivo

Que un cliente remoto autenticado ejecute comandos de Hangar y reciba sus
eventos, por internet, cifrado, sin abrir un puerto.

Al terminar, **desde otra máquina** y con el script de la Fase 0, se ve la flota
y se manda un prompt. Sin app móvil todavía — ese es el punto: Hangar queda
terminado antes de que exista el cliente.

## La decisión de diseño que lo define

**No hay SDK de Supabase para Rust. Sí lo hay para JavaScript, y ya está
instalado** (`@supabase/supabase-js` en `package.json`).

Escribir un cliente de canales Phoenix en Rust son días de trabajo para
reimplementar algo que ya está en el proyecto. Así que:

```
   webview (JS)                      Rust
   ────────────                      ────
   supabase-js  ──── blob ────►  descifra, valida, autoriza
        ▲                              │
        │                              ▼
        │                        { name, args }   ← solo si está permitido
        │                              │
        └──── invoke(name, args) ◄─────┘
                    │
                    ▼
              el mismo handler que usa la app de escritorio
                    │
        ──── result ───►  Rust cifra  ──── blob ────►  supabase-js publica
```

**El webview es una tubería tonta.** No sabe qué transporta: recibe blobs, los
pasa a Rust, y publica lo que Rust le devuelve. El cifrado, la lista blanca y la
autorización viven en Rust, que es donde tienen que vivir.

Y de aquí sale el regalo: **el despacho es gratis**. `invoke(name, args)` de
Tauri ya acepta el nombre como cadena, así que no hay que escribir una tabla de
despacho ni un `match` de 135 ramas. Un comando nuevo en Hangar queda accesible
añadiendo una línea a la lista blanca.

**Lo que esto cuesta, dicho claro**: el puente solo vive mientras Hangar esté
abierto. Es aceptable —los agentes tampoco corren con la app cerrada— y coincide
con la limitación que el TRD ya asume: con el Mac dormido no hay puente.

## Entregables

```
src-tauri/src/bridge/dispatch.rs     descifrar → autorizar → devolver al webview
src-tauri/src/bridge/session.rs      estado del canal, dispositivo conectado
src/bridge/channel.ts                supabase-js: suscripción y publicación
src/bridge/pump.ts                   el bucle: blob → invoke → blob
```

## Diseño

### Canal y autenticación

`hangar:<machine_id>`, con `machine_id` guardado en `app_settings` — un UUID, no
el hostname: un hostname es adivinable y cambia.

El canal exige sesión de Supabase (login con Google, Fase 5) y una política que
solo deja entrar a los canales del propio usuario. El cifrado protege el
contenido; la política evita que un desconocido inunde el canal.

### Eventos

El webview ya escucha los eventos de Tauri para pintarse. El puente se suscribe a
los mismos y los reenvía cifrados, filtrados por la suscripción del dispositivo
(TRD §6):

| Evento | Alcance |
|---|---|
| `terminal-state`, `terminal-accounts-updated` | siempre |
| `session-status`, `session-ended`, `session-output` | solo suscritos |
| `pr-comments-found`, `queue-updated` | siempre |
| `oauth-callback` | **nunca** — es local del escritorio |

`session-output` no lleva bytes de PTY: lleva lo que Hangar ya extrae del
transcript (mensaje del agente, acción, turno terminado).

### Lo que hace falta añadir a Hangar

Dos cosas pequeñas que hoy no existen y que la app necesitará:

- **Nombre legible de máquina**, editable en Settings. Con varios Macs, el móvil
  vería dos UUID.
- **Modo global** (`yolo` y compañía) como ajuste de máquina. Hoy el modo vive
  por sesión.

### Reconexión

Retroceso exponencial hasta 60 s, en silencio: estar sin red es normal, no es un
error que merezca una alerta.

**Un comando en vuelo cuando se cae la conexión no se reintenta.** Pudo haber
llegado, y reintentar `open_pr_review_terminal` a ciegas corta ramas duplicadas.

## Criterios de aceptación

1. Desde otra máquina, el script de la Fase 0 lista terminales y cuota.
2. Manda un prompt y aparece en la terminal del Mac.
3. Un comando de nivel superior al del dispositivo da `E_PERMISSION`.
4. Cortar la red y volver: reconecta solo, sin avisos, sin duplicar nada.
5. Con Hangar cerrado, el cliente ve que no hay nadie — no se queda colgado.

## Riesgos

**El webview como transporte.** Si el proceso del webview muere, el puente muere.
Tauri lo mantiene vivo mientras la app corre, así que el fallo es "la app se
cerró", que ya es visible.

**Autorización en el sitio correcto.** Toda la validación pasa en Rust *antes* de
que el nombre del comando llegue al webview. Si se colara al revés, el webview
podría invocar cualquier cosa.

## Fuera de alcance

Emparejamiento (TDD 2), interfaz del escritorio (TDD 3), app (TDD 4+).
