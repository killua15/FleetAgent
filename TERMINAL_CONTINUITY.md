# Continuidad de terminales: persistir sesiones y rotar cuenta sin perder la conversación

Análisis de dos preguntas relacionadas, con lo verificado separado de lo propuesto:

1. Si cierro Fleet con terminales abiertas, ¿pueden volver — y con la conversación que tenían?
2. ¿Se puede rotar de cuenta/CLI manteniendo la misma conversación de la terminal?

Estado: **análisis, nada implementado**. Fecha: 2026-07-25. Verificado contra Claude Code
v2.1.219 y el config dir real `~/.claude-work` (47 transcripts) en esta máquina.

---

## 0. Qué pasa hoy

Al cerrar Fleet:

- **Los procesos no sobreviven, y es deliberado.** `shutdown_children` (`src-tauri/src/lib.rs:32`)
  llama `terminals::kill_all` (`terminals.rs:678`), que mata cada shell; al vaciar el registry se
  dropea el master del PTY, con lo que el `claude` de dentro recibe SIGHUP y cae con ella.
- **No se recuerda nada.** `TerminalRegistry` es un `HashMap` en memoria. Al arrancar, la vista
  está vacía.
- **Rotar de cuenta empieza conversación nueva.** Es lo que dice hoy el banner de cuota. La razón
  asumida hasta ahora era que el transcript vive en el config dir de la cuenta y `--resume` no
  cruza config dirs. La sección 2 demuestra que esa razón es incompleta.

---

## 1. Persistir terminales entre cierres

Cuatro capas independientes. Se pueden implementar por separado y dan cosas distintas.

### Capa 1 — que los paneles vuelvan

Tabla `open_terminals`: target (`kind` + `source_id`), cuenta, posición en el grid, y el
`InitCommand` con el que se abrió. Espejo en disco del registry: insert en `open_terminal`
(`terminals.rs:366`), delete en `close_terminal` (`terminals.rs:583`).

- **El cwd no se guarda.** `resolve_target` ya lo deriva; guardarlo sería una segunda fuente de
  verdad que se desincroniza al mover un worktree.
- **Tabla y no JSON en settings**, por las FKs: con `ON DELETE CASCADE`, borrar un proyecto o una
  cuenta se lleva la fila y nunca intentas restaurar algo que ya no existe.
- **El punto que se rompe si se hace a lo bruto**: `kill_all` vacía el registry al cerrar la app.
  Si comparte camino con `close_terminal`, cerrar Fleet borra exactamente lo que querías recordar.
  Hay que distinguir *"el usuario cerró esta terminal"* (olvidar) de *"se está cerrando Fleet"*
  (recordar).
- **Restaurar desde el frontend, no desde `setup()`**: en `setup()` todavía no hay panel que adopte
  los PTYs, y abrirlos sin nadie leyéndolos llena scrollbacks a ciegas y alarga el arranque.

### Capa 2 — que el texto que veías vuelva

Fleet ya guarda 256 KB de scrollback por terminal (`SCROLLBACK_LIMIT_BYTES`, `terminals.rs:41`) y
`attach_terminal` (`terminals.rs:531`) ya lo reproduce en el xterm al navegar fuera y volver.
Persistir ese anillo al cerrar y precargarlo al restaurar es reusar maquinaria que existe.

- **Choca con la capa 3**: si además retomas la conversación, el CLI la reimprime, y verías la
  conversación dos veces — una muerta arriba, la viva debajo. Con capa 3, esta capa solo aporta el
  output de shell anterior a `claude`.
- Necesita un separador visual (`── restaurado ──`) para que el usuario no teclee sobre un prompt
  que parece vivo y no lo está.

### Capa 3 — que la conversación siga (recomendada)

`claude --help` en esta máquina expone lo necesario:

```
--session-id <uuid>    Use a specific session ID for the session
-r, --resume [value]   Resume a conversation by session ID
--fork-session         When resuming, create a new session ID
-n, --name <name>      Set a display name for this session
```

Diseño:

1. Al abrir una terminal, Fleet genera un UUID y arranca
   `CLAUDE_CONFIG_DIR=<dir> claude --session-id <uuid>`, guardando el uuid en la fila.
2. Al restaurar, `claude --resume <uuid>` en el mismo cwd y bajo la misma cuenta.

**Por qué `--session-id` y no `--continue`.** `--continue` retoma "la más reciente de esta
carpeta". El grid permite dos terminales en la misma carpeta y cuenta, y las dos retomarían la
*misma* conversación: dos procesos escribiendo el mismo transcript. Con un id por terminal es
imposible, y no hay que adivinar nada.

**La verificación es exacta.** El transcript de la sesión `<uuid>` en el cwd `<slug>` es:

```
<config_dir>/projects/<slug>/<uuid>.jsonl
```

donde `<slug>` es la ruta con las barras convertidas en guiones. Como el uuid lo asigna Fleet, la
comprobación es "existe este fichero concreto", no "coge el más nuevo del directorio" — lo cual
importa porque **el slug es lossy**: `/private/tmp/claude-501/-Users-killua-…` se aplana a
`-private-tmp-claude-501--Users-killua-…`, así que una carpeta con guiones y una ruta con barras
colisionan.

**Regla**: si el fichero no existe, arrancar limpio con un uuid nuevo. Nunca reutilizar un id cuyo
estado no conocemos.

**Y el fichero solo existe si hubo conversación de verdad** — ver evidencia E4. Así que la
comprobación distingue exactamente lo que interesa: la terminal que usaste vuelve con su hilo, la
que solo abriste vuelve limpia.

### Capa 4 — que el proceso siga vivo (descartada)

Es la única que da "exactamente como lo dejé": scrollback real, un `npm run build` a medias, lo
escrito sin enviar. Requiere un multiplexor que sobreviva a Fleet.

- **tmux no está instalado** en esta máquina; `screen` sí, pero es el de Apple (viejo, mal
  candidato). `claude` tiene una flag `--tmux` para worktrees.
- Sobre las capas 2+3 aporta poco (el hilo vuelve igual) a cambio de una dependencia dura y de que
  el pane herede status line y prefijo de teclas de tmux.

---

## 2. Rotar manteniendo la conversación

### Caso A — otra cuenta del mismo CLI (claude → claude): viable

**El hallazgo que lo sostiene: una conversación no está atada a una cuenta.** Barridos los 47
transcripts del config dir (64 claves distintas), **no hay ninguna clave de identidad de cuenta**
— ni `account*`, ni `organization`, ni `email`, ni `userId`, ni credenciales. Las únicas ataduras
son `sessionId`, `cwd`, `version` y `gitBranch`, y las dos primeras las controla Fleet.

Lo que hace que la conversación *parezca* intransferible es solo dónde vive el fichero: un problema
de sistema de archivos, no de identidad.

**El estado de una sesión son tres rutas, enumerables:**

```
<config_dir>/projects/<slug>/<uuid>.jsonl   ← la conversación
<config_dir>/file-history/<uuid>/           ← historial de ediciones de la sesión
<config_dir>/session-env/<uuid>/            ← entorno de la sesión
```

Los otros dos candidatos del config dir no son por sesión: `sessions/` usa nombres numéricos tipo
PID (`3774.json`) y `shell-snapshots/` va por timestamp.

**No hay atajo oficial**: el CLI no tiene export/import ni forma de apuntar a un transcript
arbitrario — solo `--session-id`, `--resume` y `--fork-session`. La transferencia consiste en poner
los ficheros donde el CLI los espera.

**Depende de la capa 3.** Sin `--session-id`, Fleet no sabe *qué* copiar: solo podría adivinar "el
transcript más nuevo de esa carpeta", justo la heurística que la capa 3 elimina.

**Reglas no opcionales:**

- **Copiar preservando el uuid**, nunca renombrar: cada línea del `.jsonl` lleva su `sessionId`
  dentro, así que un fichero renombrado tendría nombre y contenido en desacuerdo.
- **Copiar, no mover.** Si el resume falla, no se ha perdido la conversación. Encaja con la
  disciplina que ya tiene `reopen_with_account_inner` (`terminals.rs:638`): abrir la nueva antes de
  cerrar la vieja.
- **Fleet debe registrar en qué config dir vive la copia autoritativa.** Al copiar, el mismo id
  existe en dos cuentas con futuros divergentes; sin ese registro, rotar de vuelta más tarde
  resucitaría la copia vieja y perdería lo hecho en la otra cuenta. Es una columna en la fila de la
  terminal, y Fleet lo sabe porque es quien rota.
- **Verificar que el resume levantó** antes de cerrar la terminal vieja; si no, caer al
  comportamiento de hoy (conversación nueva) diciéndolo en el banner.

**Los tres costes reales:**

1. **El primer turno en la cuenta nueva paga el contexto entero**, porque retomar replantea la
   conversación. En este config dir: mediana 133 KB, máximo 18,2 MB. No anula el propósito (la
   ventana de 5 h es por cuenta), pero rotar una conversación enorme no es gratis y las muy grandes
   las compactará el CLI.
2. **El prompt de confianza de carpeta.** La cuenta destino nunca ha visto ese directorio y la
   confianza se recuerda por config dir, así que la primera rotación a esa cuenta aterriza en
   `Is this a project you created or one you trust?`. Ninguna flag lo evita (ver E3).
3. **Formato interno no documentado.** Es cirugía sobre el estado privado de otro producto: una
   actualización de Claude Code puede cambiar el layout y Fleet se rompería en silencio. Decisión de
   producto, no técnica.

**El pago**: convierte el banner de *"reabrir empieza una conversación nueva"* en rotación de
verdad — lo que se quería desde el principio.

### Caso B — otro CLI (claude → codex/gemini): no de verdad

- Los transcripts no comparten esquema; copiar no significa nada entre CLIs.
- Lo único honesto es **llevar el contexto, no la sesión**: renderizar el `.jsonl` a texto y pasarlo
  como prompt inicial. Fleet ya tiene el mecanismo — `open_terminal` acepta un `init_prompt`, que es
  como "enviar comentarios de PR a una terminal" mete la review en el agente.
- Se pierde el historial de herramientas y el estado de ediciones, y hay un problema de tamaño:
  18 MB no caben en un prompt, así que habría que resumir (cuesta un turno) o truncar a los últimos
  N mensajes.
- **Y hoy está bloqueado antes**: `account_command_line` (`terminals.rs:297`) rechaza cualquier
  provider que no sea claude — *"Fleet can't run a 'codex' CLI as a specific account yet"* — porque
  no existe el equivalente de `env_for_account` para ellos. Rotar de CLI no es alcanzable por una
  razón que no tiene nada que ver con conversaciones.

---

## 3. Evidencia verificada

| # | Afirmación | Cómo se comprobó | Resultado |
|---|---|---|---|
| E1 | El transcript no lleva identidad de cuenta | Barrido de los 47 `.jsonl` de `~/.claude-work/projects`, recolectando todas las claves JSON | 64 claves distintas, **ninguna** de cuenta/org/email/credencial. Ataduras: `sessionId`, `cwd`, `version`, `gitBranch` |
| E2 | El CLI permite fijar y retomar un id de sesión | `claude --help` (v2.1.219) | `--session-id <uuid>`, `-r/--resume [value]`, `--fork-session`, `-n/--name`. Sin export/import ni ruta de transcript arbitraria |
| E3 | Una carpeta nueva dispara el prompt de confianza | Arrancado `claude --session-id <uuid>` en un directorio nuevo dentro de un PTY (`script`), capturando la salida | Se detiene en `Is this a project you created or one you trust?` antes de cualquier prompt |
| E4 | El transcript se crea al conversar, no al arrancar | El poller de `/usage` arranca `claude` en `$HOME` cada 5 min, le escribe `/usage` y lo mata. Contados los transcripts del slug de `$HOME` | **1** fichero, de Jul 7, pese a muchos arranques. Arrancar (incluso usar un slash command) no deja transcript |
| E5 | Ruta y forma del transcript | Inspección de `~/.claude-work/projects/**` | `projects/<slug>/<uuid>.jsonl`, con `<slug>` = ruta con `/` → `-`. Junto a él puede haber un dir `<uuid>/` con artefactos (workflows) |
| E6 | El estado de sesión vive en tres sitios | Listado de `sessions/`, `file-history/`, `session-env/`, `shell-snapshots/` | `file-history/<uuid>/` y `session-env/<uuid>/` van por sesión; `sessions/*.json` es numérico tipo PID y `shell-snapshots/` por timestamp |
| E7 | Coste de retomar | `stat` de tamaño sobre los 47 transcripts | Mediana 133 KB, máximo 18,2 MB |
| E8 | tmux no está disponible | `which tmux screen` | `tmux not found`; `/usr/bin/screen` (el de Apple) |

**E3 invalida el experimento directo de E4**: la prueba se detuvo en el prompt de confianza, así que
la ausencia de transcript allí no prueba nada por sí sola. E4 se sostiene sobre la evidencia
indirecta del poller, que es fuerte pero indirecta. En cualquier caso, la regla de diseño
("`--resume` solo si el fichero existe y no está vacío") hace que la respuesta no cambie nada.

---

## 4. Incógnita abierta

**¿Acepta `claude --resume <uuid>` un transcript que ese config dir nunca creó?** Si el CLI mantiene
un índice propio de sesiones además de los ficheros, un `.jsonl` plantado a mano podría ser
invisible. Es la única cosa que decide si el caso A funciona.

Experimento: copiar una sesión pequeña de `~/.claude-work` a `~/.claude-David` e intentar retomarla
ahí. Dos minutos y reversible (es aditivo, no borra nada), pero escribe dentro del config dir de
otra cuenta — decisión del usuario.

---

## 5. Decisiones pendientes

1. ¿La restauración es automática al abrir Fleet, o pregunta?
2. ¿Los paneles restaurados arrancan el CLI solos, o vuelven en frío con un botón de arrancar?
3. ¿Se asume el acoplamiento al formato interno de Claude Code para el caso A (con verificación y
   fallback), o se deja la rotación como está hoy?

## 6. Orden recomendado

1. **Capa 1 + capa 3** — paneles que vuelven con su hilo. Es el bloque con mejor relación
   valor/riesgo, y la capa 3 es prerrequisito de todo lo demás.
2. **Caso A** (rotar copiando la sesión) — solo después de resolver la incógnita de la sección 4.
3. **Capa 2** solo si, después de usarlo, se echa de menos el output de shell.
4. **Capa 4 y caso B**: fuera. La 4 aporta poco sobre 2+3 a cambio de una dependencia dura; el caso
   B es un producto distinto y peor, y no conviene venderlo como rotación.

## 7. Alcance estimado (capas 1+3)

- `schema.sql` + migración: tabla `open_terminals` (target, cuenta, `session_uuid`, posición, init).
- `terminals.rs`: espejo en `open_terminal`/`close_terminal`, con la distinción frente a `kill_all`;
  `--session-id` como un brazo más del match en `account_command_line`; variante `Resume` en
  `InitCommand`; comando de restauración.
- `Terminals.tsx`: llamada de restauración al montar y adopción de los paneles.
- Tests: el espejo se escribe y se borra donde toca; `kill_all` no borra; el comando sale con la
  flag correcta; la detección del transcript; y una fila cuyo target ya no existe se descarta sin
  tumbar el resto de la restauración.
