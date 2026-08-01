# TDD 4-8 · El cliente Flutter

**Lado**: repositorio aparte. **Depende de**: TDD 0-3 terminados.

---

> **Aviso sobre la profundidad de este documento.**
>
> Las fases 0 a 3 están detalladas porque su contrato está congelado y son lo
> siguiente que se construye. Estas no. Escribir hoy el detalle de una pantalla
> que aún no existe, contra un puente que aún no se ha usado nunca, sería
> ficción con formato de especificación — y la ficción en un documento técnico
> es peor que un hueco, porque se implementa.
>
> Cada fase aquí fija **su objetivo, qué consume del contrato y cómo se sabe que
> está hecha**. El diseño de pantallas se escribe cuando el puente funcione y
> después de media hora usando [Happy](https://github.com/slopus/happy), que es
> el competidor directo y va a enseñar más que cualquier boceto.

---

## Fase 4 · La base · ~1 semana

**Objetivo**: una app que se empareja y no hace nada más.

Proyecto Flutter en repositorio propio. `sodium_libs` para el cifrado (los mismos
vectores de prueba que Rust, para verificar que las dos mitades se entienden),
`supabase_flutter` para el canal, `mobile_scanner` para el QR.

**Consume**: TRD §4 (sobre), §7 (emparejamiento).
**Hecho cuando**: escanea el QR de un Mac, se acepta en el Mac, y ejecuta un
`hello` que devuelve la versión del protocolo y la lista de comandos permitidos.
Sin pantallas de producto.

**Ficheros dorados**: se copian en CI desde el repositorio de Hangar, que es la
verdad. Un test los parsea. Si Hangar cambia una forma, este test falla al
siguiente CI y no en producción.

## Fase 5 · El espejo · ~1,5 semanas

**Objetivo**: ver la flota.

Lista de máquinas emparejadas → terminales de la elegida, con su estado y su
cuenta → detalle de una terminal: **mensajes del agente, acciones, estado**.
Nunca el volcado del terminal, para una terminal **con sesión** (TRD §12.4 —
hay una excepción para las que no tienen sesión, ver Fase 6bis).

**Consume**: `list_*`, `get_*` (nivel *mirar*), eventos `terminal-state`.
**Hecho cuando**: abres la app fuera de casa y sabes qué está corriendo y cuánta
cuota queda, sin tocar nada.

Esta fase ya es útil por sí sola, y es la que conviene usar una semana antes de
construir la siguiente.

## Fase 6 · Dirigir · ~1 semana

**Objetivo**: el caso que justifica la app.

Seleccionar una terminal (se marca en el Mac), mandarle un prompt, pararla.
Y las acciones que **abren** una terminal en el Mac: mandar un PR a revisar,
mandar una tarea. No hay "abrir terminal vacía" (TRD §12.9).

**Consume**: nivel *dirigir* y *lanzar*, `bridge_select`.
**Hecho cuando**: desde el móvil, mandas a revisar un PR y la terminal se abre en
el Mac ya trabajando.

## Fase 6bis · PTY directo · ~1 semana

**Objetivo**: dirigir una terminal sin sesión — el flujo que de verdad se usa
día a día ("+ New terminal"), que Fase 5/6 no cubren porque no genera bloques
tipados.

Composer nuevo (no el de Fase 6: ese está armado sobre bloques tipados) que
escribe con `write_terminal`, y una vista que muestra el tail acotado y
redactado de `terminal-tail` — el transcript completo, reemplazado entero en
cada emisión, no un log que se concatena. Reconstruido con un emulador de
terminal real (`vt100`) del lado del Mac, para que un párrafo que Claude Code
redibuja al reajustar el ancho de línea se vea una sola vez, no repetido ni
mezclado. Sin fidelidad visual de cursor/alt-screen en el móvil — texto
plano, a propósito.

**Decisión revertida para llegar acá**: TRD §12.4 decía "nada de scrollback al
móvil, nunca". Se revirtió por escrito, con el trade-off explícito: redacción
por patrones de secreto conocidos, best-effort, no garantía. Ver TRD §6 y
§12.4, y §1 ("no es acceso a shell libre — con una excepción escrita").

**Consume**: `write_terminal` (nivel *dirigir*, ya expuesto), evento nuevo
`terminal-tail` (nivel *mirar*).
**Hecho cuando**: desde el móvil escribís en una terminal PTY real del Mac, y
ves lo que pasó — acotado, redactado, sin ser un espejo de la pantalla.

## Fase 7 · El resto de la flota · ~1 semana

**Objetivo**: lo que ningún competidor puede enseñar.

Tablero de tareas, PRs y comentarios, cambios por worktree, uso y gasto por
cuenta y proyecto.

**Aquí es donde la app deja de ser "una terminal en el bolsillo" y pasa a ser "la
flota en el bolsillo"** — que es la única razón por la que tiene sentido
construirla habiendo alternativas gratuitas.

### Fase 7a · Gasto y uso por cuenta/proyecto — hecho

Pantalla `spend` por Mac: gasto de cada cuenta (sesión/semana, o coste directo
para una cuenta que factura por clave), gasto por proyecto en los últimos 7
días, y presupuestos semanales con su `estimated` cuando el reparto entre
proyectos es una aproximación. Solo lectura — cambiar un presupuesto o tope
sigue ➕ (FUNCIONALIDADES.md §12).

**Consume**: `list_account_spend`, `list_project_spend`, `list_project_budgets`
(nivel *mirar*, ya expuestos desde antes de Fase 7). Sin evento en vivo — se
carga al entrar y se refresca a mano (`pull-to-refresh`), igual que el
escritorio no tiene un evento dedicado para esto tampoco.
**Hecho cuando**: desde la tarjeta de una Mac en la flota, un ícono lleva a su
gasto por cuenta y proyecto, y a sus presupuestos.

### Fase 7b · PRs y comentarios — hecho

Bandeja de revisión por Mac, agrupada por proyecto: cada PR abierto con sus
comentarios pendientes, con pestañas cliente-side (mío / para revisar /
todos) sobre `relation`. Acciones: mandar un PR entero a revisar, mandar un
comentario puntual (uno a la vez, igual que el escritorio: nunca selección
múltiple), descartar un comentario sin mandarlo a ningún agente. Elegir la
cuenta con la que se abre la terminal es un selector manual (`list_accounts`),
no automático — a propósito, para no adivinar con qué cuenta se gasta.

**Consume**: `list_pr_review_inbox`, `get_pull_request` (*mirar*),
`check_pr_comments`, `check_all_open_prs_now`, `dismiss_pr_comments`
(*dirigir*), `open_pr_review_terminal`, `send_pr_comments_to_terminal`
(*lanzar*) — todos ya expuestos, sin tocar Rust salvo agregar
`check_all_open_prs_now` a `allowlist.json`. La terminal que abren estos dos
últimos es siempre de sesión (`kind=Session`), así que la app arma el
`TerminalSummary` directo de lo que devuelven, sin una vuelta a
`list_terminals`.
**Hecho cuando**: desde el móvil se ve la bandeja de revisión real de un Mac,
se manda un comentario o un PR a una terminal, y la app navega a esa
conversación ya con un agente trabajando en el Mac.

Fuera de esta fase: crear un PR desde el móvil (`preview_pull_request`/
`create_pull_request`) — pertenece al ciclo de "terminar el trabajo" de una
sesión propia (Fase 6), no a la bandeja de revisión.

### Fase 7c · Cambios por worktree/diffs — hecho

Por Mac: estado de git de cada proyecto (su checkout propio + la lista de
sus worktrees — un worktree no es una entidad propia, es una fila de
`sessions` con `worktreePath`), archivos cambiados de cualquiera de esos
paths (sin commitear / commiteado sobre `base`), y el diff línea por línea
de cada archivo. Solo lectura — sin `fetch`/`pull`/`push`/`checkout`/crear
worktree en esta v1 (el propio `FUNCIONALIDADES.md` marca `push` como "la
acción menos reversible de toda la lista"; no era lo que se pidió acá).

**El wire nunca manda un diff calculado** — `get_worktree_file_diff` devuelve
el archivo entero antes/después (`CommitFileDiff{oldText,newText}`), igual
que el escritorio. El diff línea por línea se calcula del lado del cliente
(`presentation/widgets/diff_view.dart`, un LCS con el mismo tipo de tope de
tamaño que usa `diffLines.ts` para no colgar la UI con un archivo enorme),
sin el paso de colapsar contexto lejano del escritorio — se ve toda la
pantalla, sin "expandir sección oculta".

**Consume**: `get_git_overview`, `get_worktree_changes`,
`get_worktree_file_diff` (*mirar*, ya expuestos desde antes de Fase 7) — sin
tocar Rust ni `allowlist.json` en esta sub-fase.
**Hecho cuando**: desde el móvil se abre el estado de git de un proyecto
real, se entra a un worktree con cambios sin commitear, y el diff de un
archivo modificado se ve línea por línea, coloreado.

Fuera de esta fase: historial de commits y ramas (`list_branches`,
`list_commits`, `get_commit_files`, `get_commit_file_diff`) — es la pantalla
separada `GitHistoryView.tsx` del escritorio, "qué pasó" en vez de "qué
cambió ahora"; queda para una sub-fase futura si hace falta.

### Fase 7d · Tablero de tareas — hecho

`docs/mobile/FUNCIONALIDADES.md` §8: de un issue a un worktree trabajando.
Se entra por proyecto (un ícono en la fila propia de un proyecto en
`git-overview`, nunca en una fila de worktree — una tarea abre uno nuevo, no
vive en uno existente), no por una pantalla separada de fuentes/tableros:
`list_project_tasks` ya resuelve del lado del Mac qué fuentes mirar y a qué
scope, vía los links existentes o, sin ninguno, todas las configuradas a su
scope por defecto. Filtro por tablero de origen (`Task.container`) del lado
del cliente, sin pedir `list_source_scopes` — no hace falta si ya no hay que
elegir de dónde salen las tareas.

Igual que en la bandeja de PRs: de una tarea por vez, nunca selección
múltiple; elegir cuenta es manual (mismo selector reusado de Fase 7b), sin
editor de rama (la sugerida por el Mac se usa tal cual). Abrir siempre
resuelve una terminal de sesión con el agente ya arrancando.

**Consume**: `list_project_tasks` (*mirar*), `open_task_worktrees`
(*lanzar*), `list_accounts` (ya expuesto desde Fase 7b) — todos ya
allowlisteados desde antes de esta fase, sin tocar Rust.
**Hecho cuando**: desde el móvil se ve el tablero de tareas de un proyecto
real, se elige una tarea y una cuenta, y la app navega a la terminal de
sesión que el Mac abrió con el worktree y el brief ya cargados.

Fuera de esta fase: navegar por fuente/tablero sin pasar por un proyecto
(`list_task_sources`/`list_source_scopes` desde una pantalla propia) —
ninguna fuente Mac-wide lo pide todavía, y agregarla sin ese caso de uso
sería especular.

## Fase 8 · Voz — hecho

**Objetivo**: dictar un prompt.

`speech_to_text`, en el dispositivo, sin API ni coste. Transcribe, **lo enseña**,
y tú confirmas antes de enviar.

La confirmación no es burocracia: *"dijiste algo mal y el agente lo hizo"* es el
fallo que hunde la confianza en la voz para siempre, y ocurre una vez.

Sin comando nuevo ni pantalla nueva: `VoiceMicButton` (nuevo,
`presentation/widgets/`) se mantiene apretado, dicta en vivo, y el texto
reconocido reemplaza el contenido del composer — visible y editable, nunca
mandado solo. Se agregó en los dos composers que ya existían: el de sesión
(`terminal_screen.dart`, que ya tenía el ícono de mic como placeholder desde
Fase 6) y el de PTY directo (`pty_terminal_screen.dart`, Fase 6bis, que no
tenía ninguno). Confirmar sigue siendo el mismo botón de enviar de siempre —
`send_message` o `write_terminal`, según cuál composer sea.

**Consume**: nada nuevo del lado del Mac — es reconocimiento nativo del
teléfono, no un comando de la allowlist.
**Hecho cuando**: desde cualquiera de los dos composers, mantener apretado el
mic dicta, soltar deja el texto reconocido visible y editable, y tocar
enviar lo manda como si se hubiera tipeado.

---

## Candidatas para versiones futuras, sin fase asignada

Fases 4 a 8 son el v1 completo (ver "Es una función Pro" en
`FUNCIONALIDADES.md`). Lo de acá está expuesto en `allowlist.json` — no
exige tocar Rust — pero nadie lo consume todavía del lado del móvil.
Deliberadamente sin el detalle de una fase real: escribirlo hoy sería
especular contra pantallas que no existen (ver el aviso al principio de
este documento).

- **La cola de trabajo** (`FUNCIONALIDADES.md` §6): `queue-updated` ya
  llega al teléfono y no lo escucha nadie.
- **Fan-out** (§7): mandar un prompt a varias terminales a la vez — una de
  las razones de existir de Hangar, sin pantalla móvil.
- **Árboles de agentes** (§14): ver el avance y lanzar uno ya definido
  (crear/editar la estructura sigue ⛔, eso sí es de escritorio).
- **Retomar y auditar** (§4): conversaciones resumibles y qué ficheros tocó
  un turno.
- **Rotación y refresco real de cuotas** (§11): más allá de lo que ya
  consume Fase 7a.

Lo que sigue genuinamente sin exponer siquiera (harían falta líneas nuevas
en `allowlist.json`, algunas ni existen como comando de Hangar todavía):
`resolve_terminal_target`, `delete_session`, `read_file`/`list_files`,
`fetch_real_usage`, presupuestos/topes de cuenta, memoria del proyecto
(`CLAUDE.md`) desde el móvil, y renombrar el Mac desde el teléfono.

---

## Lo que no se construye, y por qué

**Emulador de terminal.** Es donde se va la mitad del esfuerzo del competidor, y
un `xterm` de 120 columnas no se lee en seis pulgadas.

**Notificaciones.** Descartadas en el contrato (TRD §12.5): exigen una credencial
de servidor y no hay dónde ponerla. El modo yolo es lo que ocupa su lugar.

**Shell libre.** El teléfono pide lo que Hangar sabe hacer. La única excepción
escrita es Fase 6bis: `write_terminal` sobre una terminal `kind='project'` ya
abierta en el Mac — no "abrir una shell nueva desde el teléfono a gusto", que
sigue sin existir.
