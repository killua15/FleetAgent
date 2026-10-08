# opencode como proveedor

Verificado contra **opencode 1.18.20** instalada con `brew install opencode`, y contra su código
fuente. Nada de este documento sale de su documentación: los docs no dicen dónde viven las
credenciales, que es justo la única cosa de la que depende toda la integración.

## Lo que decide todo: el aislamiento va por `XDG_DATA_HOME`

El modelo de Hangar es un directorio de configuración por cuenta. Para opencode, **la variable no es
`OPENCODE_CONFIG_DIR`**, que es lo que cualquiera supondría por el nombre.

De `packages/core/src/global.ts`:

```js
const data   = path.join(xdgData!,   "opencode")   // ~/.local/share/opencode
const config = path.join(xdgConfig!, "opencode")   // ~/.config/opencode
...
config: Flag.OPENCODE_CONFIG_DIR ?? Path.config,   // redirigible
data:   Path.data,                                  // NO redirigible
```

Y de `packages/opencode/src/auth/index.ts`:

```js
const file = path.join(Global.Path.data, "auth.json")
```

O sea: `OPENCODE_CONFIG_DIR` mueve agentes, comandos, modos y plugins, y deja a **todas las cuentas
compartiendo un único fichero de credenciales**. En silencio, que es la peor versión de este fallo.

Comprobado en la máquina:

```
$ XDG_DATA_HOME=/tmp/oc-a opencode auth list
┌  Credentials /tmp/oc-a/opencode/auth.json
└  0 credentials
```

Dos consecuencias, ambas asumidas a propósito:

1. **`XDG_DATA_HOME` es genérica.** Cualquier herramienta consciente de XDG que se lance en esa misma
   terminal escribirá también dentro del directorio de la cuenta. Se acepta: esa terminal existe para
   correr un agente bajo una cuenta. Si algún día molesta, la salida es envolver el lanzamiento para
   que la variable aplique solo a ese proceso y no al shell.
2. **Es una raíz, y opencode le añade su nombre.** El `auth.json` cae en
   `<dir de cuenta>/opencode/auth.json`. Por eso `auth_marker_file("opencode")` devuelve esa ruta
   anidada y no un `auth.json` pelado, que nunca se encontraría.

### La trampa que parecía la solución limpia

`OPENCODE_AUTH_CONTENT` inyecta el JSON de credenciales por variable de entorno, sin tocar disco —
suena mejor que redirigir un directorio. **Es de solo lectura.** Las escrituras siguen yendo al
fichero compartido (`auth/index.ts:79,88`), así que un refresco de token OAuth contaminaría entre
cuentas: exactamente el fallo que Hangar existe para evitar. Descartado.

## No hace falta login: la clave va por el entorno

**Este fue el hallazgo que cambió el diseño**, y contradice a la documentación oficial, que dice que
para Anthropic y OpenAI las variables de entorno «no bastan» y hay que pasar por `/connect`.

Medido contra el binario, con un `XDG_DATA_HOME` limpio:

| | modelos totales | `anthropic/` |
|---|---|---|
| sin credencial | **7** (todos `opencode/`, el nivel gratuito) | 0 |
| con `ANTHROPIC_API_KEY` en el entorno | — | **15** |

Dos conclusiones:

1. **opencode lee la clave del entorno.** Así que la ruta de clave de Hangar —  que ya pasa el secreto
   como entorno ambiental, nunca en la línea de comandos— le llega tal cual, igual que a cualquier
   otra cuenta de clave.
2. **Funciona sin ninguna credencial**, con los modelos gratuitos de OpenCode Zen.

Por eso `login_args` es `None` y opencode entra en `KEY_ONLY_PROVIDERS`. No es una limitación: es que
`opencode auth login` es un **menú de flechas**, y el paso de login del asistente es un panel de solo
lectura con una caja para pegar códigos. Renderizaría un menú que nadie puede contestar — que es
exactamente el síntoma que se reportó, una terminal con el cursor parpadeando y nada más.

**No se pierde el OAuth**: quien lo quiera puede ejecutar `opencode auth login` en una terminal de
Hangar de verdad, que sí es interactiva, una vez creada la cuenta.

## Cómo responde a la sonda de login

`opencode auth list` imprime el fichero que leyó y luego un recuento. Sin credenciales:

```
└  0 credentials
```

Con una (creada a mano para la prueba):

```
◇  Anthropic api
└  1 credentials
```

Las filas de proveedor de arriba varían según el vendedor usado, así que **el recuento es la única
señal fiable**. De ahí los marcadores:

| | |
|---|---|
| `logged_out_marker` | `0 credentials` |
| `logged_marker` | `credentials` |

El negativo contiene al positivo, y eso funciona porque `read_text_probe` comprueba el negativo
primero — un orden que ya estaba puesto a propósito para el `"Not logged in"` de Codex.

Tener sonda es lo que hace que el asistente ofrezca el proveedor en vez de deshabilitarlo.

## Los cuatro modos de permiso

> **Reverificado contra opencode 2.0.20 (2026-10-08).** La 2.x quitó `--agent` de la TUI: solo queda
> en `opencode run` y `opencode mini`. Una terminal tecleada con `opencode --agent plan` muere con
> `Unrecognized flag: --agent` antes de arrancar. El modo `plan` va ahora por configuración.

El único flag de aprobación es `--auto` («auto-approve permissions that are not explicitly denied»). El
control fino no vive en flags: vive en **agentes**, que llevan reglas `{permission, action, pattern}`
con acciones `allow`, `ask` y `deny`.

| Modo de Hangar | opencode |
|---|---|
| `yolo` | `--auto` |
| `plan` | `OPENCODE_CONFIG=<app_data>/opencode/plan.json`, un fichero con `{ "default_agent": "plan" }` |
| `auto_edit` | sin flag — pregunta (menos permisivo de lo que promete el modo) |
| `ask_edits` | sin flag — pregunta ✔ correcto |

### Cómo se elige el agente `plan` en la 2.x

Probado con `opencode run "hi"` y mirando la cabecera (`> build · …` o `> plan · …`):

| Forma | Resultado |
|---|---|
| `OPENCODE_CONFIG=<fichero con default_agent: plan>` | **`plan`** ✔ — y la configuración global del usuario se sigue cargando (se fusiona, no se sustituye) |
| `OPENCODE_CONFIG_CONTENT='{"default_agent":"plan"}'` | `build` — no mueve el agente |
| `OPENCODE_CLI_CONFIG_CONTENT=…` | `build` |
| `opencode.json` en el proyecto con `default_agent` | `build` |
| `opencode run --agent plan` | `plan` (solo en `run`/`mini`) |

Por eso Hangar escribe ese fichero en sus datos de app y lo pasa por entorno
(`providers::permission_env`). No toca la configuración del usuario ni el repo del proyecto.

La comprobación se hizo con `run`; la TUI interactiva no se puede manejar desde un test, y comparte la
misma carga de configuración.

### El agente `plan` viene de fábrica, y aguanta

`opencode agent list` muestra siete agentes, y uno es `plan`. Sus reglas relevantes:

```
deny   edit   *
allow  edit   .opencode/plans/*.md
allow  read   *
ask    read   *.env
```

Probado ejecutándolo, no leído: se le pidió escribir un fichero con bash de dos formas —  directa, y
disfrazada de comprobación de permisos de disco— y las rechazó ambas, ofreciendo en la segunda una
alternativa no destructiva.

**Hasta dónde llega, dicho con precisión**: `edit` está denegado por regla; `bash` lo sostienen las
instrucciones del propio agente. Es la misma forma que el `--permission-mode plan` de Claude Code, así
que confiar en ello aquí es coherente y no un salto nuevo.

## Lo que no llega: las preferencias de proyecto

opencode **no tiene ningún flag de system prompt** — `--prompt` es el mensaje del usuario, no
instrucciones permanentes. Así que el idioma de conversación y de código que Hangar configura por
proyecto hoy no le llega.

Las dos vías:

- `opencode.json` en la raíz del proyecto. Funcionaría y **está prohibido**: la regla de
  [AGENT_CONTEXT.md](../../AGENT_CONTEXT.md) es que nada se escribe dentro del repositorio del usuario.
- `OPENCODE_CONFIG_CONTENT`, una variable que inyecta una configuración entera sin tocar disco. Es
  la vía abierta, y es la misma palanca que necesita el modo `plan`.

## Sesiones: las tiene, pero Hangar no puede acuñarlas

`--session <id>`, `-c/--continue`, `--fork`, y `opencode session list|delete`. Lo que no se puede es
elegir el id al arrancar:

```
$ opencode run --session id-que-no-existe-123 "hola"
Error: Session not found
```

Así que `SessionControl` queda en `None` — no porque no haya sesiones, sino porque Hangar no puede
dirigirlas todavía, y una terminal arranca de cero en vez de fingir continuidad.

**Cuando se cablee, la vía es mejor que la de Claude**: opencode tiene `export [sessionID]` e
`import <file>` en JSON, así que transferir un hilo entre cuentas es exportar e importar, no mover
ficheros a mano como hace `handoff.rs`.

## Fuera de alcance, y por qué

| | |
|---|---|
| **Rotación por agotamiento** | `is_exhaustion_signal` devuelve `false`. Capturar el mensaje exige agotar una cuota de verdad; no se puede adivinar |
| **Gasto por turno** | `usage_meter.rs` lee transcripts por desplazamiento de bytes. opencode guarda en SQLite (`OPENCODE_DB`) y agrega con `opencode stats`. Es otra vía de ingesta entera |
| **Skills y MCP** | No hay `--add-dir` ni `--mcp-config`; `mcp` es un subcomando que gestiona servidores. Ambos devuelven vacío |

## El panel estrecho lo mata

Su TUI **segfaultea dentro de Bun con 28 columnas o menos**, y funciona desde 30. Las filas no
influyen: 5×20, 10×20 y 20×20 mueren igual; 24×40 no.

```
panic(main thread): Segmentation fault at address 0x5084817FC
oh no: Bun has crashed. This indicates a bug in Bun, not your code.
```

Un panel de una rejilla de seis es exactamente así de estrecho, y también lo es cualquier panel antes
de que el `fit()` del frontend aterrice. Desde fuera se lee como «opencode no abre»: la terminal
aparece, el proceso muere, y vuelve el prompt del shell.

Por eso `MIN_PTY_COLS` en `terminals.rs` pone un suelo de 40 columnas a lo que se le dice al PTY —
para todos los proveedores, no solo para este: ningún CLI de agente es usable por debajo de eso, así
que lo único que cuesta es que un panel imposiblemente estrecho ajuste el texto más ancho de lo que
enseña, y lo que evita es entregarle a un CLI un tamaño que lo mata.

## Notas sueltas que ahorran tiempo

- opencode es **multi-proveedor por dentro**: una "cuenta de opencode" en Hangar es en realidad un
  directorio de datos con credenciales de N vendedores. Por eso `login_args` no fija `--provider`: qué
  vendedor se usa es elección del usuario dentro del menú.
- Por lo mismo, `key_env` no mapea limpio. Se nombra `ANTHROPIC_API_KEY` por ser el caso más común,
  no porque exista una "clave de opencode".
- El login es un **menú** (elegir proveedor, luego método), no una URL con código que pegar. De ahí
  `login_expects_code: false`: enseñar una caja de pegado pediría algo que aún no existe, que es el
  error que cometía el login de Codex.
- Se instala con `brew install opencode`, no con npm, a diferencia de los otros tres proveedores con
  binario propio.
