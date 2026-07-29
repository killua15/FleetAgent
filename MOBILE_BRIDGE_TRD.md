# TRD — Puente móvil de Hangar

Documento de contrato entre **Hangar (escritorio)** y **Hangar Mobile**. No es una
lista de funciones: es lo que las dos partes pueden dar por cierto una de la otra.

Existe porque son dos bases de código con **calendarios de publicación distintos**
— el escritorio se actualiza solo desde `updates/latest.json`, el móvil pasa por
la App Store — así que en cualquier momento hay usuarios con versiones que no
coinciden. Un contrato escrito y versionado es lo que hace que eso sea aburrido
en vez de un incidente.

**Estado**: borrador. Nada de esto está implementado todavía.
**Rama**: `feat/mobile-companion`.

---

## 1 · Alcance

### Qué es

Un puente que expone **la superficie que Hangar ya tiene** —135 comandos y 8
eventos— a un cliente remoto autenticado, cifrado extremo a extremo, sin
infraestructura propia.

### Qué NO es, y por qué

**No es una API nueva.** El puente no define endpoints. Enruta `invoke(nombre,
args)` a la misma función que llama el webview. Si define endpoints propios,
cada función nueva de Hangar hay que escribirla dos veces — que es exactamente
lo que este documento existe para evitar.

**No es un emulador de terminal remoto.** Se transmiten hechos (turno terminado,
fichero editado, comando ejecutado), no bytes de PTY. Un `xterm` de 120 columnas
no se lee en seis pulgadas, y el flujo crudo es dos órdenes de magnitud más
pesado que los eventos que lo describen.

**No es acceso a shell.** El móvil puede pedir lo que Hangar sabe hacer, nunca lo
que `bash` sabe hacer. Es la única decisión que importa el día que se pierda un
teléfono.

---

## 2 · Actores y confianza

| Actor | Confianza | Ve |
|---|---|---|
| **Hangar** (Mac) | raíz de confianza; posee la clave | todo |
| **Dispositivo emparejado** | la que le diera Hangar al emparejar | lo que su nivel permita |
| **Supabase** | **ninguna** | metadatos de enrutado y blobs que no puede abrir |

Supabase es transporte, no depositario. Esto no es purismo: el competidor de pago
del sector guarda las conversaciones en texto plano en sus servidores, y el
gratuito cifra. Un relay en claro sería estrictamente peor que la alternativa
gratuita.

### Niveles de permiso

Se eligen al emparejar y se revocan desde el Mac. Cada comando de la lista blanca
declara el suyo:

```
mirar     list_* get_* read_*        estado, cuota, diffs, PRs, ficheros
dirigir   + send_* respond_* stop_*  prompts, permisos, parar, cambiar de cuenta
lanzar    + open_* create_* delete_* worktrees, tareas, borrar sesiones
```

La lista blanca es **datos, no código**: un fichero de `nombre → nivel`. Añadir
una función de Hangar al móvil es añadir una línea.

---

## 3 · Transporte

Canal de Supabase Realtime por máquina. Hangar mantiene una **conexión saliente**
— así no hay NAT que abrir ni puerto que exponer — y el móvil publica en el mismo
canal.

```
canal: hangar:<machine_id>
```

`machine_id` es un UUID generado al primer arranque y guardado en `app_settings`.
No es el hostname: un hostname es adivinable y cambia.

**Límite conocido**: los mensajes de broadcast rondan los 256 KB. Es de sobra
para eventos y respuestas; los payloads que puedan crecer (scrollback, diffs)
están acotados explícitamente en §6.

**Sin servidor propio, y lo que eso cuesta**: si el Mac está dormido no hay
puente ni notificación. Es aceptable porque los agentes solo corren con el Mac
despierto, pero es una limitación, no un detalle.

---

## 4 · El sobre

Todo mensaje, en las dos direcciones:

```jsonc
{
  "v": 1,                        // versión del protocolo
  "id": "01J...",                // ULID; correlaciona petición y respuesta
  "kind": "invoke" | "result" | "error" | "event" | "hello",
  "name": "list_account_spend",  // comando o evento
  "nonce": "base64",             // libsodium
  "payload": "base64"            // cifrado; el claro es JSON
}
```

Solo `v`, `id`, `kind` y `nonce` viajan en claro. `name` va **dentro** del
payload cifrado: saber que alguien llamó a `list_account_spend` cada cinco
minutos ya es información.

> Corrección respecto al borrador anterior: `name` estaba fuera. Va dentro.

### Versionado

- Cambios **solo aditivos** dentro de una versión mayor: campos nuevos opcionales,
  comandos nuevos. Nunca renombrar ni cambiar el tipo de un campo existente.
- El móvil manda `hello` con su `v`; Hangar responde con la suya y con la lista
  de comandos permitidos **para ese dispositivo**.
- Versiones incompatibles → error explícito (`E_PROTOCOL_VERSION`) con las dos
  versiones dentro. Un cliente viejo debe poder decir *"actualiza la app"*, no
  fallar de forma rara.

---

## 5 · Comandos

### Contrato

El nombre y la forma del payload son **los que ya usa el webview**. Ya están en
camelCase por serde y ya están tipados.

Con el móvil en **Flutter**, los tipos no se pueden compartir: Rust y Dart no
tienen un compilador en común, y el escritorio ya consume estos mismos tipos
desde TypeScript. Tres idiomas, una sola forma.

**Decisión: ficheros dorados, no generación de código.**

```
contract/fixtures/list_account_spend.result.json
contract/fixtures/terminal-state.event.json
…
```

Hangar los escribe desde sus propios tests (los datos reales que serializa, no
ejemplos escritos a mano), y los tres lados tienen un test que los parsea. Si
alguien cambia un campo, el fichero dorado cambia en el diff y el test del otro
lado falla.

Se descartó una tubería de generación (JSON Schema → Dart + TS): para ~30
comandos expuestos es más maquinaria que contrato, y una tubería rota no avisa
de nada. Un fichero dorado que no parsea sí.

```
invoke  → { name, args }
result  → { ok: true, data }
error   → { ok: false, code, message }
```

### Códigos de error

```
E_PROTOCOL_VERSION   versiones incompatibles
E_NOT_ALLOWED        el comando no está en la lista blanca
E_PERMISSION         el dispositivo no llega a ese nivel
E_UNKNOWN_COMMAND    no existe (o el escritorio es más viejo que la app)
E_DECRYPT            no se pudo descifrar: clave equivocada o mensaje manipulado
E_COMMAND_FAILED     el comando corrió y devolvió Err — `message` es el suyo
E_TOO_LARGE          la respuesta excede el límite del canal
```

`E_UNKNOWN_COMMAND` merece su propio código y no colapsarse en `E_NOT_ALLOWED`:
distingue *"tu app pide algo que este escritorio aún no tiene"* de *"no tienes
permiso"*, y son dos mensajes distintos para el usuario.

### Comandos que no se exponen

Los que reciben un `Channel` (streaming de PTY, `terminals.rs`) **no** entran en
la lista blanca. Su equivalente remoto está en §6.

---

## 6 · Eventos y selección de terminales

### El problema

Hangar puede tener terminales de varios proyectos y varias cuentas. Mandar los
eventos de todas a un móvil es ruido, batería y datos; y notificar de todas hace
que el teléfono vibre seis veces por tarea.

### El contrato

**El estado se manda entero; los eventos se suscriben.** El estado de la flota
son unos kilobytes y hace falta para poder elegir. El volumen está en los
eventos.

```jsonc
// móvil → Hangar
{ "kind": "invoke", "name": "bridge_subscribe",
  "args": { "terminals": ["term-a", "term-b"], "notify": ["term-a"] } }
```

- `terminals` — de cuáles quiero eventos en vivo. Vacío = ninguno (la lista
  sigue actualizándose por `terminal-state`, que es barato).
- `notify` — de cuáles quiero **notificación push**. Subconjunto, y separado a
  propósito: mirar una terminal y querer que te despierte por ella son dos
  decisiones distintas.

La suscripción **vive en el dispositivo, no en la sesión**: sobrevive a
reconexiones y a cerrar la app. Se cambia llamando otra vez.

### Eventos reenviados

| Evento | Alcance | Cuándo |
|---|---|---|
| `terminal-state` | flota | siempre — es cómo se elige |
| `terminal-accounts-updated` | flota | siempre |
| `session-status`, `session-ended` | suscritos | |
| `session-output` | **suscritos, y resumido** | ver abajo |
| `pr-comments-found`, `queue-updated` | flota | |
| `oauth-callback` | **nunca** | es local del escritorio |

### `session-output`: lo que se manda en vez de bytes

En vez del flujo del PTY, el puente manda lo que Hangar ya extrae del transcript:

```jsonc
{ "terminalId": "…", "kind": "agent_message" | "action" | "turn_done",
  "text": "…", "files": ["src/x.rs"], "at": "…" }
```

Y para cuando de verdad haga falta lo crudo, un comando acotado y a petición:

```jsonc
{ "name": "bridge_scrollback", "args": { "terminalId": "…", "lines": 200 } }
```

Máximo 500 líneas, recortado por el escritorio. Nunca un flujo.

---

## 7 · Emparejamiento y ciclo de vida del dispositivo

```
1. El Mac muestra un QR:  { machine_id, clave, caducidad 120s, nivel }
2. El móvil lo escanea, genera su par de claves y publica `hello`
3. El Mac lo enseña: "iPhone de David quiere emparejarse · mirar"  → Aceptar
4. Queda registrado: id, nombre, nivel, alta, último visto, token push
```

- La clave viaja **en el QR**, un canal fuera de banda que se ve con los ojos y
  no pasa por la red.
- El QR **caduca a los 120 segundos**. Una captura de pantalla en un chat no debe
  seguir sirviendo mañana.
- La aceptación es explícita en el Mac aunque el QR sea correcto: escanear no es
  autorizar.
- **Revocar** borra la clave del dispositivo. Un móvil perdido se corta desde el
  Mac, y el corte es inmediato porque sin clave no puede descifrar nada.

---

## 8 · Notificaciones

**Con Flutter el push cambia**, y es la parte donde "sin infraestructura propia"
necesita un matiz.

Expo tenía una API push que acepta un token y no exige secreto. Flutter usa FCM
(que cubre Android y, vía APNs, iOS), y llamar a FCM **exige una credencial de
servidor**. Meterla en la app de escritorio sería repartir esa credencial con
cada instalación.

**Decisión: una Edge Function de Supabase manda el push.** La credencial de FCM
vive ahí, del lado del servidor, y nunca se distribuye. Sigue sin haber
infraestructura que administrar —las Edge Functions son parte del Supabase que ya
pagas— pero es honesto decir que **hay una función de servidor**, no cero código
remoto.

El Mac le pide a esa función que notifique; el contenido de la notificación viaja
en claro hasta ella (ver abajo), así que solo lleva título y `terminalId`.

Reutiliza `notifications::notify_if_enabled`, que ya existe y ya sabe qué eventos
quiere el usuario: los mismos cinco interruptores de Settings deciden qué llega
al móvil, filtrados además por la lista `notify` de cada dispositivo.

```
título:  el nombre del panel o el título que puso el agente
cuerpo:  qué pasó
datos:   { terminalId } — para abrir la app justo ahí
```

**Nada del contenido de la conversación va en la notificación.** Pasa por
servidores de Apple y Google en claro; el resto del sistema está cifrado
precisamente para que no.

---

## 9 · Reconexión y estado

- El móvil que reconecta manda `hello` y recibe el estado completo de la flota.
  **No hay reproducción de eventos perdidos**: el estado actual es la verdad, y
  una cola de eventos por dispositivo es un depósito que nadie quiere mantener.
- Hangar reintenta la conexión con retroceso exponencial hasta 60 s, y no molesta
  al usuario: estar sin red es normal.
- Un comando en vuelo cuando se cae la conexión **no se reintenta**. Pudo haber
  llegado. Reintentar `open_task_worktrees` a ciegas corta ramas duplicadas.

---

## 10 · Lo que este documento congela

Al terminar las fases 0–2, esto no debería cambiar:

1. El sobre y su versionado (§4)
2. Los nombres de comando y sus payloads = los del webview (§5)
3. Los códigos de error (§5)
4. El modelo de suscripción y notificación por dispositivo (§6)
5. El emparejamiento y la revocación (§7)

Todo lo demás —qué pantallas tiene el móvil, cómo se ven— puede cambiar sin tocar
Hangar. Ese es el objetivo del documento.

---

## 11 · Stack del cliente

| Pieza | Elección | Por qué |
|---|---|---|
| Framework | **Flutter** | decidido |
| Realtime | `supabase_flutter` | SDK oficial con Realtime de primera clase |
| Cifrado | `sodium_libs` | enlaza libsodium de verdad; nada casero |
| QR | `mobile_scanner` | mantenido, usa la cámara nativa |
| Voz | `speech_to_text` | dictado en el dispositivo, sin API ni coste |
| Push | FCM + APNs vía Edge Function | ver §8 |

---

## 12 · Preguntas abiertas

1. **Múltiples Macs.** ¿Un móvil puede estar emparejado con dos ordenadores? El
   canal por máquina lo permite; la interfaz tendría que elegir. Propuesta: sí en
   el contrato, y en la app hasta la fase 5.
2. **Rotación de clave.** No hay mecanismo. Propuesta: revocar y volver a
   emparejar; construir rotación cuando exista una razón, no antes.
3. **Dos móviles dirigiendo la misma terminal.** El contrato no lo impide. ¿Hace
   falta bloqueo, o gana el último? Propuesta: gana el último y se muestra quién,
   que es lo que hace un terminal compartido.
4. **`bridge_scrollback` y secretos.** El scrollback puede contener lo que el
   agente imprimió, incluidas claves. Va cifrado, pero queda en la caché del
   móvil. ¿Se guarda o solo se muestra? Propuesta: solo en memoria.
