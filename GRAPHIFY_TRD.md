# TRD — Grafo de conocimiento (Graphify)

Cómo Hangar usa [Graphify](https://github.com/Graphify-Labs/graphify) — extractor
de grafos de código por AST, local, Apache-2.0 — para que sus agentes en paralelo
dejen de trabajar a ciegas, incluso cuando el sistema en el que trabajan son
varios repositorios.

No es un documento de "integrar una herramienta". Es la respuesta a por qué esto
vale algo en un mercado donde cualquiera puede instalar el mismo skill en una
tarde.

**Estado**: Fases 0–5 implementadas en `feat/graphify-multi-repo-graph`. 483
tests unitarios más uno de integración que recorre la cadena entera contra dos
repos reales (`--ignored`, §6). La Fase 5 se entregó **distinta de como este
documento la describía**, y el porqué está escrito en su sección.
**Dependencia externa**: `graphifyy` en PyPI, binario `graphify` (Python ≥3.10,
vía `uv`). Opcional para siempre — Hangar arranca y funciona sin ella.

---

## 1 · El problema

Hangar corre hasta seis agentes a la vez, cada uno en su worktree. Hoy pasan tres
cosas:

**Cada agente arranca ciego.** Lo primero que hace un turno nuevo es orientarse:
veinte greps, leer ficheros enteros para descartarlos, reconstruir desde cero un
mapa del repo que el agente de al lado ya reconstruyó hace diez minutos. Eso se
paga en tokens, y Hangar los mide uno a uno en `turn_usage`
(`src-tauri/src/db/schema.sql:255`). Es cuota que se gasta en no saber dónde
están las cosas.

**Los worktrees evitan colisiones de fichero, no de criterio.** `.hangar/worktrees/<slug>`
garantiza que dos agentes nunca escriban la misma línea. No garantiza nada sobre
que dos agentes reescriban el mismo subsistema en direcciones opuestas: los dos
PRs quedan verdes, los dos mergean, y el conflicto aparece en producción. El
aislamiento por worktree resolvió el problema mecánico y dejó el semántico
intacto.

**Y el proyecto acaba en el borde del repo.** Un sistema real son varios: un
servicio que llama a otro, una app y su backend. El agente que edita uno no tiene
forma de saber qué hay al otro lado del contrato — y es justo el sitio donde
equivocarse sale más caro, porque ahí no hay compilador que avise. Hangar ya
gestiona los dos proyectos; lo que no sabe es que son el mismo sistema.

Graphify ataca los tres con el mismo artefacto, si se le pide bien: los dos
primeros como viene, el tercero construyendo lo que no trae (§5.6).

---

## 2 · Por qué esto es un diferencial (y qué parte no lo es)

Hay que ser honesto con la parte que no lo es. **Graphify es público.** Se
instala con `uv tool install graphifyy` —el paquete de PyPI es `graphifyy` con
dos íes, el binario que deja es `graphify`; no es una errata, es una trampa que
ya nos costó un rato— y se registra como skill en veinte plataformas. Cualquier competidor puede cablearlo en una tarde. "Hangar tiene
grafo de conocimiento" no es un foso; es una casilla de checklist que se copia
sola.

El foso está en la intersección. Hangar es el único orquestador que sabe, a la
vez, tres cosas:

1. **qué agentes están corriendo** (`sessions`, el registro de terminales),
2. **sobre qué están trabajando** (worktree, rama, prompt),
3. **cuánto costó cada turno** (`turn_usage`, con input/output/caché separados
   porque se facturan distinto).

Un competidor con grafo pero sin (1) no puede detectar colisiones entre agentes:
no sabe que hay flota. Uno sin (3) no puede demostrar el ahorro: solo puede
afirmarlo. De ahí salen dos capacidades defendibles, y hay una tercera que no
sale de la intersección sino de la propia herramienta:

**Diferencial A — colisión semántica entre agentes paralelos.** Avisar, al crear
una subsesión, de que ya hay otra tocando el mismo subsistema. Requiere conocer
la flota. Nadie más conoce la flota.

**Diferencial B — ahorro de cuota demostrado, en dólares.** No "el grafo ahorra
contexto" como eslogan, sino la cifra medida sobre las tareas reales del usuario,
sacada de una tabla que ya existe. Requiere medir por turno. Nadie más mide por
turno.

**Diferencial C — grupos multi-repo que de verdad conectan.** Marcar que dos
proyectos cualesquiera son un mismo sistema, indexarlos juntos y verlo.

Aquí hay que afinar, porque la versión ingenua de este argumento es falsa:
**graphify sí trae multi-repo**. Tiene `merge-graphs` y un grafo global
(`global add --as <tag>`). Lo que no trae es que sirva: medido contra dos repos
reales, la fusión produce **cero aristas entre ellos** (§5.7). Es una unión de
dos grafos en un fichero, con los ids prefijados por repo. Dos islas.

El diferencial, entonces, no es "construimos lo que no existe" —eso sería
mentira— sino **hacer que lo que existe conecte**, y saber quién está tocando
cada lado. Es una pieza más pequeña de lo que parecía y está mejor apuntada:
§5.7 la mide y §5.6 la diseña.

Y los tres componen con el borde que Hangar ya vende: un turno que gasta menos
contexto es un turno que no agota la cuenta, que es exactamente el problema que
la rotación existe para sobrevivir. El grafo no es una feature paralela — hace
que la propuesta de valor actual funcione mejor.

---

## 3 · Qué NO es

**No se vendoriza graphify.** Apache-2.0 es compatible con AGPL en esa dirección,
pero meterla dentro nos hace responsables de su superficie de dependencias
Python. Se invoca como binario externo, igual que `claude` y `gh`.

**No escribimos nuestro propio extractor.** Si graphify desaparece, la feature se
degrada; no la reemplazamos con un tree-sitter propio. El coste de mantener eso
supera a lo que aporta.

**No se toca el repositorio del usuario.** Ni un `.mcp.json` versionado, ni un
`.graphify/` en su árbol de trabajo, ni un hook de git instalado a su espalda.
Hangar ya tiene fama de no ensuciar; el MCP entra por `--mcp-config` en el spawn
(§5.2) y la caché vive en el app data de Hangar. Un usuario que desinstala Hangar
no debe encontrar residuos en su repo.

**No sale nada de la máquina.** Graphify parsea código en local, pero su análisis
semántico de docs, PDFs e imágenes va por subagentes de Claude. El README de
Hangar dice *"nothing leaves the machine"*. Por tanto: **solo extracción AST
local**, impuesto en el código, no en la documentación. Si la única forma de
indexar los docs de un proyecto es mandarlos a un tercero, no se indexan los
docs. Esto no es negociable y no se expone como toggle "avanzado" — un toggle es
una promesa rota con pasos extra.

Y se impone con una bandera que existe: **`graphify extract <path> --code-only`**
—"index code (local AST, no API key) and skip doc/paper/image files"—. Verificado
en el spike contra este repo: *"--code-only: skipping 69 non-code file(s) (19
docs, 0 papers, 50 images) — no LLM extraction"*. Ni una llamada a un modelo, ni
una API key leída.

Es una bandera, o sea que se puede olvidar. Va en el único sitio que construye la
línea de comandos de graphify, con un test que falle si alguien la quita.

**No se instalan sus integraciones.** Graphify trae `graphify hook install`
(git hook post-commit) y `graphify claude install` (escribe en el `CLAUDE.md`
local). Los dos hacen exactamente lo que el párrafo anterior prohíbe. Hangar no
los invoca nunca. Es una regla sobre el código que llama a graphify, no un
consejo: quien quiera instalarlos, que los instale — pero no en su nombre y sin
que se entere.

**No es obligatorio.** Sin `graphify` en el sistema, Hangar se comporta como hoy.
Mismo trato que Codex y Gemini: reconocido, marcado, sin romper nada.

**No es un grafo por worktree.** Ver §5.1 — es el detalle del que depende que
esto funcione o sea un desastre.

---

## 4 · Objetivos

| # | Objetivo | Verificable por |
|---|---|---|
| G1 | `graphify` detectado y opcional | Hangar arranca igual sin él; la UI lo marca |
| G2 | Un grafo por proyecto, compartido por los N worktrees | Seis subsesiones → una sola construcción |
| G3 | Toda sesión recibe el MCP sin tocar el repo del usuario | `git status` limpio tras abrir seis agentes |
| G4 | Delta de tokens medido, no afirmado | Informe A/B desde `turn_usage` |
| G5 | Aviso de colisión al crear subsesión | Dos tareas sobre el mismo subsistema → aviso |
| G6 | El usuario agrupa proyectos y obtiene un grafo del conjunto | Dos repos marcados como un sistema → una construcción, un grafo |
| G7 | El grafo se puede mirar | El HTML del grupo se abre desde Hangar |

---

## 5 · Decisiones de diseño

### 5.1 · Un grafo por ámbito, nunca por worktree

Es la decisión que decide todo lo demás.

Los worktrees son checkouts del mismo repo. Si graphify se indexa sola en cada
uno, construimos seis grafos idénticos, seis veces, en paralelo, compitiendo por
CPU con los seis agentes. Eso convierte una optimización de cuota en una
regresión de rendimiento.

Regla: **se construye una vez en el repo principal; los worktrees leen esa
caché.**

Esto ya no es una apuesta, y resultó más fácil de lo previsto: **`graphify
extract <path> --out <dir>`** escribe el `graphify-out/` donde le digas, sin
tocar la ruta indexada. Verificado en el spike — se indexó este repo entero
apuntando la salida al scratchpad y `git status` no se movió. Además todos los
comandos de consulta (`query`, `path`, `explain`, `affected`, `god-nodes`)
aceptan `--graph <path>`, así que los worktrees leen el grafo del repo principal
sin copiar nada.

La duda que esta sección dejaba como bloqueante de la Fase 0 está resuelta a
favor, y con una bandera en vez de con una artimaña.

(Cuando el proyecto pertenece a un grupo, el grafo compartido es el del grupo y
no el del proyecto — §5.6.)

Política de reconstrucción: al mergear a la rama base, y a mano. **Nunca por
turno.** Un grafo que se reconstruye mientras seis agentes trabajan es CPU
robada al trabajo real. Debounce, y una sola construcción en vuelo por ámbito.

### 5.2 · El MCP entra por flag, no por fichero en el repo

El CLI de Claude acepta `--mcp-config <fichero>`. Hangar mantiene un JSON en su
propio app data y lo pasa al spawn. Dos sitios:

- `build_args` (`src-tauri/src/providers/claude.rs:43`) — terminales del grid,
- `build_turn_args` (`src-tauri/src/providers/claude.rs:87`) — turnos headless.

**Sin `--strict-mcp-config.`** Ese flag ignora toda otra configuración MCP, lo
que significa apagarle al usuario sus propios servidores por el privilegio de
darle el nuestro. Inaceptable: aditivo o nada.

Esto también esquiva el problema del aislamiento por cuenta. Hangar inyecta
`CLAUDE_CONFIG_DIR` por cuenta (`providers/claude.rs:137`) precisamente para que
las cuentas no se contaminen; registrar el MCP a nivel de usuario obligaría a
escribirlo en cada config dir y a re-escribirlo con cada cuenta nueva. Un flag en
el spawn es una sola verdad, para todas las cuentas, sin tocar su estado.

### 5.3 · Detección

`providers::resolve_binary("graphify")` (`src-tauri/src/providers/mod.rs:141`) tal
cual. Ya resuelve el caso de la app lanzada desde Finder con PATH mínimo, ya
mira `~/.local/bin` — que es exactamente donde `uv tool install` deja las cosas —
y ya cachea el resultado en un mutex global, así que la detección es gratis
después de la primera.

No hace falta código nuevo de descubrimiento. Ese es el punto de haberlo escrito
bien la primera vez.

### 5.4 · Persistencia

Tablas nuevas, **todas por migración**, ninguna en `schema.sql`. El aviso de
`schema.sql:281` es explícito: `schema.sql` se aplica a bases de datos ya
existentes antes de que corra ninguna migración, así que referenciar ahí algo que
introduce una migración tumba la app al arrancar en cualquier instalación
antigua. Entrada nueva al final de `MIGRATIONS` (`src-tauri/src/db/mod.rs:51`).

Un grafo no cuelga de un proyecto: cuelga de un **ámbito**, que es un proyecto
suelto o un grupo (§5.6). Así la misma tabla sirve para los dos casos y no hay
dos caminos paralelos que mantener.

```
graphs(scope_kind, scope_id, graph_path, built_at,
       node_count, edge_count, status, last_error,
       PRIMARY KEY(scope_kind, scope_id))          -- scope_kind: 'project' | 'group'

graph_commits(scope_kind, scope_id, project_id, commit_sha,
              PRIMARY KEY(scope_kind, scope_id, project_id))

session_graph_scope(session_id, community_id, PRIMARY KEY(session_id, community_id))
```

`commit_sha` es lo que hace honesta la antigüedad: un grafo construido sobre un
commit que ya no es HEAD se muestra como viejo, sin adivinar. Va en tabla aparte
y no como columna porque un grafo de grupo se construyó sobre **N** commits, uno
por miembro, y basta con que uno se mueva para que el conjunto esté viejo (§5.5).

Las tablas del agrupamiento, en §5.6. Todo por migración, no en `schema.sql`.

### 5.5 · Un grafo viejo es peor que ninguno

Un agente que consulta un grafo desactualizado recibe una respuesta con la forma
de la verdad y el contenido equivocado — y actúa sobre ella con más confianza que
si no hubiera preguntado. Por eso el estado del grafo (commit, antigüedad,
"stale") viaja con cada respuesta y se muestra en la UI. Cuando el grafo está
desfasado más allá de un umbral, **se desregistra el MCP** en vez de servir algo
en lo que no confiamos.

En un grupo esto se endurece: **la frescura del conjunto es la del peor miembro.**
Si un miembro avanzó veinte commits y el otro no se ha movido, el grafo del grupo
está viejo, y lo está entero — porque lo que el usuario le preguntará es
precisamente cómo se relacionan los dos. Un grafo de grupo medio
fresco no existe: o vale, o se desregistra.

---

### 5.6 · Grupos: varios proyectos, un grafo

Un agente que trabaja en un servicio y llama a otro no tiene forma de saber qué
hay al otro lado. Es el mismo problema del §1 —arrancar ciego— pero en el borde
donde más caro sale equivocarse, porque ahí ni siquiera hay compilador que avise.

**Esto vale para dos proyectos cualesquiera de la lista**, sin categorías ni
tipos especiales: un backend y su app, un servicio y su SDK, un monorepo partido
en tres. Los ejemplos de este documento son ejemplos.

El usuario marca qué proyectos son un mismo sistema. No se infiere: adivinar qué
repos van juntos a partir del nombre o de las dependencias es exactamente el tipo
de magia que produce agrupaciones que nadie pidió.

**Modelo.** Tabla propia, con la misma forma que la agrupación que Hangar ya
tiene para memoria compartida (`memory_groups` / `memory_group_projects`,
`schema.sql:64`):

```
graph_groups(id PK, name, created_at)
graph_group_projects(group_id, project_id, PRIMARY KEY(group_id, project_id))
```

**El mismo patrón, no la misma tabla.** Reutilizar `memory_groups` ahorraría una
pantalla y costaría la propiedad que hace esto usable: cambiar con quién
compartes memoria pasaría a disparar una reconstrucción de grafo, y no podrías
tener un ámbito sin el otro. Son dos preguntas distintas —"¿qué contexto
comparten?" y "¿qué código se indexa junto?"— y se responden por separado.

**Cómo se unen: con su mecanismo, no con el nuestro.** Graphify ya sabe fusionar
grafos. Un `extract --code-only --out <dir>` por proyecto miembro y un
`merge-graphs g1.json g2.json --out <grupo>.json`. Nada de raíces compuestas ni
de enlaces simbólicos: es un comando suyo, documentado, y los ids salen
prefijados por repo (`g-fleet::components`) con un campo `repo` en cada nodo, que
es justo lo que hace falta para saber de qué lado viene cada cosa.

Se descarta su **grafo global** (`global add --as <tag>`,
`~/.graphify/global-graph.json`): es uno solo por máquina, y aquí hacen falta N
grupos independientes. `merge-graphs --out` da exactamente eso.

Beneficio secundario de fusionar en vez de indexar junto: el propio graphify
recomienda partir y fusionar cuando dos ficheros acuñan el mismo nombre —lo
avisó al indexar `hangar-mobile`, donde `FlutterMacOS` aparecía en tres ficheros
y se perdían nodos—. La ruta que elegimos por diseño resulta ser también la que
él sugiere para no perder información.

**Lo que la fusión no hace, y tenemos que hacer nosotros: conectar.** Está
medido en §5.7. Es la única pieza de lógica propia de este documento, y es
pequeña: un puente entre nodos de repos distintos que comparten `norm_label`, un
campo que graphify ya calcula. Un join, no un extractor. §3 sigue en pie.

**Acceso, no solo conocimiento.** Un grafo que le habla al agente de código que
no puede abrir es una crueldad. `providers::skills_args`
(`src-tauri/src/providers/mod.rs:121`) ya pasa `--add-dir` para claude, kimi y
grok, y ya está documentado allí que esa bandera concede acceso a ficheros además
de cargar skills. Un agente en un proyecto agrupado recibe `--add-dir` de sus
hermanos: mismo mecanismo, sin inventar nada.

**UI.** Diseñada en §5.8.

### 5.7 · Qué conecta un grafo de grupo — medido, no supuesto

Es la sección que decide si §5.6 vale algo, y ya no es una opinión: se midió en
la Fase 0 sobre **Hangar + `hangar-mobile`**, dos repos que se hablan de verdad
por un protocolo documentado (`MOBILE_BRIDGE_TRD.md`) y en dos lenguajes que no
comparten compilador (Rust/TypeScript y Dart).

| Medida | Resultado |
|---|---|
| Nodos | 2 613 (Hangar) + 1 867 (mobile) = 4 480 |
| Aristas | 7 332 + 2 862 = 10 194 |
| **Aristas que cruzan de un repo al otro** | **0** |
| Comunidades que mezclan los dos repos | **0 reales** |
| Etiquetas normalizadas presentes en ambos | **53** (con ruido: ver abajo) |

**`merge-graphs` es una unión, no una fusión.** Los totales suman exacto: no
inventa ni una arista. La predicción de que el AST no cruza el borde del repo era
correcta — entre dos servicios el vínculo es HTTP, una cola o un schema, y nada
de eso es una llamada de código.

> **Trampa que casi nos come.** El primer conteo dijo "105 de 117 comunidades
> mezclan los dos repos". Es falso: cada grafo se clusteriza por separado y los
> ids de comunidad **colisionan** (`community 49` existe en los dos). La fusión
> no reclusteriza. Cualquier métrica de grupo que se calcule sobre `community`
> tal cual da un número bonito y mentiroso. Anotado aquí porque la Fase 4 se
> apoya en comunidades y habría heredado el error.

**Lo que sí conecta: 53 etiquetas compartidas, y son las que importan.** No es
ruido léxico — es la superficie del protocolo:

```
accountspend      src-tauri/src/usage_meter.rs   ↔  lib/domain/entities/spend_overview.dart
commitfilediff    …                              ↔  lib/domain/entities/commit_file_diff.dart
changedfile       …                              ↔  lib/domain/entities/worktree_changes.dart
billing           src/types/account.ts           ↔  lib/domain/entities/pickable_account.dart
envelope, session, sessionblock, permissionprompt, prcomment, tokentotals, …
```

Ese es exactamente el sitio donde un agente que edita un lado rompe el otro sin
enterarse. La señal existe; lo que falta es dibujarla.

**Conclusión de diseño.** El puente lo pone Hangar: una arista entre nodos de
repos distintos con el mismo `norm_label`. Es determinista, sin modelo, sin
coste de cuota, y se apoya en un campo que graphify ya calcula. Con esos puentes
la clusterización del grafo del grupo sí puede mezclar, y §5.5 y la Fase 4
recuperan sentido.

**Y no todos los 53 valen.** En la misma lista aparecen `build`, `base`,
`description`, `command`, `datetime` — nombres genéricos que coinciden por
casualidad y que unirían medio grafo con el otro medio. El puente necesita un
filtro, y la primera versión es la barata: descartar etiquetas por debajo de una
longitud, las que aparecen en muchos ficheros dentro de su propio repo, y una
lista de nombres comunes. Es afinable y medible —el caso Hangar + `hangar-mobile`
tiene respuesta conocida—, pero **no está resuelto**: es el primer trabajo real
de la Fase 2, no un detalle de implementación.

Y la regla de honestidad se mantiene: si en un grupo concreto los puentes salen
a cero, **se dice en la UI**. Dos islas dibujadas juntas no son un sistema.

### 5.8 · Cómo se ve y cómo se toca

**El HTML de graphify no se embebe.** Hay que corregir lo que este documento dio
por hecho: `graph.html` **no es autocontenido**. Carga `vis-network` desde
`unpkg.com` con un `<script src="https://…">`. Bajo el CSP de la app se queda en
blanco, sin red no funciona, y viene en un único tema oscuro que no es el
nuestro. Verificado abriendo el fichero, no leyendo su documentación.

De ahí, dos superficies en vez de una:

- **La vista de Hangar**, dibujada por nosotros desde `graph.json`, con los
  tokens de `src/styles/tokens.css` y los tres temas.
- **"Abrir la vista completa de graphify"**, acción secundaria que lanza su HTML
  en el navegador del sistema. Fuera del CSP, fuera de nuestra promesa de temas,
  y etiquetada como lo que es: una herramienta externa.

**Qué dibuja la vista de Hangar: la costura, no la madeja.** Un grafo de fuerzas
de 4 480 nodos queda bien en una captura y no responde ninguna pregunta — y
además graphify ya lo hace. La vista propia es un **mapa de dos raíles**: un repo
por raíl, los ficheros que participan en el contrato, y una curva por puente.
Responde la única pregunta que tiene un grupo: *¿por dónde se tocan estos dos
repos, y qué fichero hay a cada lado?*

Dibujarlo destapó algo que en una lista no se ve: `models.rs` y
`pull_request.dart` concentran seis puentes cada uno. **Son los hubs de la
costura**, y es donde una edición descuidada rompe el otro lado.

**Dónde vive cada parte:**

| Superficie | Qué muestra |
|---|---|
| Settings → **Graph groups** | Tres columnas, el esqueleto de `SharedMemorySettings.tsx`: grupos, miembros, y —en la tercera— **los puentes**. Donde memoria compartida lista ficheros, esto lista la prueba de que agrupar sirvió |
| AgentPanel → pestaña **Graph** | El mapa de costura, filtrado a los ficheros que la sesión tocó |
| Diálogo de crear subsesión | El aviso de colisión (Fase 4), con el tipo compartido concreto |
| **El grid** | Nada. Un índice y su antigüedad no son una señal gruesa |

**Dos decisiones que parecen detalle y no lo son:**

*La frescura se muestra por miembro, no solo por grupo.* §5.5 manda que el
conjunto valga lo que su peor miembro, pero un aviso que solo dice "viejo" no
dice qué reconstruir. Chip por proyecto (`fresh`, `18 commits behind`) **y**
aviso de grupo.

*El recuento de puentes se muestra siempre, incluido el cero.* Un grupo con cero
puentes tiene que verse en la lista, no descubrirse abriendo el grafo. Es la
misma regla de §5.7 llevada al sitio donde el usuario decide si deshacer la
agrupación.

Mockup con datos reales del spike: `docs/graphify/ui-mockup.md`.

---

## 6 · Plan por fases

Cada fase entrega valor sola y tiene criterio de muerte. Ninguna depende de que
la siguiente se construya.

### Fase 0 — Spike. ✅ Ejecutado

Contra Hangar (Rust + TypeScript, 182 ficheros de código) y `hangar-mobile`
(Dart/Swift/Kotlin), que se hablan por un protocolo real. Resultados:

| Pregunta | Respuesta |
|---|---|
| Coste de construcción | **6,0 s** Hangar (2 613 nodos / 7 332 aristas / 105 comunidades), **2,8 s** mobile. El miedo a competir por CPU con los agentes era infundado |
| ¿Se puede sacar la salida del repo? | Sí, `extract --out <dir>`. `git status` intacto tras indexar |
| ¿Modo local puro? | Sí, `--code-only`: *"skipping 69 non-code file(s) — no LLM extraction"*. Cero llamadas a modelo |
| ¿Multi-repo? | Sí, `merge-graphs` — pero une sin conectar: **0 aristas cruzadas** (§5.7) |
| ¿Hay señal cruzada aprovechable? | Sí, **53 etiquetas compartidas**, y son la superficie del protocolo (§5.7) |

**Criterios de muerte: ninguno se cumplió.** El de los grupos estuvo cerca —las
aristas cruzadas son cero y las comunidades no mezclan— pero no se dispara,
porque la señal existe en otro campo (`norm_label`) y el puente que la convierte
en aristas es determinista y de nuestro lado (§5.7). Si el puente no hubiera
aparecido, §5.6 se archivaba aquí.

**Lo que el spike descubrió y no estaba en el guion:**
- El paquete de PyPI es `graphifyy`, el binario `graphify`. Instalar el nombre
  "correcto" falla con *"no versions of graphify"*.
- Requiere Python ≥3.10; el del sistema en macOS es 3.9.6. `uv` es prerrequisito
  real, no un detalle de instalación (§8.4 deja de ser teórica).
- **Los `.sql` no aportan nada** sin `pip install "graphifyy[sql]"`. En este repo
  eso significa que `schema.sql` —el corazón del modelo de datos— es invisible
  para el grafo. El extra va en la instalación, no es opcional para nosotros.
- Marcó `tokens.css` como "potencialmente sensible" y lo saltó. Falso positivo
  inofensivo, pero conviene saber que ese filtro existe y puede tragarse ficheros.
- Hay `--cargo` (deps crate→crate desde `Cargo.toml`), que en un repo Rust añade
  la capa de dependencias que el AST solo no ve. Candidato para la Fase 1.

**Lo que queda por probar y no bloquea nada:** registrar el MCP a mano
(`graphify-mcp`) y juzgar si responde mejor que un grep. Se hace al empezar la
Fase 1, con el grafo ya construido.

El spike es reproducible: `docs/graphify/spike.sh <repo-a> <repo-b>` imprime las
cifras de §5.7 para cualquier par de proyectos.

**Y la cadena entera tiene su propio test**, que es lo que prueba que las piezas
encajan y no solo que cada una funciona por separado:

```sh
HANGAR_GRAPH_TEST_REPO_A=~/Proyects/Fleet-Agent \
HANGAR_GRAPH_TEST_REPO_B=~/Proyects/hangar-mobile \
  cargo test --lib -- --ignored the_whole_chain
```

Construye, fusiona, puentea, reclusteriza y renderiza el brief contra dos repos
de verdad, y comprueba que `git status` de los dos sigue limpio — la promesa de
§3, que es la única que no se puede deducir leyendo código. Corriéndolo contra
Hangar + `hangar-mobile` da **29 puentes** (el spike ve 54 candidatos crudos; la
diferencia son los 25 que descarta el filtro de §5.7).

### Fase 1 — Detección y grafo único (G1, G2, G3). ✅ Hecha

- `resolve_binary("graphify")` + comando de estado.
- Construcción por proyecto, una en vuelo, con progreso:
  `graphify extract <repo> --code-only --out <app data>/graphs/<scope>`. Las tres
  banderas son obligatorias y van en un solo constructor de línea de comandos,
  con test: `--code-only` es la promesa de §3, `--out` la de "no tocar el repo".
- Instalación con el extra de SQL (`graphifyy[sql]`), o `schema.sql` no existe
  para el grafo. Evaluar `--cargo` para la capa de dependencias entre crates.
- Escritura del `--mcp-config` de Hangar y su paso en los dos sitios de spawn.
- Migración con `graphs` y `graph_commits`, ya con `scope_kind = 'project'` — el
  ámbito nace generalizado aunque la Fase 1 solo use un valor, porque añadirlo
  después obliga a una segunda migración sobre la clave primaria.
- Reconstrucción al mergear a base y a mano.
- UI mínima: un indicador de estado del grafo en ajustes del proyecto. Nada en
  el grid — el grid es para señales gruesas, y el estado de un índice no lo es.

**Aceptación**: seis subsesiones sobre un proyecto → una construcción, un grafo,
seis agentes con la herramienta disponible, y `git status` del repo del usuario
limpio. Sin `graphify` instalado, todo lo anterior desaparece sin un solo error.

### Fase 2 — Grupos multi-repo y visualización (G6, G7, Diferencial C). ✅ Hecha

Va aquí, y no al final, porque es la pieza que la Fase 0 dejó apuntada y porque
las dos partes se validan la una a la otra: el grafo del grupo solo es creíble si
se puede mirar.

- `graph_groups` / `graph_group_projects` y su pantalla, con el patrón de
  `SharedMemorySettings.tsx`.
- Construcción de grupo: un `extract` por miembro y un `merge-graphs … --out`
  (§5.6). Sin raíces compuestas ni symlinks — se usa su comando.
- **El puente por `norm_label`** (§5.7) y reclusterización del grafo del grupo.
  Es la única lógica de grafo propia del plan, y hay que tratarla como tal: test
  con dos grafos de juguete, y el número de puentes visible.
- `--add-dir` de los proyectos hermanos en el spawn, vía `skills_args`.
- Frescura por miembro: `graph_commits`, y el peor manda (§5.5).
- El mapa de costura en una pestaña de `AgentPanel`, dibujado por nosotros
  (§5.8). **No en el grid**: es una feature densa, y esa lección ya se aprendió
  con el árbol de ficheros y los diffs.
- **La métrica de §5.7 en la UI**: cuántos puentes hay y entre qué ficheros. Si
  el grupo son dos islas, el usuario lo ve y deshace la agrupación.

**Aceptación**: dos proyectos agrupados producen un grafo con nodos de ambos y
**más de cero puentes**, listados con su fichero a cada lado; un agente en uno de
ellos puede abrir ficheros del otro; el HTML se abre desde Hangar; y `git status`
de los dos repos sigue limpio. Deshacer el grupo deja los proyectos exactamente
como estaban.

Contra Hangar + `hangar-mobile` eso son 53 puentes conocidos: es un caso de
prueba con respuesta esperada, no un "a ver qué sale".

### Fase 3 — Medición (G4, Diferencial B). ✅ Hecha

`turn_usage` ya trae input, output, caché y `cost_usd` por sesión y proyecto. Se
añade a `sessions` una marca de si el turno corrió con grafo, y se compara.

- Comando de informe: tokens y dólares por turno, con grafo y sin grafo, mismo
  proyecto.
- Presentación honesta: si el ahorro es ruido, se dice. Un informe que solo sabe
  dar buenas noticias no es un informe.

**Aceptación**: el informe corre sobre datos reales y devuelve un delta con su
tamaño de muestra. Esta fase es la que convierte el argumento comercial en una
cifra propia del usuario, sobre su código, y por eso no se salta.

### Fase 4 — Colisión semántica (G5, Diferencial A). ✅ Hecha

Aquí está lo que nadie más puede copiar sin construir Hangar entero. Y sobre un
grupo vale doble: el aviso deja de ser "dos agentes tocan el mismo módulo" y pasa
a ser "estos dos agentes están tocando los dos lados del mismo contrato, cada uno
en su repo".

- Al crear una subsesión (`src/components/CreateSubsessionDialog.tsx:47`), se
  consulta el grafo con el nombre y el prompt de la tarea y se resuelven las
  comunidades tocadas → `session_graph_scope`.
- Si otra sesión **activa** ya tiene esa comunidad, aviso en el diálogo: qué
  sesión, qué subsistema, qué rama.
- **Aviso, nunca bloqueo.** A veces dos agentes sobre el mismo subsistema es
  exactamente lo que el usuario quiere. Hangar informa; el usuario decide.
- El mismo dato alimenta el panel de la flota: qué está tocando cada agente, en
  términos de arquitectura y no de rutas de fichero.

**Aceptación**: dos subsesiones sobre el mismo subsistema, descritas con palabras
distintas, producen aviso. Dos sobre subsistemas ajenos, no. El falso positivo se
tolera; el silencio ante una colisión real, no.

### Fase 5 — Superficie (memoria). ✅ Hecha, y **distinta de como se planeó**

Lo que esta sección decía: meter el `GRAPH_REPORT.md` de graphify como
`memory_files` (`schema.sql:76`). No se hizo, por dos razones que aparecieron al
implementarla y que merecen quedar escritas.

**La aritmética no salía.** Ese fichero son 25 KB para este repo — unos 6 000
tokens en el system prompt de *cada* turno. Gastar contexto para ahorrar
contexto es un intercambio que pierde, y además habría contaminado justo la
comparación de la Fase 3: los turnos "con grafo" arrancarían 6 000 tokens por
delante y el informe habría medido nuestro propio lastre.

Lo que se entrega es un **brief compacto**: los ficheros más conectados —el
grado de un fichero en el grafo no es algo que veinte greps revelen— y, en un
grupo, la costura. Unos cientos de tokens, y un test que falla si pasa de 4 KB.

**Y `memory_files` no llega a ningún agente.** Nada ha alimentado nunca esa
tabla a un turno; por eso su propia pantalla está oculta, y `SettingsTabs.tsx`
lo dice con todas las letras. Escribir el brief ahí habría sido entregar una
fase rota. Va por `--append-system-prompt-file`, el canal verificado con un
canario, y **además** se guarda en `memory_files` para que sea visible: el
usuario puede leer lo que se les está contando a sus agentes, que es la
diferencia entre contexto y una caja negra.

La lección general: una fase que escribe en un canal que nadie lee no está
hecha, está simulada.

---

## 7 · Riesgos

| Riesgo | Mitigación |
|---|---|
| **Dependencia de Python** — hoy Hangar pide Node + Rust + `claude` | Opcional siempre; detectada y marcada, nunca requerida |
| **Fuga de datos por análisis semántico** | `extract --code-only`, verificado (§3), en un solo constructor de comando y con test que falla si se quita |
| **Instalar sus integraciones a espaldas del usuario** | `graphify hook install` y `graphify claude install` no se invocan nunca (§3). Confirmado que existen y escriben en `CLAUDE.md` y en los hooks de git |
| **Grafo desactualizado da respuestas confiadas y falsas** | Estado y commit en cada respuesta; desregistro del MCP si está muy viejo (§5.5) |
| **Un grupo produce dos islas y el grafo no aporta nada** | Medido: la fusión da 0 aristas cruzadas, y el puente por `norm_label` las repone. El recuento de puentes se muestra en la UI (§5.7) |
| **Métricas de grupo calculadas sobre `community`** | Los ids de comunidad colisionan entre grafos fusionados. Se reclusteriza tras el puente, o no se usan (§5.7) |
| **Ficheros invisibles para el grafo** | `.sql` necesita el extra `graphifyy[sql]`; el filtro de "sensibles" puede saltarse ficheros. Ambos se comprueban al construir, no en producción |
| **Dependencia externa joven** | Versión fijada; degradación limpia; sin vendorizar |
| **Coste de CPU compitiendo con los agentes** | Una build en vuelo, debounce, nunca por turno (§5.1) |
| **Falsos positivos de colisión cansan al usuario** | Aviso descartable, nunca bloqueo; si la señal es mala, la Fase 4 se retira sin tocar las anteriores |
| **La competencia copia el cableado del MCP** | Es lo esperado, y es la parte que no defendemos. Las Fases 2, 3 y 4 son las que piden el puente que la fusión no da, la contabilidad y la flota (§2) |

### Tres fallos que destapó implementarlo

Escritos porque los tres son del tipo que vuelve, y ninguno se veía leyendo el
diff que los introdujo.

**`bool::then_some` evalúa de forma ansiosa.** El lock de construcción (§5.1)
hacía `set.insert(k).then_some(BuildGuard { key })`. Cuando la inserción falla
—o sea, cuando hay que rechazar una segunda construcción— `then_some` ya ha
construido el guard, y destruirlo bloquea el mismo mutex que seguíamos
teniendo cogido. `std::sync::Mutex` no es reentrante: la segunda construcción
simultánea habría congelado *todas* las del proceso hasta reiniciar la app. Lo
destapó un test que no fallaba, **colgaba**. Es `then`, con closure.

**`store_turn_usage` nunca guardó `project_id`.** Anterior a este trabajo, y
mucho más ancho: **ningún** informe por proyecto veía los turnos headless, no
solo el del grafo.

**`#[serde(default)]` no cubre `null`.** Solo cubre la clave ausente. Un grafo
real trae 306 de 4 480 nodos con `"source_file": null`, así que el fichero
entero fallaba al parsear y construir cualquier grupo moría con *"is not a
graphify graph"*. Los tests unitarios no lo vieron porque todos usaban
fixtures escritas a mano; lo cazó el test de integración la primera vez que
corrió. Es la razón de que ese test exista.

---

## 8 · Preguntas abiertas

1. ~~**Reconstrucción y quien la paga.**~~ **Cerrada.** El pase semántico de
   graphify sí gasta cuota —registra tokens por run en `graphify-out/cost.json`—
   y el AST no. Como §3 nos deja solo con el AST, la build es siempre local y
   gratis en cuota. La pregunta era si eso era posible; lo es.
2. **Proyectos sin repo git.** Hangar soporta `worktree_kind='copy'` para
   proyectos sin git. Ahí no hay "repo principal" donde anclar el grafo, ni
   `commit_sha` con el que juzgar su frescura. Propuesta: fuera de alcance en
   Fase 1, decidir con datos de uso.
3. **Ámbito de la comunidad.** ¿Las comunidades de Leiden son suficientemente
   estables entre builds como para comparar sesiones abiertas en momentos
   distintos? Si los ids bailan, la Fase 4 necesita anclar por fichero
   representativo en vez de por id de comunidad. La Fase 0 ya dio media
   respuesta y no es buena: entre dos grafos distintos los ids **colisionan**
   sin significar nada. Anclar por fichero parece la apuesta segura.
4. ~~**Instalación asistida.**~~ **Cerrada: detectar y enlazar, nunca instalar.**
   En un macOS limpio hacen falta dos pasos —`uv` primero, porque el Python del
   sistema es 3.9.6— y la respuesta la dio el registro de capacidades
   (`src-tauri/src/capabilities.rs`): cada función declara lo que necesita y la
   app enseña **el primer eslabón que falta**, no el último. Instalar software
   en la máquina de alguien sigue siendo cosa suya.

   Lo que queda: las pistas asumen Homebrew, así que en un Mac sin `brew` el
   comando tampoco es ejecutable. Es el mismo error un nivel más abajo.
5. **Sacar un proyecto de un grupo.** ¿Se reconstruye el grafo al momento —CPU
   por una edición de ajustes— o se marca viejo y se reconstruye cuando toque?
   Marcarlo viejo es más barato y más honesto, pero deja un grafo que menciona
   código que el agente ya no puede abrir. Probablemente: marcar viejo y quitar
   el `--add-dir` de inmediato, que es la mitad que sí importa.
6. **Cómo se nombra esto en la UI.** "Grupo de grafo" describe la
   implementación; el usuario está pensando en "estos dos servicios son un
   sistema". Se decide con la pantalla delante, no aquí.
