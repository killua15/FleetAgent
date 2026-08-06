# TRD — Grafo de conocimiento (Graphify)

Cómo Hangar usa [Graphify](https://github.com/Graphify-Labs/graphify) — extractor
de grafos de código por AST, local, Apache-2.0 — para que sus agentes en paralelo
dejen de trabajar a ciegas.

No es un documento de "integrar una herramienta". Es la respuesta a por qué esto
vale algo en un mercado donde cualquiera puede instalar el mismo skill en una
tarde.

**Estado**: borrador. Nada de esto está implementado.
**Dependencia externa**: `graphify` (Python 3.10+, vía `uv`/`pipx`). Opcional
para siempre — Hangar arranca y funciona sin ella.

---

## 1 · El problema

Hangar corre hasta seis agentes a la vez, cada uno en su worktree. Hoy pasan dos
cosas, las dos caras:

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

Graphify ataca los dos con el mismo artefacto.

---

## 2 · Por qué esto es un diferencial (y qué parte no lo es)

Hay que ser honesto con la parte que no lo es. **Graphify es público.** Se
instala con `uv tool install graphifyy` y se registra como skill en veinte
plataformas. Cualquier competidor puede cablearlo en una tarde. "Hangar tiene
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
afirmarlo. Las dos capacidades que salen de esa intersección son las que se
defienden:

**Diferencial A — colisión semántica entre agentes paralelos.** Avisar, al crear
una subsesión, de que ya hay otra tocando el mismo subsistema. Requiere conocer
la flota. Nadie más conoce la flota.

**Diferencial B — ahorro de cuota demostrado, en dólares.** No "el grafo ahorra
contexto" como eslogan, sino la cifra medida sobre las tareas reales del usuario,
sacada de una tabla que ya existe. Requiere medir por turno. Nadie más mide por
turno.

Y los dos componen con el borde que Hangar ya vende: un turno que gasta menos
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
semántico de docs, PDFs e imágenes usa API keys externas. El README de Hangar
dice *"nothing leaves the machine"*. Por tanto: **solo extracción AST local**,
impuesto en el código, no en la documentación. Si la única forma de indexar los
docs de un proyecto es mandarlos a un tercero, no se indexan los docs. Esto no es
negociable y no se expone como toggle "avanzado" — un toggle es una promesa rota
con pasos extra.

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

---

## 5 · Decisiones de diseño

### 5.1 · Un grafo por proyecto, no por worktree

Es la decisión que decide todo lo demás.

Los worktrees son checkouts del mismo repo. Si graphify se indexa sola en cada
uno, construimos seis grafos idénticos, seis veces, en paralelo, compitiendo por
CPU con los seis agentes. Eso convierte una optimización de cuota en una
regresión de rendimiento.

Regla: **se construye una vez en el repo principal; los worktrees leen esa
caché.** El mecanismo exacto (variable de entorno, flag de ruta de caché, o
copia dirigida) es lo primero que resuelve el spike de Fase 0 — si graphify no
permite desacoplar la caché del directorio indexado, la Fase 1 cambia de forma y
hay que saberlo antes de escribir nada.

Política de reconstrucción: al mergear a la rama base, y a mano. **Nunca por
turno.** Un grafo que se reconstruye mientras seis agentes trabajan es CPU
robada al trabajo real. Debounce, y una sola construcción en vuelo por proyecto.

### 5.2 · El MCP entra por flag, no por fichero en el repo

El CLI de Claude acepta `--mcp-config <fichero>`. Hangar mantiene un JSON en su
propio app data y lo pasa al spawn. Dos sitios:

- `build_args` (`src-tauri/src/providers/claude.rs:43`) — terminales del grid,
- `build_turn_args` (`src-tauri/src/providers/claude.rs:83`) — turnos headless.

**Sin `--strict-mcp-config.`** Ese flag ignora toda otra configuración MCP, lo
que significa apagarle al usuario sus propios servidores por el privilegio de
darle el nuestro. Inaceptable: aditivo o nada.

Esto también esquiva el problema del aislamiento por cuenta. Hangar inyecta
`CLAUDE_CONFIG_DIR` por cuenta (`providers/claude.rs:124`) precisamente para que
las cuentas no se contaminen; registrar el MCP a nivel de usuario obligaría a
escribirlo en cada config dir y a re-escribirlo con cada cuenta nueva. Un flag en
el spawn es una sola verdad, para todas las cuentas, sin tocar su estado.

### 5.3 · Detección

`providers::resolve_binary("graphify")` (`src-tauri/src/providers/mod.rs:71`) tal
cual. Ya resuelve el caso de la app lanzada desde Finder con PATH mínimo, ya
mira `~/.local/bin` — que es exactamente donde `uv tool install` deja las cosas —
y ya cachea el resultado en un mutex global, así que la detección es gratis
después de la primera.

No hace falta código nuevo de descubrimiento. Ese es el punto de haberlo escrito
bien la primera vez.

### 5.4 · Persistencia

Dos tablas nuevas, **por migración**, no en `schema.sql`. El aviso de
`schema.sql:281` es explícito: `schema.sql` se aplica a bases de datos ya
existentes antes de que corra ninguna migración, así que referenciar ahí algo que
introduce una migración tumba la app al arrancar en cualquier instalación
antigua. Entrada nueva al final de `MIGRATIONS` (`src-tauri/src/db/mod.rs:51`).

```
project_graphs(project_id PK, graph_path, commit_sha, built_at,
               node_count, edge_count, status, last_error)

session_graph_scope(session_id, community_id, PRIMARY KEY(session_id, community_id))
```

`commit_sha` es lo que hace honesta la antigüedad: un grafo construido sobre un
commit que ya no es HEAD se muestra como viejo, sin adivinar.

### 5.5 · Un grafo viejo es peor que ninguno

Un agente que consulta un grafo desactualizado recibe una respuesta con la forma
de la verdad y el contenido equivocado — y actúa sobre ella con más confianza que
si no hubiera preguntado. Por eso el estado del grafo (commit, antigüedad,
"stale") viaja con cada respuesta y se muestra en la UI. Cuando el grafo está
desfasado más allá de un umbral, **se desregistra el MCP** en vez de servir algo
en lo que no confiamos.

---

## 6 · Plan por fases

Cada fase entrega valor sola y tiene criterio de muerte. Ninguna depende de que
la siguiente se construya.

### Fase 0 — Spike (medio día). No se escribe producto.

Contra este mismo repo, que es un caso real: Rust + TypeScript, dos lenguajes,
tamaño medio.

1. Instalar graphify e indexar Hangar. Medir: tiempo de construcción, tamaño del
   grafo, CPU durante la build.
2. **Responder la pregunta de §5.1**: ¿se puede apuntar la caché fuera del
   directorio indexado, de forma que seis worktrees lean un solo grafo?
3. **Verificar el modo local puro**: indexar sin ninguna API key configurada y
   confirmar, con el tráfico observado, que no sale nada. Si el modo solo-AST no
   se puede forzar, §3 nos obliga a parar aquí.
4. Registrar su MCP a mano en una sesión de Claude y hacer tres preguntas reales
   de este repo ("¿quién llama a `resolve_binary`?", "¿qué toca la rotación?").
   Juicio cualitativo: ¿responde mejor que grep?

**Criterios de muerte** — cualquiera de estos y el proyecto se archiva con lo
aprendido escrito:
- no hay modo local puro comprobable;
- la caché no se puede compartir entre worktrees y reconstruir por worktree es
  inviable;
- la build de un repo mediano tarda tanto que compite con el trabajo de los
  agentes;
- las respuestas no son mejores que un grep bien puesto.

### Fase 1 — Detección y grafo único (G1, G2, G3)

- `resolve_binary("graphify")` + comando de estado.
- Construcción por proyecto, una en vuelo, con progreso.
- Escritura del `--mcp-config` de Hangar y su paso en los dos sitios de spawn.
- Migración con `project_graphs`.
- Reconstrucción al mergear a base y a mano.
- UI mínima: un indicador de estado del grafo en ajustes del proyecto. Nada en
  el grid — el grid es para señales gruesas, y el estado de un índice no lo es.

**Aceptación**: seis subsesiones sobre un proyecto → una construcción, un grafo,
seis agentes con la herramienta disponible, y `git status` del repo del usuario
limpio. Sin `graphify` instalado, todo lo anterior desaparece sin un solo error.

### Fase 2 — Medición (G4, Diferencial B)

`turn_usage` ya trae input, output, caché y `cost_usd` por sesión y proyecto. Se
añade a `sessions` una marca de si el turno corrió con grafo, y se compara.

- Comando de informe: tokens y dólares por turno, con grafo y sin grafo, mismo
  proyecto.
- Presentación honesta: si el ahorro es ruido, se dice. Un informe que solo sabe
  dar buenas noticias no es un informe.

**Aceptación**: el informe corre sobre datos reales y devuelve un delta con su
tamaño de muestra. Esta fase es la que convierte el argumento comercial en una
cifra propia del usuario, sobre su código, y por eso no se salta.

### Fase 3 — Colisión semántica (G5, Diferencial A)

Aquí está lo que nadie más puede copiar sin construir Hangar entero.

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

### Fase 4 — Superficie (memoria y visualización)

- El reporte markdown de graphify entra como `memory_files`
  (`schema.sql:76`), regenerado con el grafo. Es contexto de arquitectura que hoy
  cada agente re-deriva y paga; los grupos de memoria compartida ya existen
  (`src/screens/SharedMemorySettings.tsx`) y esto es exactamente su caso de uso.
- El HTML interactivo, en una pestaña de `AgentPanel`. **No en el grid**: es una
  feature densa, y esa lección ya se aprendió con el árbol de ficheros y los
  diffs.

---

## 7 · Riesgos

| Riesgo | Mitigación |
|---|---|
| **Dependencia de Python** — hoy Hangar pide Node + Rust + `claude` | Opcional siempre; detectada y marcada, nunca requerida |
| **Fuga de datos por análisis semántico** | Solo AST local, impuesto en código (§3). Sin toggle |
| **Grafo desactualizado da respuestas confiadas y falsas** | Estado y commit en cada respuesta; desregistro del MCP si está muy viejo (§5.5) |
| **Dependencia externa joven** | Versión fijada; degradación limpia; sin vendorizar |
| **Coste de CPU compitiendo con los agentes** | Una build en vuelo, debounce, nunca por turno (§5.1) |
| **Falsos positivos de colisión cansan al usuario** | Aviso descartable, nunca bloqueo; si la señal es mala, la Fase 3 se retira sin tocar 1 y 2 |
| **La competencia copia el cableado del MCP** | Es lo esperado, y es la parte que no defendemos. Las Fases 2 y 3 son las que requieren la flota y la contabilidad (§2) |

---

## 8 · Preguntas abiertas

1. **Reconstrucción y quien la paga.** ¿La build corre siempre en local (CPU del
   usuario, sin coste de cuota) o hay algún paso que quiera un modelo? Lo primero
   es la respuesta correcta; el spike debe confirmar que es la única.
2. **Proyectos sin repo git.** Hangar soporta `worktree_kind='copy'` para
   proyectos sin git. Ahí no hay "repo principal" donde anclar el grafo. Propuesta:
   fuera de alcance en Fase 1, decidir con datos de uso.
3. **Ámbito de la comunidad.** ¿Las comunidades de Leiden son suficientemente
   estables entre builds como para comparar sesiones abiertas en momentos
   distintos? Si los ids bailan, la Fase 3 necesita anclar por fichero
   representativo en vez de por id de comunidad.
4. **Instalación asistida.** ¿Hangar ofrece instalar graphify (`uv tool install`)
   o solo detecta y enlaza a las instrucciones? Instalar software en la máquina
   del usuario es una acción con consecuencias; por defecto, detectar y enlazar.
