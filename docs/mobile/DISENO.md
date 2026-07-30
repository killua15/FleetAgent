# Hangar Mobile — el diseño, y dónde vive

Para retomar esto sin volver a analizarlo desde cero.

## Dónde está

Proyecto de Claude Design **«Fleet: orquestador de agentes IA»**
`a717b7cd-6ca3-482a-a2be-95d3d5f9a45c`

```
HangarMobile.dc.html    índice: 21 tarjetas agrupadas, NO dibuja nada
PhoneFrame.dc.html      el diseño de verdad: 18 pantallas en un componente
support.js              runtime generado (plantillas x-dc sobre React)
```

El resto del proyecto es el escritorio (`Fleet.dc.html`, `AgentPanel.dc.html`,
`GitPanel.dc.html`…), de donde sale el lenguaje visual.

**El índice no es el diseño.** Define las pantallas y delega con
`<dc-import name="PhoneFrame" screen="...">`. Buscar el diseño en
`HangarMobile.dc.html` y no encontrarlo es el primer tropiezo.

### Cómo funciona `PhoneFrame`

Un solo componente con dos props:

- `screen` — enum de 18 valores
- `theme` — `dark` (por defecto) o `light`

Dentro: una `meta()` con el título, el estado del puente y el nivel de cada
pantalla, y un `<sc-if>` por pantalla. Añadir una es cuatro sitios: el enum de
`data-props`, una fila en `meta()`, un `isLoQueSea: is('loquesea')`, y el bloque.

## Las 18 pantallas

| clave | pantalla | estado del puente | nivel |
|---|---|---|---|
| `pair` | Emparejar (QR, 120 s) | ✅ verificado en vivo | watch |
| `fleet` | La flota | ✅ | watch |
| `open` | Abrir terminal | ➕ `open_terminal`, `close_terminal` | launch |
| `notif` | Pantalla bloqueada | 🔧 Edge Function + FCM | watch |
| `terminal` | La conversación | ✅ | steer |
| `permission` | Permiso pendiente | ✅ | steer |
| `mode` | plan / ask_edits / auto_edit / yolo | ➕ `set_default_terminal_mode` | steer |
| `voice` | Dictar y confirmar | 🔧 todo en el teléfono | steer |
| `queue` | Cola de trabajo | ➕ `list_queued_runs`… | steer |
| `fanout` | Un prompt, varias terminales | ➕ `fan_out_prompt` (ya existe) | launch |
| `tasks` | Issue → worktree | ◑ falta `list_source_scopes` | launch |
| `git` | Diffs, ramas, commits | ◑ falta `get_commit_files` | watch |
| `prs` | Bandeja de revisión | ◑ falta `dismiss_pr_comments` | steer |
| `accounts` | Cuotas y gasto | ◑ falta `reopen_terminal_with_account` | watch |
| `trees` | Árbol en marcha | ➕ `run_agent_tree` | launch |
| `offline` | Mac dormido | ✅ | watch |
| `revoked` | `E_DEVICE_REPLACED` | ✅ | watch |
| `toolarge` | `E_TOO_LARGE` | ✅ | watch |

El detalle comando a comando está en [FUNCIONALIDADES.md](FUNCIONALIDADES.md).

## Lo que el diseño acertó

**No es un emulador de terminal.** Es la restricción más difícil del proyecto y la
que cualquiera se salta: «controlar terminales desde el móvil» suena a SSH. La
pantalla `terminal` dibuja bloques tipados del transcript —`you` violeta, `think`
tenue, `edit` verde, `cmd` apagado, `agent` teal— con monoespaciada solo para el
contenido. Sin PTY, sin cursor, sin scrollback.

Y añadió algo que no estaba en el brief y merece quedarse: un marcador
**«↑ desde el móvil»** en los bloques que salieron del teléfono, para que al
volver al Mac se distinga quién escribió qué.

## Lo que se corrigió el 2026-07-30

### El contraste había vuelto atrás

Usaba la paleta anterior a los arreglos de contraste del escritorio:

| token | tenía | ahora | contraste sobre `--bg` |
|---|---|---|---|
| `--mut` | `#9a9ab0` | `#b4b4c8` | 6,9:1 → 9,4:1 |
| `--dim` | `#61627a` | `#8a8ba3` | **3,2:1** → 5,7:1 |

`--dim` es lo que lleva horas, etiquetas y metadatos. A 3,2:1 sobre el fondo
(2,8:1 sobre panel) no llegaba al mínimo AA de 4,5:1 — y en un teléfono al sol es
justo donde peor se nota. Los valores buenos están en `src/styles/tokens.css`; **es
la fuente, no el diseño.**

### Los badges mentían, y por una razón estructural

Estaban **por pantalla** cuando la realidad es **por comando**. Una pantalla mezcla
comandos con estado distinto y un solo badge lo aplasta. Tres eran falsos:

- `pair` decía «falta exponer». Está terminado y verificado en vivo. Sus comandos
  son solo-Mac a propósito, que es otra cosa.
- `fanout` decía «falta construir». `fan_out_prompt` y `project_fan_out` ya existen
  en Hangar: es una línea en `allowlist.json`, no semanas.
- `tasks`, `prs`, `git` y `accounts` iban en bloque siendo mixtas.

Ahora hay un cuarto estado **◑ parcial** (teal) y una línea bajo el título que dice
qué falta exactamente, con nombre de comando.

### Faltaban tres estados difíciles y el tema claro

- **`offline`** — Mac dormido. Lo último que se supo en gris y con su hora, nunca
  en blanco: una lista vacía se lee como «no hay nada corriendo», que es lo
  contrario de lo que pasa.
- **`revoked`** — `E_DEVICE_REPLACED` con su nombre, porque si no fuiste tú, alguien
  con acceso al Mac emparejó un teléfono. Un «se perdió la conexión» genérico
  esconde eso.
- **`toolarge`** — el tope de 200 KB del canal, presentado como límite y no como
  fallo: cuánto sobra y qué hacer.
- **Tema claro** — como todo estaba en variables CSS, salió con una paleta más y un
  prop. Los valores son los de `tokens.css`.

## Ver una pantalla sola

Dentro de la herramienta hay dos maneras:

```
…?file=HangarMobile.dc.html#m1f     una tarjeta concreta de la galería
…?file=PhoneFrame.dc.html           el componente solo, con el enum de 18 en el editor de props
```

Anclas: `m1a` pair · `m1b` fleet · `m1c` open · `m1d` notif · `m1e` terminal ·
`m1f` permission · `m1g` mode · `m1h` voice · `m1i` queue · `m1j` fanout ·
`m1k` tasks · `m1l` git · `m1m` prs · `m1n` accounts · `m1o` trees ·
`m1p` offline · `m1q` revoked · `m1r` toolarge · `m1s`–`m1u` tema claro.

Pero eso exige tener el proyecto de diseño delante. Para construir en Flutter hace
falta que cada pantalla se entienda sin abrirlo — es lo que va abajo.

## Qué lleva cada pantalla

Contenido literal de la maqueta. Los datos de ejemplo son parte de la
especificación: dicen qué campos existen y qué longitud tiene que aguantar el
diseño.

### `pair` — Emparejar · watch
QR de 9×9 sobre blanco, «caduca en 01:47» en ámbar, y la frase que carga el
concepto: «Escanear no autoriza nada todavía: el Mac te pedirá aprobar este
teléfono». Debajo, la lista **ESTADOS QUE HAY QUE MOSTRAR** con cinco entradas:
código caducado · `E_PENDING_APPROVAL` · rechazado (se borra el intento, no queda
a medias) · `E_PROTOCOL_VERSION` (decir cuál actualizar).

### `fleet` — Tu flota · watch
Macs → proyectos → terminales, tres niveles. Cada Mac con estado y color: «MacBook
Pro · estudio / conectado» en teal, «Mac mini · casa / dormido · 12 min» en gris.
Cada terminal con su estado: `permiso` (ámbar), `trabajando` (verde), `terminó`.
Al pie: «☾ Mac dormido no es un error: seguís viendo el último estado conocido y la
hora del corte».

### `open` — Abrir terminal · launch ➕
Tres selectores en fila —PROYECTO, CUENTA, MODO— cada uno con sus opciones y una
marcada. Campo **PRIMER PROMPT**. Y la nota que evita el malentendido: «El móvil
manda cols/rows nominales (80×24). Nunca se muestran: no hay terminal que
dimensionar».

### `notif` — Pantalla bloqueada · watch 🔧
Reloj 9:41, tres notificaciones apiladas. La urgente lleva botones **Aprobar** /
**Rechazar** en la propia notificación: permiso pendiente con el comando visible
(`rm -rf ./dist && pnpm build`). Las otras dos son turno terminado (24 pasaron, 1
falló) y cuota agotada.

### `terminal` — La conversación · steer
Bloques tipados con color por clase y hora: `VOS` violeta · `PENSANDO` tenue ·
`EDICIÓN` verde con el diff en una línea · `COMANDO` apagado con `$` · `AGENTE`
teal, que termina preguntando. El bloque de `VOS` lleva **«↑ desde el móvil»**.
Abajo, la caja de escribir con micrófono y enviar.

### `permission` — Permiso pendiente · steer
`toolName` **Bash** y `toolInput` como JSON formateado, monoespaciado y completo.
**Aprobar** / **Rechazar**. Y la regla escrita: «Dos segundos y una decisión. Si no
hay respuesta, el turno espera — no se aprueba solo».

### `mode` — Modo de permisos · steer ➕
Cuatro filas, cada una con su descripción: `plan` (solo propone) · `ask_edits`
(seleccionado, marcado como default del Mac) · `auto_edit` (edita solo, pregunta
para comandos) · `yolo` en rojo (hace todo sin preguntar; «reservalo para worktrees
descartables»). Aviso: **yolo desde el teléfono pide escribir el nombre del
proyecto a mano.**

### `voice` — Dictar · steer 🔧
Micrófono grande, y la transcripción bajo el rótulo **REVISÁ ANTES DE MANDAR**, con
**Editar** y **Mandar**. «Nunca se manda lo dictado sin confirmar: una palabra mal
transcrita cambia lo que hace el agente».

### `queue` — Cola de trabajo · steer ➕
Cuatro filas numeradas: `▶ corriendo`, dos `en cola` con ✕ para cancelar, y una
`cancelado` tachada. Campo para encolar otro. Al pie: «El puente ya emite
`queue-updated`; hoy nadie lo mira. Esta pantalla es ese evento hecho visible».

### `fanout` — Un prompt, varias terminales · launch ➕
Un prompt arriba, y **MANDAR A — 3 DE 5 SELECCIONADAS** con cinco terminales de
proyectos distintos, cada una con su estado. La del Mac dormido aparece
deseleccionada y no se puede marcar. Botón «Mandar a 3 terminales».

### `tasks` — Issue → worktree · launch ◑
Selector de **ÁMBITO** rotulado con el comando que falta (`list_source_scopes`), y
la lista de tareas: referencia, etiqueta (`listo para agente` / `falta contexto`),
título y la rama que se generaría (`fix/cas-214`). Acción **→ worktree**.

### `git` — Diffs y commits · watch ◑
Cabecera con rama y `+55 −6`. Ficheros con badge M/A y sus números. **DIFF —
money.py** con líneas coloreadas por signo. Botones **Commit** y **Push**, el
segundo con ⚠ porque es lo menos reversible de la app.

### `prs` — Pull requests · steer ◑
PRs con referencia, estado (`cambios pedidos` ámbar, `aprobado` verde), diff y
título. Los comentarios desplegados con iniciales del autor. Acciones **mandar al
agente** y **descartar**, más **Crear PR desde fix/billing-round**.

### `accounts` — Cuotas y gasto · watch ◑
Cuentas con inicial, plan y dos barras (ventana de 5 h y semanal), en ámbar al
pasar del 75%. Acme Corp al 100% dispara el aviso: «se quedó sin cuota → Reabrir la
terminal con Personal **manteniendo la conversación entera**».

### `trees` — Árbol en marcha · launch ➕
Nodos indentados por profundidad con rol, actividad y estado: Product Lead
(espera) → Tech Lead (revisa) → Frontend / Backend / QA. «Diseñar el árbol se queda
en el escritorio. Desde el móvil: mirar cómo va y lanzarlo otra vez».

### `offline` — Mac dormido · watch
Punto gris, nombre del Mac y «hace 14 min». Las terminales conocidas en línea
discontinua al 55% de opacidad, con la hora de su último dato. La regla:
**nunca en blanco** — una lista vacía se lee como «no hay nada corriendo».

### `revoked` — `E_DEVICE_REPLACED` · watch
Tarjeta roja con el código, «Otro teléfono se emparejó con este Mac», y la
consecuencia: la clave guardada aquí se borra. **Volver a emparejar** / **Olvidar
este Mac**. Se nombra el motivo porque si no fuiste tú, es una señal de seguridad.

### `toolarge` — `E_TOO_LARGE` · watch
El fichero, cuánto ocupa (1,8 MB) y cuánto cabe (200 KB), con una barra al 11%.
Salidas: **ver solo el resumen** o **abrirlo en el Mac**. Nunca un reintento que va
a fallar igual.

## Si se retoma esto

1. **El orden que menos retrabajo genera** es exponer primero los doce comandos del
   resumen de `FUNCIONALIDADES.md` y después construir contra un contrato cerrado.
   El diseño ya marca cuáles son.
2. `PhoneFrame` es un componente de maqueta, no de producción: estilos en línea y
   datos falsos dentro de `renderVals()`. Para Flutter sirve como especificación
   visual, no como código a traducir.
3. Los colores por tipo de bloque (`you`/`think`/`edit`/`cmd`/`agent`) son la parte
   que conviene copiar literal: son los mismos que el escritorio usa en
   `AgentPanel`, y que coincidan es lo que hace que las dos apps se sientan una.
