# Roadmap v2 — la capa de flota

Reescrito el 2026-07-26 después de encontrar a **Orca** (`stablyai/orca`): un ADE con 29.5k estrellas,
2.111 forks, MIT, respaldado por Y Combinator, que corre 30+ agentes CLI en worktrees paralelos, con
Monaco, apps móviles, worktrees por SSH y releases diarias.

La v1 de este documento se escribió sin saber que existía, y planificaba con cuidado todo **menos** lo
único en lo que estamos solos. Esta v2 corrige la puntería.

---

## La estrategia en una frase

**Hangar no compite por ser un ADE. Es la capa de flota que ningún ADE tiene.**

Orca corre tus agentes; Hangar reparte tus cuentas. Se compite en el único eje donde su arquitectura
no puede seguirnos sin rehacer su modelo — y de paso deja de ser un rival frontal y pasa a ser algo
que se instala *junto* a él.

Competir "medianamente" es realista **en un eje**. En catálogo de features no se gana siendo uno.

### Los tres pilares, ya construidos

1. **Aislamiento real por cuenta.** Un `CLAUDE_CONFIG_DIR` por cuenta (`env_for_account`): N
   terminales con N cuentas distintas, a la vez.
2. **Rotación con políticas + continuidad.** `auto` / `round_robin` / `manual` por proyecto,
   detección de agotamiento, cooldown de 5 h, y el hilo de conversación que sobrevive al cambio de
   cuenta (copia del transcript + `--resume`).
3. **Auditoría de gasto.** `rotation_events` ya registra qué cuenta se usó, en qué repo, cuándo. Es
   el único de los tres que todavía no es producto — y el único que se factura.

### La evidencia de que ese eje está libre

Su propio tracker, verificado:

| Issue de Orca | Qué piden | Estado en Hangar |
|---|---|---|
| **#10757** (abierto el 26/07/2026): *"CHAT HISTORY LOST AFTER ACCOUNT SWITCH !!!!"* | conservar la conversación al cambiar de cuenta | enviado |
| **#5203** (abierto el 11/06/2026): *"el switcher está en un sitio tan global que sigue mezclando qué cuenta aplica a qué proyecto"* | cuentas por proyecto | enviado |
| **#8121**: *"auto switch cuando se pasa el límite… que la sesión no se pare"* | rotación automática | enviado |

El #5203 describe literalmente nuestra arquitectura antes de pedirla: ese usuario ya tiene `~/.claude`
y `~/.claude-personal` con alias `claudew`/`claudep`. Su estado de cuenta es **global a la app**; eso
no es una feature que les falte, es su modelo, y cambiarlo les cuesta un refactor.

### Lo que heredamos de Orca: tres gestos, ni uno más

- **El abanico**, en versión multi-cuenta: un prompt → N worktrees → **N cuentas** → comparar diffs →
  mergear el ganador. El suyo reparte N agentes sobre **una** cuota (de ahí la queja recurrente de
  "3 agentes = 3× tu cuota"). El nuestro es el único sostenible.
- **Scrollback que sobrevive al reinicio** (la "capa 2" de `TERMINAL_CONTINUITY.md`, ya analizada y
  aparcada; ahora justificada).
- **Anotar el diff antes de commitear**, que es lo que hace útil comparar cinco resultados.

### Lo que NO se hace, por escrito

Diseño gráfico tipo *design mode*, apps móviles, navegador por worktree, LSP, soporte de 30 agentes,
worktrees remotos por SSH. Cada una es meses de trabajo en el terreno donde ellos son más fuertes y
nosotros no aportamos nada. Que quede escrito para que en tres meses no vuelvan por la puerta de
atrás.

Agentes: **Claude + Codex y se para la lista**. "Rotar entre proveedores" no significa nada con uno, y
no significa más con treinta.

### Monetización

Muere el plan de $19/mes al individuo: no se sostiene contra un competidor gratis, MIT y financiado.

- **Individuos: gratis y completo**, rotación incluida. Limitar el plan gratuito a 1 cuenta era tapar
  la demo de lo único que ganamos.
- **Teams: ~$12 por usuario/mes.** Pool de cuentas compartido con política, auditoría de qué
  suscripción gastó qué en qué repo, y la frontera work/personal aplicada. Ahí hay presupuesto.

### Distribución, que es el problema de verdad

Con cero usuarios, el precio es hipotético; el anonimato no.

- **`hangar run` como wrapper**: lo que Orca, Warp, tmux o iTerm lanzan cuando quieren un agente con
  cuenta gestionada. Sus 29.000 usuarios pasan de competencia a embudo. **Requiere un spike de un
  día**: hoy la rotación funciona porque Hangar es dueño del PTY; como wrapper hay que confirmar que
  se puede detectar el rate limit y re-ejecutar `claude --resume` con la cuenta siguiente sin que el
  ADE de arriba se rompa.
- **La demanda ya está escrita y es pública**: #5203, #8121, #10757. Un comentario honesto en cada
  uno, una vez, sin spam.

### Qué es ganar

No son 29.000 estrellas. Es ser lo que la gente con varias suscripciones instala **junto** a su ADE,
y entre 10 y 50 equipos pagando por la auditoría. Eso es alcanzable siendo uno; el resto no, y
planificarlo como si lo fuera es cómo se pierde un año.

---

# Orden de las fases

```
1. PRs sobre GraphQL          ← media tarde, se nota cada 5 minutos
2. Terminal con worktree      ← prerrequisito del abanico
3. Abanico multi-cuenta       ← donde estamos solos
4. Ficheros con Monaco        ← deja de ser una casilla perdida
5. Login con Google           ← la puerta del backup, no del peaje
6. Sync SQLite ↔ Supabase     ← y el cimiento de Teams
7. Auditoría de gasto / Teams ← lo que se factura
8. Buscar PR                  ← pequeño, útil
9. Codex como segundo proveedor
```

Y el **spike del wrapper** en paralelo, cuando toque decidir distribución.

---

# Fase 1 — PRs sobre GraphQL · media tarde

> **Hecha** (2026-07-26). Una query por repo hace descubrimiento y comentarios juntos;
> `PR_FIELDS_FRAGMENT` fija el contrato de ids con un test para que nada ya visto parezca nuevo.

## Qué pasa hoy

Cada pasada del poller hace, **por repo**, un `gh pr list` más una búsqueda de `review-requested:@me`;
y **por cada PR tuyo**, un `gh pr view` de sondeo, y si algo cambió otro `gh pr view` más un
`gh api .../pulls/N/comments` para los comentarios en línea.

Con 3 proyectos y 5 PRs propios son **11-21 procesos** a ~0,5-0,9 s cada uno: **10 a 20 segundos por
pasada**. Se nota: los PRs tardan en aparecer, y bajar el intervalo de 5 minutos hoy sería peor.

## Qué podría pasar

Medido en este repo, hoy:

```
gh pr list  (una de nuestras llamadas)                       0,88 s
gh api graphql  (PRs + reviews + comentarios + reviewers)     0,40 s
```

**Una query por repo** que trae de golpe: los PRs abiertos con autor, asignados, título y ramas; sus
comentarios; sus reviews; sus `reviewThreads` (con `path` y `line`, que hoy nos cuesta una llamada
REST aparte); la búsqueda de `review-requested:@me` como campo raíz aliaseado en la misma query; y
`viewer { login }`, que elimina el `gh api user` de `fetch_gh_login`.

Tres repos ≈ **1,2 s** en total. De 15 segundos a poco más de uno, sin dependencias nuevas —
`gh api graphql` ya está instalado. Es casi con seguridad lo que hace Orca para "colocarlos súper
rápido": una query en vez de veinte procesos.

## El riesgo que hay que manejar

Los `external_id` de comentarios que ya están en la base vienen de REST y del JSON del CLI. Si
GraphQL devuelve identificadores con otra forma, **todos los comentarios ya vistos parecerían nuevos**
y la bandeja explotaría en la primera pasada. Hay que elegir los campos de GraphQL que produzcan
exactamente los mismos strings (`databaseId` cuando toque), y cubrirlo con un test que compare las
dos formas.

## Hecho cuando

Una pasada completa cuesta una query por repo, la bandeja no duplica nada de lo ya visto, y los
comentarios en línea siguen llegando con su `path:line`.

---

# Fase 2 — Terminal con su propio worktree · S/M

> **Hecha** (2026-07-26). `create_worktree_for_project` + la fila `⑃ new worktree…` del picker.
> El cuerpo de `create_subsession` se extrajo a `create_worktree_session` sin tocar la lógica de git.

## Qué puedes hacer hoy

Para trabajar en una rama aislada hay que pasar por el AgentPanel: abrir una sesión de agente, darle
a "crear subsesión" (`CreateSubsessionDialog`), que pide nombre, cuenta, modelo, effort, modo y setup
command. Eso crea el worktree **y** una sesión de agente.

O sea: **el worktree solo nace como hijo de una sesión de agente**. Trabajando solo con terminales —
que es como quedó la app — no hay sesión padre, y no hay forma de llegar al worktree.

## Qué podrías hacer después

En `+ New terminal`, tras elegir la cuenta, en el paso "¿dónde debería correr?" aparece una fila más:

```
RECENT PROJECTS
  Fleet-Agent        /Users/killua/Proyects/Fleet-Agent
  propkit            /Users/killua/Proyects/propkit
  ⑃ Nuevo worktree…     ← esto es lo nuevo
  📁 Explore another folder…
```

Eliges proyecto y escribes un nombre — `fix-billing` en Fleet-Agent. Lo que pasa:

1. `git worktree add` real en `~/Proyects/Fleet-Agent/.hangar/worktrees/fix-billing`, rama
   `subsession/fix-billing`
2. copia los `.env` del proyecto (no están en git, y sin ellos no arranca nada)
3. `.hangar/` al `.gitignore` del proyecto
4. corre el setup command si lo hay
5. abre la terminal **con el cwd en ese worktree**, con Claude bajo la cuenta elegida

Tu checkout principal ni se entera. Tres terminales sobre tres ramas del mismo repo, cada una con su
cuenta.

## Por qué es pequeña

De `create_subsession` (`sessions.rs:289`) entero, **lo único que necesita del padre es una query**:
la de `sessions.rs:300-306`, que saca el `project_id` de la sesión padre. Todo lo demás —
`create_linked_worktree`, `copy_env_files`, `ensure_worktree_dir_ignored`, `run_setup_command` —
trabaja sobre `project_path`. Le pasas el proyecto directamente, `parent_session_id` queda NULL, y el
cuerpo no se toca.

## Qué se hereda del AgentPanel y qué no

| Del diálogo de subsesión | En la terminal |
|---|---|
| Nombre / rama, cuenta, setup command | sí |
| Worktree real, o copia si no hay git | sí, igual |
| Copia de `.env` | sí, igual |
| Modelo, effort, modo | **no** |

Los tres últimos no: en una terminal los elige el propio Claude Code al arrancar. Guardarlos sería un
valor que nadie lee y que se contradice con lo que ves en pantalla.

## Lo único que hay que decidir

**Cerrar la terminal no borra el worktree.** Cerrar una pestaña no puede llevarse trabajo sin
commitear. El borrado ya existe (`delete_session`, `sessions.rs:634`, hace el `git worktree remove`
de verdad); falta el botón, y va en el Git panel, que ya lista los worktrees, con confirmación.

Coste aceptado: si abres worktrees a lo loco, se acumulan y los barres tú.

---

# Fase 3 — Abanico multi-cuenta · M/L

> **Lanzador hecho** (2026-07-26): `fan_out_prompt` + el panel `⑃ Fan out`. Un prompt, N worktrees,
> N cuentas, en serie. **Pendiente**: la vista de comparación y el merge, que esperan a Monaco
> (Fase 4); de momento se comparan en el Git panel, que ya lista los worktrees con ahead/behind.

## Qué puedes hacer hoy

Abrir varias terminales a la vez con "+ Multi terminal", cada una en un proyecto y con una cuenta. Es
la mitad de abajo del gesto. Lo que no hay: mandar **el mismo prompt** a todas, ni comparar lo que
salió, ni quedarte con uno.

## Qué podrías hacer después

Escribes un prompt una vez, eliges cuántas ramas quieres y con qué cuentas, y le das a lanzar:

```
PROMPT   arregla el redondeo de la facturación y añade tests

  ⑃ fan/redondeo-1   →  David Personal 1     ● trabajando
  ⑃ fan/redondeo-2   →  Cuenta de trabajo    ● trabajando
  ⑃ fan/redondeo-3   →  Personal 2           ✓ terminado   +84 −12  3 ficheros

                                    [ comparar ]  [ mergear #3 ]
```

Tres worktrees, tres cuentas, tres cuotas distintas. Cuando acaban, comparas los diffs y mergeas el
que quieras; los otros worktrees se quedan o se borran, tú decides.

## Por qué nuestra versión es mejor que la suya, y no es marketing

El abanico de Orca lanza N agentes que corren todos con **la única cuenta activa**: N veces el gasto
sobre la misma cuota, que es su queja recurrente. Un abanico de cinco solo es sostenible si son cinco
cuotas — y eso, hoy, solo lo puede hacer Hangar.

## De qué se compone

- Las dos piezas de abajo ya están: `create_subsession` corta worktrees y el multi-terminal ya abre
  varias con cuenta distinta cada una.
- Falta: el lanzador (un prompt → N pares rama/cuenta), el estado por rama, y **la vista de
  comparación** — que es el trabajo de verdad y se apoya en la Fase 4.
- El prompt inicial ya sabe viajar: `open_terminal` acepta `init_prompt`, que es como la bandeja de
  PRs mete los comentarios en la terminal.

## Lo que hay que decidir

Qué pasa con los worktrees perdedores: propongo dejarlos y ofrecer "borrar los N-1 restantes" en un
botón, nunca automático.

---

# Fase 4 — Ficheros con CodeMirror · M

> **Hecha** (2026-07-26), **con Monaco descartado tras medirlo**: 93 MB desempaquetados frente a
> ~1 MB de CodeMirror 6 + `@codemirror/merge`, y lo que compra ese peso (edición con IntelliSense,
> LSP) es justo la superficie que decidimos no disputar. CodeMirror también es un editor real, así
> que el techo no baja. react-arborist también fuera: nuestro árbol ya funcionaba y solo hacía falta
> extraerlo. Bundle: 988 KB → 1.576 KB.

## Qué puedes hacer hoy

El árbol de ficheros y el visor existen, funcionan bien (colorean por estado de git, marcan borrados,
diff del agente + contenido actual) y **solo se llega dentro del panel de un agente**:
`list_session_files` recibe un `session_id` y `FileTreePanel` vive dentro de `AgentPanel.tsx`.

## Qué podrías hacer después

Una pantalla `Files` al lado de Git, con la estructura del Git panel — proyectos a la izquierda,
contenido a la derecha — y **el editor de VS Code de verdad** en el panel de contenido.

## El cambio de plan que lo hace ganable

La v1 planeaba un visor casero compitiendo contra Monaco. Con licencias comprobadas:

| | Estrellas | Licencia | Para qué |
|---|---|---|---|
| **microsoft/monaco-editor** | **46.4k** | **MIT** | el editor de VS Code, como paquete npm |
| **jameskerr/react-arborist** | 3.7k | MIT | el árbol (virtualizado, drag&drop, multi-select) |
| coder/code-server | 78.5k | MIT | ❌ VS Code entero **como servidor**: mala forma para Tauri |
| eclipse-theia/theia | 21.6k | EPL-2.0 | ❌ framework de IDE, otra licencia |
| voideditor/void | 28.9k | Apache-2.0 | ❌ es un editor completo, no un componente |

**Monaco + react-arborist**, los dos MIT, los dos npm. Deja de ser "nuestro visor contra Monaco" y
pasa a ser **el mismo Monaco**. `lib/diffLines.ts` se retira: Monaco trae `DiffEditor` nativo.

Nota: el árbol de ficheros de VS Code **no es extraíble** (vive dentro del producto, no en Monaco).
De ahí react-arborist.

## Lo demás

- **Backend**: generalizar a un target en vez de una sesión, como ya hicieron las terminales:
  `list_files(target)` / `read_file(target, path)` con `target = project | session`, reusando
  `resolve_target`. Los comandos por sesión quedan como envoltorio para no tocar el AgentPanel.
- **Extracción**: `FileTreePanel`/`FileTreeRows` salen de `AgentPanel.tsx` a `components/FileTree.tsx`.

## El agujero que se tapa en esta fase

`read_session_file` (`sessions.rs:908`) hace:

```rust
std::fs::read_to_string(Path::new(&cwd).join(&path))
```

Sin comprobar nada. Y `join` con una ruta **absoluta descarta la base entera**: `path = "/etc/passwd"`
lee `/etc/passwd`, y `"../../.ssh/id_rsa"` lee tu clave privada. Hoy es inofensivo porque el único que
llama es el árbol del AgentPanel, que solo manda rutas que él generó; convertirlo en el visor general
de la app es exactamente lo que lo vuelve un problema. Se canonicaliza y se comprueba que cae bajo la
raíz resuelta, con test, **en esta fase**.

---

# Fase 5 — Login con Google sobre Supabase · M/L

> **Hecha salvo lo que depende de Google** (2026-07-26). Esquema `profiles` + `subscriptions` con RLS
> aplicado al proyecto real; el `SECURITY DEFINER` del trigger quedaba expuesto como RPC y se le
> revocó el EXECUTE (el linter de seguridad de Supabase ya sale limpio). En la app: servidor loopback
> en `auth.rs` (sin dependencias nuevas), PKCE, refresh token en el llavero y botón en la barra.
> **Bloqueado en ti**: crear el OAuth client en Google Cloud, pegarlo en Supabase, y añadir
> `http://127.0.0.1:47823/callback` a las Redirect URLs.

## Qué puedes hacer hoy

Nada, y no es exageración: **la app no habla con ninguna red**. Ni `reqwest` en `Cargo.toml` ni
`supabase-js` en `package.json`. Todo es local: SQLite, el llavero del sistema y los CLIs. Por eso
esta fase es la más grande aunque "solo" sea un botón: es la primera capa de red del producto.

## Qué podrías hacer después

Un `Sign in with Google` en la barra de arriba. Se abre tu navegador, eliges la cuenta, la pestaña se
cierra sola, y en la barra queda tu email. Cierras la app, la reabres, sigues dentro.

**Y aquí está el cambio de propósito frente a la v1**: el login ya no es la puerta del peaje, es **la
puerta del backup**. Nadie se registra para que le cobren; sí se registra para no perder su
configuración. Habilita la Fase 6, que es algo que el usuario quiere.

## Las tres piezas

**1. El proyecto de Supabase** — ya existe (`yqrvofogmyqdqufyxlfk`, ca-central-1, ACTIVE_HEALTHY) y
está **vacío**: 0 tablas, 0 edge functions.

- Habilitar el provider de Google. Requiere un OAuth client en Google Cloud Console: **eso lo tiene
  que crear el usuario**; el client id y el secret van en la config de Supabase.
- Esquema con RLS desde el minuto uno:
  - `profiles(id → auth.users, email, created_at)` — cada uno ve la suya.
  - `subscriptions(user_id, status, plan, current_period_end, stripe_customer_id, …)` — el usuario
    **solo lee** la suya; escribir es exclusivo del webhook con service role. Si la app puede escribir
    su propia suscripción, no hay monetización: hay una casilla que cualquiera marca.

**2. El flujo OAuth de escritorio**, que no es el de web: PKCE con `skipBrowserRedirect: true`, abrir
el navegador del sistema con `plugin-opener`, y recibir el código en un **servidor loopback**
(`http://127.0.0.1:<puerto>`) que Rust levanta solo mientras dura el login. Loopback y no deep link
(`hangar://`): funciona igual en `tauri dev` que en el bundle, sin registrar esquemas en macOS.

**3. Dónde vive el token**: el refresh token **al llavero** (`credential_vault.rs`, que ya usa
`keyring`). Nunca a `localStorage` — cualquier cosa que corra en el webview lo lee.

## La regla

**La app funciona sin login.** Es una herramienta local que orquesta CLIs que ya pagas; pedir cuenta
para abrir una terminal es hostil. El login es la puerta de Teams y del backup, no la de la app.

---

# Fase 6 — Sync SQLite ↔ Supabase · M/L

## Qué puedes hacer hoy

Nada: si se muere el disco, se va todo. Y cambiar de máquina significa reconfigurar cuentas,
proyectos, políticas de rotación y cuentas permitidas a mano.

Esto **corrige un supuesto de la v1**, que decía "nada de sync". Estaba mal, por un motivo que no
había visto: **el ledger de auditoría en Postgres es exactamente lo que Teams necesita**. Con sync, el
dashboard de equipo es una query, no un sistema nuevo. El sync no es un coste: es lo que convierte la
auditoría en producto.

## Qué podrías hacer después

Instalas Hangar en otra máquina, entras con Google, y tus proyectos, cuentas (sin secretos),
políticas y todo el histórico de uso están ahí.

## El problema central no es el sync: es la identidad del proyecto

Hoy `projects.path` es absoluto (`/Users/killua/Proyects/Fleet-Agent`). En otra máquina eso no existe.
La clave portable es el **remote de git**, que ya detectamos con `detect_repo_owner_and_name`.

- `projects.remote_url` → **identidad que sincroniza**
- `projects.path` → **estado local por dispositivo, nunca sale**

## El contrato de qué sale y qué no, que no es negociable

| Sincroniza | Nunca sale de la máquina |
|---|---|
| proyectos (por remote), políticas de rotación, cuentas permitidas, ajustes | credenciales del llavero |
| metadatos de cuenta: nombre, scope, provider, plan | contenido de los config dirs |
| tracking: `turn_usage`, `rotation_events` (append-only) | transcripts `.jsonl` — ahí está tu código y tus conversaciones enteras |
| | copias de `.env`, rutas absolutas |

## Modelo de conflictos: el más simple que es correcto

- Tablas de **configuración**: pequeñas y de un solo usuario → *last-write-wins* por fila con
  `updated_at`. Hace falta añadir esa columna donde no está.
- Tablas de **tracking**: append-only con PK uuid → **no pueden conflictuar por construcción**.

Nada de CRDTs. Y RLS: cada fila lleva `user_id`, cada usuario ve solo la suya.

## La regla de seguridad

**SQLite local es la fuente de verdad; Supabase es espejo, nunca autoridad.** Restaurar es una acción
explícita del usuario, jamás un pull automático que sobrescriba. El sync es donde se pierden los
datos, y se pierden solos.

---

# Fase 7 — Auditoría de gasto y Teams · M

## Qué puedes hacer hoy

`rotation_events` y `turn_usage` ya guardan lo que hace falta, y nadie los mira. La pregunta *"¿qué
cuenta de la empresa gastó cuánto, en qué repo, cuándo?"* no se puede responder desde la app.

## Qué podrías hacer después

Una pantalla que la responde, por proyecto, por cuenta y por periodo. Y con la Fase 6 hecha, la misma
tabla en Postgres da el dashboard de equipo casi gratis.

Es el nicho que paga: un manager que reparte cinco suscripciones de Claude entre ocho personas hoy no
tiene forma de saber quién gastó qué. **Y es el ángulo que Orca no va a priorizar**, no por
incapacidad sino por dirección: van a por superficie de IDE.

## Lo que trae Teams

Pool de cuentas compartido con política, auditoría, y la frontera work/personal aplicada en vez de
solo avisada. ~$12 por usuario/mes.

---

# Fase 8 — Buscar PR · S

## Qué puedes hacer hoy

Elegir un proyecto y mirar sus PRs. No hay búsqueda, y ahora que el descubrimiento trae **todos** los
abiertos del repo, la lista de un repo con tráfico se recorre a ojo.

## Qué podrías hacer después

Una caja que filtra **entre todos los proyectos a la vez** por número, título, autor, repo y rama.
Filtrado local sobre lo que ya está en SQLite: cero llamadas a `gh`, respuesta mientras tecleas. Y en
`⌘K`, escribir `#128` o pegar una URL salta a su tarjeta.

## El caso que no se puede resolver, y por qué

Traer un PR que el descubrimiento no vio se hace con `gh pr view <número|url>` — **siempre que ese
repo sea un proyecto de Hangar**. Para un repo que no lo es, la búsqueda puede encontrarlo pero no se
ofrece revisarlo: sin clon local no hay worktree, y el botón fallaría después de ilusionarte. Ahí la
única respuesta honesta es "añádelo como proyecto primero".

---

# Fase 9 — Codex como segundo proveedor · M

## Qué puedes hacer hoy

Nada: `account_command_line` (`terminals.rs:322`) rechaza explícitamente cualquier provider que no
sea claude — *"Hangar can't run a 'codex' CLI as a specific account yet"* — porque no existe el
equivalente de `env_for_account` para ellos.

## Por qué está al final y por qué está

Al final porque no desbloquea nada que no tengamos. Y está porque **"rotar entre proveedores" no
significa nada con un solo proveedor**, y ese es el titular de la fase 3 en cuanto haya dos.

El límite honesto: entre CLIs distintos **no hay transferencia de sesión**. Los transcripts no
comparten esquema. Lo que sí se puede llevar es el contexto renderizado, que es lo que `handoff.rs`
ya hace — y es también lo que su #5592 pide.

---

# Supuestos vivos

1. **El sync no lleva secretos ni transcripts.** Ver el contrato de la Fase 6.
2. **La distribución es fuera de la App Store** (DMG / GitHub Releases), así que Stripe vale y no hay
   comisión del 15-30% de Apple.
3. **Su ventaja puede evaporarse**: el #5203 lleva mes y medio abierto, pero cerrarlo es voluntad, no
   años. Si parten el config dir por worktree, nos queda la rotación con políticas, la continuidad y
   la auditoría. Y va en los dos sentidos: **son MIT y pueden copiarnos**. La velocidad importa más
   que el sigilo.
