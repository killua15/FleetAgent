# Hangar Mobile — funcionalidades y flujos

Qué tiene que poder hacer la app móvil, con el flujo de cada cosa y **si el
escritorio ya lo permite o no**.

Este documento no es el contrato — el contrato es `MOBILE_BRIDGE_TRD.md`. Esto es
el inventario de trabajo: lo que se puede montar hoy, lo que necesita una línea
en `allowlist.json`, y lo que no debe exponerse nunca.

## El número que ordena el resto

```
comandos en Hangar          150
expuestos al puente          35   ← lo que el móvil puede pedir hoy
no expuestos                115
```

De esos 115, la mayoría no hacen falta o no deben salir del Mac. Pero **unos
quince sí**, y entre ellos están cosas sin las que «manejar Hangar desde el
móvil» no es cierto: no se puede abrir una terminal, no se puede ver la cola, y no
se puede poner una terminal en yolo.

Cada funcionalidad de abajo lleva marca:

- ✅ **ya se puede** — está en la lista blanca
- ➕ **falta exponer** — el comando existe en Hangar; es una línea en
  `allowlist.json`
- 🔧 **falta construir** — no existe equivalente remoto, hay que escribirlo
- ⛔ **nunca** — se queda en el Mac a propósito

Y nivel: `watch` (mirar) · `steer` (dirigir) · `launch` (crear trabajo).

---

## 1 · Emparejar y vivir emparejado

### Emparejar un teléfono

1. En el Mac: ajustes → emparejar un teléfono → se elige el nivel.
2. El Mac abre una oferta que **caduca en 120 s** y dibuja un QR.
3. El teléfono lo escanea. Dentro va: id del Mac, nombre, **la clave de cifrado** y
   el nivel. La clave viaja solo ahí, nunca por red.
4. El teléfono manda `hello` por el canal. El Mac lo descifra con la clave de la
   oferta — poder descifrarlo **es** la prueba de que vio el QR.
5. En el Mac aparece un diálogo: «este dispositivo quiere emparejarse». **Escanear
   no es autorizar.**
6. Al aceptar, el Mac guarda la clave en el llavero y responde al `hello` con su
   nombre, la versión del protocolo y **la lista exacta de comandos que ese nivel
   permite**.
7. El teléfono guarda la clave en el almacén seguro del sistema y construye su
   interfaz con esa lista.

Estados que la app tiene que saber contar: QR caducado · esperando aprobación
(`E_PENDING_APPROVAL`) · rechazado · versión de protocolo incompatible
(`E_PROTOCOL_VERSION`, con las dos versiones en el mensaje para poder decir «qué
actualizar»).

### Sustituir el dispositivo

Solo hay **un teléfono por Mac**. Emparejar otro tumba al anterior, que recibe
`E_DEVICE_REPLACED` y tiene que cerrar sesión y decir por qué. Nada de turnos ni
disputas: esa restricción es lo que elimina la mitad de la complejidad.

### Revocar

Desde el Mac, en ajustes. El teléfono se queda sin canal en el acto.

| Comando | Estado | Nivel |
|---|---|---|
| `bridge_begin_pairing`, `bridge_accept_pairing`, `bridge_revoke_device` | ⛔ solo desde el Mac | — |

---

## 2 · La flota: qué está pasando ahora mismo

La pantalla que se abre al desbloquear el teléfono. Responde a una pregunta:
*¿alguna terminó, alguna está atascada?*

### Flujo

1. Al conectar, el teléfono pide el estado de cada Mac emparejado.
2. Se suscribe a los eventos y a partir de ahí **no vuelve a preguntar**: el Mac
   empuja los cambios.
3. Cada Mac se pinta con sus terminales, agrupadas por proyecto.

### Lo que llega solo, sin pedirlo

```
terminal-state             una terminal cambió de estado
terminal-accounts-updated  cambió la cuenta de una terminal
session-status             waiting → running → reviewing → done
session-ended              terminó un turno
session-output             un bloque nuevo de la conversación
pr-comments-found          hay comentarios nuevos en un PR
queue-updated              cambió la cola de trabajo
```

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Terminales y su estado | `list_terminals`, `list_terminal_states` | ✅ | watch |
| Proyectos | `list_projects` | ✅ | watch |
| Sesiones del espacio | `list_workspace_sessions`, `list_project_sessions` | ✅ | watch |
| Varios Macs | un canal por Mac, en paralelo | ✅ | watch |

**Mac inalcanzable** es un estado normal, no un error: sin servidor propio, un Mac
dormido no tiene puente. La lista tiene que distinguir «este Mac no contesta» de
«este Mac no tiene nada corriendo».

---

## 3 · Abrir y cerrar terminales ← el hueco más grande

Hoy el móvil **no puede empezar trabajo**, solo continuar el que ya existe. Puede
abrir un worktree desde una tarea (`open_task_worktrees`) y una revisión de PR
(`open_pr_review_terminal`), pero no lo más simple: «abre una terminal en este
proyecto con esta cuenta».

### Flujo propuesto

1. El teléfono lista proyectos y cuentas disponibles.
2. Elige proyecto, o un worktree concreto, y proveedor + cuenta.
3. Opcionalmente escribe el primer prompt.
4. El Mac abre la terminal y responde con su id.
5. El teléfono navega a esa terminal y ya recibe sus eventos.

**Detalle técnico que afecta al diseño:** `open_terminal` pide `cols` y `rows`
porque abre un PTY de verdad. El teléfono no dibuja un terminal, así que manda un
tamaño nominal (120×30) y no lo enseña nunca. Que exista ese parámetro no
significa que el móvil tenga que tener una rejilla de caracteres.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Abrir una terminal | `open_terminal` | ➕ | launch |
| Cerrarla | `close_terminal` | ➕ | launch |
| Sugerir la cuenta menos gastada | `suggest_terminal_account` | ➕ | watch |
| Resolver a qué apunta una terminal | `resolve_terminal_target` | ➕ | watch |
| Reabrir con otra cuenta (misma conversación) | `reopen_terminal_with_account` | ➕ | steer |
| Renombrarla | `rename_terminal` | ✅ | steer |
| `attach`/`detach`/`resize`/`restore` | — | ⛔ ciclo de vida del PTY, no del móvil | — |

---

## 4 · Dirigir un agente

El corazón de la app.

### Flujo: mandar un prompt

1. El teléfono está en la pantalla de una terminal.
2. Al entrar, marca la selección: **el Mac lo pinta en su propio panel**. Un
   agente dirigido desde fuera no puede parecer igual que uno que nadie toca.
3. Se escribe (o se dicta) el prompt y se manda.
4. Llegan bloques por `session-output` mientras el agente trabaja.
5. `turn_done` cierra el turno, con su coste.
6. Al salir de la pantalla, la selección se limpia.

### Lo que se ve de la conversación

**No es un terminal.** No llega el flujo del PTY: llegan bloques ya interpretados
del transcript.

```jsonc
{ "terminalId": "…", "kind": "agent_message" | "action" | "turn_done",
  "text": "…", "files": ["src/x.rs"], "at": "…" }
```

Y el histórico por bloques tipados: `you` · `think` · `edit` · `cmd` · `agent` ·
`sum` · `rev` · `out`.

**No hay scrollback de una terminal con sesión, y no lo habrá**: el volcado
crudo puede llevar claves que el agente imprimió, y el móvil no necesita
verlo para dirigir la conversación. Lo crudo se queda en el Mac.

**Excepción: una terminal sin sesión (`kind='project'`, un PTY real).** Ahí no
hay bloques que interpretar — es la shell cruda que de verdad se usa día a
día. El móvil escribe con `write_terminal` (fila de abajo) y ve un tail
acotado y redactado por patrones de secreto conocidos (~32 KB, best-effort,
no garantía) vía el evento `terminal-tail` — ver `MOBILE_BRIDGE_TRD.md`
§6 y §12.4. Reconstruido por un emulador de terminal real (`vt100`), no un
stripper de ANSI a mano, así que un párrafo que Claude Code redibuja al
reajustar el ancho de línea aparece una sola vez, no repetido — y cada
`terminal-tail` manda el transcript completo de nuevo, no un incremento a
concatenar. Sigue sin ser scrollback completo ni persistente, y esta
excepción no aplica a terminales con sesión.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Escribir en la terminal | `write_terminal` | ✅ | steer |
| Ver el tail de una terminal sin sesión, al abrir la pantalla | `get_terminal_tail` | ✅ | watch |
| Seguir viéndolo en vivo | `terminal-tail` (evento) | ✅ | watch |
| Mandar un mensaje a la sesión | `send_message` | ✅ | steer |
| Parar el agente | `stop_session` | ✅ | steer |
| Ver la conversación | `get_session_view`, `list_session_blocks` | ✅ | watch |
| Marcar dónde estoy mirando | `bridge_select` | ✅ | watch |
| Ficheros que tocó el turno | `list_file_edits` | ➕ | watch |
| Retomar una conversación anterior | `list_resumable_conversations`, `set_session_resume_target` | ➕ | steer |
| Borrar una sesión | `delete_session` | ➕ | launch |

---

## 5 · Permisos y modo yolo

Lo pediste explícitamente y **hoy no se puede desde el móvil**.

### Flujo: contestar un permiso

1. El agente pide usar una herramienta y se queda esperando.
2. Llega al teléfono con `toolName` y `toolInput`.
3. Se aprueba o se rechaza.
4. El agente sigue.

Es el momento más importante de la app: alguien fuera de casa tiene que entender
qué se le pide y decidir en dos segundos.

### Flujo: yolo

Cuatro modos: `plan` (solo analiza) · `ask_edits` · `auto_edit` · `yolo` (no
vuelve a preguntar).

1. Antes de irse, o ya en la calle, se pone la terminal en yolo.
2. El agente deja de pedir permiso.
3. Se puede volver a bajar el modo en cualquier momento.

Se puede fijar por defecto para las terminales nuevas de ese Mac, que es lo que
tiene sentido antes de salir.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Contestar un permiso | `respond_to_permission_request` | ✅ | steer |
| Modo por defecto del Mac | `get_default_terminal_mode`, `set_default_terminal_mode` | ➕ | steer |
| Modo de una sesión | `update_session_settings` | ➕ | steer |

---

## 6 · La cola de trabajo

Aquí hay una **incoherencia real** en el contrato de hoy: el puente reenvía
`queue-updated` al teléfono, pero no expone ni un comando de cola. Le cuenta al
móvil que cambió algo que el móvil no puede ni ver ni tocar.

### Flujo

1. Se encola un prompt para un proyecto, con su rama y su cuenta.
2. Cuando hay hueco (cuenta libre, cuota disponible), Hangar lo lanza.
3. `queue-updated` avisa en cada cambio.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Ver la cola | `list_queued_runs` | ➕ | watch |
| Encolar | `enqueue_run` | ➕ | launch |
| Cancelar | `cancel_queued_run` | ➕ | steer |

---

## 7 · Fan-out: un prompt a varias terminales

Mandar lo mismo a varios agentes y comparar. Es una de las razones de existir de
Hangar y hoy el móvil no la alcanza.

### Flujo

1. Se elige un prompt y varias terminales (o un proyecto entero).
2. Se manda a todas a la vez.
3. Se siguen los turnos en paralelo desde la lista.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| A terminales concretas | `fan_out_prompt` | ➕ | launch |
| A un proyecto | `project_fan_out` | ➕ | launch |

---

## 8 · Tareas: de un issue a un worktree trabajando

### Flujo

1. El teléfono lista las fuentes configuradas (GitHub de tal cuenta, ClickUp de
   tal workspace).
2. Se elige la fuente y el ámbito: repo, lista, tablero.
3. Llegan las tareas asignadas, filtrables por proyecto y por estado.
4. Se escoge una.
5. El Mac crea el worktree, la rama con nombre derivado de la tarea, escribe el
   cuerpo de la tarea en `.hangar/task-<clave>.md`, y abre la terminal con ese
   contexto ya cargado.
6. El teléfono aterriza en esa terminal.

Ese paso 5 es el más valioso de la app entera: desde el sofá, un issue se
convierte en un agente trabajando.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Fuentes configuradas | `list_task_sources`, `list_task_source_links` | ✅ | watch |
| Tareas | `list_source_tasks`, `list_project_tasks` | ✅ | watch |
| **Elegir tablero o lista** | `list_source_scopes` | ➕ | watch |
| Abrir worktrees de tareas | `open_task_worktrees` | ✅ | launch |
| Configurar una fuente (tokens) | `save_task_source`, `link_task_source` | ⛔ lleva credenciales | — |

`list_source_scopes` faltando es otra incoherencia: se pueden pedir las tareas
pero no saber de qué tableros elegir.

---

## 9 · Git: ver qué se ha hecho

### Flujo

1. Desde una terminal o un proyecto, se abre su estado de git.
2. Se ven ramas, worktrees y sus cambios.
3. Se abre un fichero y se lee el diff.
4. Se recorre el historial de commits.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Estado general | `get_git_overview` | ✅ | watch |
| Cambios de un worktree | `get_worktree_changes` | ✅ | watch |
| Diff de un fichero | `get_worktree_file_diff` | ✅ | watch |
| Ramas y commits | `list_branches`, `list_commits` | ✅ | watch |
| Qué hay dentro de un commit | `get_commit_files`, `get_commit_file_diff` | ✅ | watch |
| Leer un fichero del proyecto | `read_file`, `list_files` | ➕ no existe todavía | watch |
| `fetch` / `pull` / `push` | `git_fetch`, `git_pull`, `git_push` | ✅ | launch |
| Cambiar de rama | `checkout_branch` | ✅ | launch |
| Crear un worktree | `create_worktree_for_project` | ✅ | launch |

**Móvil, `TDD-4-a-8-movil.md` Fase 7c**: ya consume `get_git_overview`/
`get_worktree_changes`/`get_worktree_file_diff` — cambios por worktree y su
diff, línea por línea, calculado del lado del cliente. Todavía no consume
ramas/historial de commits (`list_branches`/`list_commits`/
`get_commit_files`/`get_commit_file_diff` — pantalla separada del
escritorio, "qué pasó" en vez de "qué cambió ahora") ni ninguna acción de
escritura (`fetch`/`pull`/`push`/checkout/crear worktree) — recordatorio de
más abajo: `push` sigue sin confirmación explícita en ningún cliente móvil,
porque todavía no se construyó.

**Nota sobre `push`**: exponerlo significa que el teléfono puede publicar código.
Es defendible en nivel `launch`, pero conviene que la app pida confirmación
explícita — es la acción menos reversible de toda la lista.

---

## 10 · Pull requests y revisiones

### Flujo: revisar un PR desde el móvil

1. Llega `pr-comments-found`: hay comentarios nuevos.
2. Se abre la bandeja de revisión.
3. Se manda el PR a revisar: el Mac crea el worktree de revisión y abre un agente
   con los comentarios cargados.
4. Se sigue el turno y se contesta desde el teléfono.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Bandeja de revisión | `list_pr_review_inbox` | ✅ | watch |
| Buscar comentarios ahora | `check_pr_comments` | ✅ | steer |
| Abrir terminal de revisión | `open_pr_review_terminal` | ✅ | launch |
| Mandar comentarios a una terminal | `send_pr_comments_to_terminal` | ✅ | launch |
| Descartar comentarios | `dismiss_pr_comments` | ✅ | steer |
| Ver un PR | `get_pull_request` | ✅ | watch |
| Crear un PR | `preview_pull_request`, `create_pull_request` | ✅ expuesto, sin consumir desde el móvil todavía | launch |
| Revisar todos los PRs abiertos | `check_all_open_prs_now` | ✅ | steer |

`dismiss_pr_comments` sí vacía la bandeja: limpia el comentario de
`list_pr_review_inbox` sin mandar nada a un agente (para feedback que no
necesita acción, un "LGTM" liso).

---

## 11 · Cuentas, cuotas y rotación

### Flujo

1. Se ve cuánta cuota le queda a cada cuenta.
2. Si una está agotada, se reabre la terminal con otra cuenta **manteniendo la
   conversación** — Hangar es dueño del id de sesión, así que el hilo no se pierde.
3. Se puede refrescar la cuota a mano.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Cuentas y su estado | `list_accounts`, `list_account_auth` | ✅ | watch |
| Cuotas | `list_account_quotas` | ✅ | watch |
| Refrescar cuotas | `refresh_all_quotas`, `fetch_real_usage` | ➕ | steer |
| Rotación del proyecto | `list_project_account_rotation`, `get_rotation_policy` | ➕ | watch |
| Cambiar la política de rotación | `set_rotation_policy` | ➕ | steer |
| Crear / borrar cuentas, login | `create_account`, `begin_login`, `validate_api_key` | ⛔ credenciales | — |

**Regla que hay que escribir en el TRD**: hoy solo excluye los comandos con
`Channel`. Falta decir que **nada que toque credenciales sale al puente** —
`create_account`, `begin_login`, `send_login_input`, `validate_api_key`,
`store_refresh_token`, `load_refresh_token`, `set_account_config_dir`,
`set_update_access`. Un teléfono robado no puede ser una puerta a las cuentas.

---

## 12 · Gasto

### Flujo

1. Se ve el gasto por cuenta y por proyecto.
2. Se mira el coste de un turno concreto dentro de una conversación.
3. Se comprueba si un proyecto va a pasarse del presupuesto.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Gasto por cuenta y proyecto | `list_account_spend`, `list_project_spend` | ✅ | watch |
| Coste por turno | `list_turn_usage`, `get_account_usage_summary` | ✅ | watch |
| Presupuestos | `list_project_budgets` | ✅ | watch |
| Cambiar un presupuesto o tope | `set_project_budget`, `set_account_monthly_cap` | ➕ | steer |

---

## 13 · Memoria del proyecto (CLAUDE.md)

Editar las instrucciones que el agente lee. Discutible en móvil: escribir markdown
largo en un teléfono es incómodo. Pero **leerlo** sí importa — «¿por qué el agente
hizo eso?» se contesta mirando su memoria.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Ver la memoria | `list_memory_groups`, `list_group_memory_files`, `get_memory_file` | ➕ | watch |
| Editarla | `update_memory_file_content`, `create_memory_file` | ➕ | steer |

Recomendación: solo lectura en la v1.

---

## 14 · Árboles de agentes

Encadenar agentes en un flujo. Es la funcionalidad más compleja de Hangar y la
menos apta para una pantalla de teléfono.

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Ver los árboles y su avance | `list_agent_nodes` | ➕ | watch |
| Lanzar uno ya definido | `run_agent_tree` | ➕ | launch |
| Crear o editar la estructura | `create_agent_node`, `update_agent_node` | ⛔ en móvil no | — |

Lanzar sí, diseñar no: definir un árbol es trabajo de escritorio.

---

## 15 · Voz

Todo en el teléfono, nada nuevo en el escritorio.

### Flujo

1. Se mantiene pulsado el botón de voz.
2. Se dicta el prompt.
3. **Se enseña la transcripción y se confirma antes de mandar.** Un prompt mal
   transcrito le cuesta dinero y trabajo al usuario.
4. Al confirmar, se manda por `write_terminal` o `send_message` como cualquier otro.

No hay comando nuevo: la voz es una forma de escribir. Reconocimiento nativo del
sistema (`speech_to_text`), sin servicio externo — encaja con «no quiero mi propia
infra» y evita mandar audio a ninguna parte.

---

## 16 · Notificaciones

Lo que hace que la app valga la pena: enterarse **sin** tener la app abierta.

Hoy no existe. Con la app cerrada no llega nada, porque el puente vive en el
webview de Hangar y el teléfono no mantiene conexión de fondo.

### Flujo previsto (`MOBILE_BRIDGE_TRD.md` §8)

1. Hangar detecta un turno terminado o un permiso pendiente.
2. Llama a una Edge Function de Supabase.
3. La Edge Function manda el push por FCM/APNs.
4. El teléfono abre la app en esa terminal.

La credencial de FCM vive en la Edge Function, no en el escritorio. Es
infraestructura, pero no *propia*: es parte del Supabase que ya se usa.

Hasta que exista, el diseño no puede dar por hecho que el usuario se enterará.

---

## 17 · Ajustes del Mac desde el móvil

| Funcionalidad | Comando | Estado | Nivel |
|---|---|---|---|
| Nombre del Mac | `set_machine_name` | ➕ | steer |
| Modo por defecto de las terminales | `set_default_terminal_mode` | ➕ | steer |
| Ajustes generales | `get_app_settings`, `set_app_settings` | ⛔ mezcla cosas sensibles | — |

`set_app_settings` guarda un revoltijo que incluye rutas y tokens; exponerlo
entero sería un agujero. Si hace falta algún ajuste concreto, se saca uno a uno.

---

## Resumen: qué hay que exponer para que «manejar Hangar» sea cierto

En orden de cuánto duele no tenerlo:

| # | Comandos | Por qué |
|---|---|---|
| 1 | `open_terminal`, `close_terminal` | Sin esto el móvil no empieza trabajo, solo lo continúa |
| 2 | `get_default_terminal_mode`, `set_default_terminal_mode`, `update_session_settings` | El yolo que pediste |
| 3 | `list_queued_runs`, `enqueue_run`, `cancel_queued_run` | Ya se reenvía `queue-updated` sin nada que verlo |
| 4 | `get_commit_files`, `get_commit_file_diff` | Se listan commits que no se pueden abrir |
| 5 | `list_source_scopes` | Se piden tareas sin poder elegir el tablero |
| 6 | `dismiss_pr_comments` | La bandeja no se vacía nunca |
| 7 | `fan_out_prompt`, `project_fan_out` | Una de las razones de existir de Hangar |
| 8 | `git_push`, `git_pull`, `git_fetch`, `checkout_branch`, `create_worktree_for_project` | Cerrar el ciclo sin abrir el portátil |
| 9 | `preview_pull_request`, `create_pull_request`, `get_pull_request` | Terminar el trabajo, no solo hacerlo |
| 10 | `suggest_terminal_account`, `reopen_terminal_with_account`, `refresh_all_quotas` | Cuando una cuenta se agota estando fuera |
| 11 | `list_resumable_conversations`, `set_session_resume_target`, `list_file_edits` | Retomar y auditar |
| 12 | `list_agent_nodes`, `run_agent_tree` | Lanzar un árbol ya definido |

**El coste de cada línea de esa tabla es una línea en `allowlist.json`.** Eso era
justo el objetivo de la arquitectura: que añadir una función al móvil no obligue a
tocar el escritorio. La única excepción es `open_terminal`, que necesita decidir
qué `cols`/`rows` manda un cliente que no dibuja terminal.

Esta tabla quedó vieja: a esta altura **los 12 puntos ya están en
`allowlist.json`** (verificado leyéndolo entero). Lo que de verdad falta rastrear
fase a fase es qué de esto **consume ya la app móvil** — eso vive en
`TDD-4-a-8-movil.md`, no acá.

## Y lo que no debe salir del Mac, dicho de una vez

- **Credenciales**: crear cuentas, logins, validar claves, tokens de refresco,
  directorios de configuración, acceso a actualizaciones.
- **Streaming de PTY**: `attach_terminal`, `detach_terminal`, `resize_terminal`,
  `restore_terminals`. El móvil no dibuja un terminal.
- **Volcado crudo del terminal**: puede contener lo que el agente imprimió, claves
  incluidas.
- **Ajustes en bloque**: `set_app_settings` mezcla lo inocuo con lo sensible.
- **Editar la estructura de un árbol de agentes**: cabe en una pantalla grande, no
  en un teléfono.
