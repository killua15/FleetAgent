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
