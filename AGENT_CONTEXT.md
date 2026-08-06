# Lo que Hangar le dice al agente antes de que escribas

Cómo llegan al CLI el idioma y las skills: dónde se configuran, dónde aterrizan
en disco, y con qué bandera entran.

**Estado**: implementado. Verificado **ejecutando `claude`**; `kimi` y `grok`
heredan por ir sobre el mismo binario, que es una inferencia razonable y no una
comprobación. Codex recibe el idioma pero todavía no las skills (§7).

---

## 1 · El problema que resuelve

Hangar tenía ajustes y **ninguno llegaba nunca al agente**. El síntoma diario
era pedir algo en la consola, recibirlo en inglés, y tener que pedir que se
reescribiera. Cada vez.

El código ya lo documentaba, en `SettingsTabs.tsx`, explicando por qué la
pantalla de memoria compartida llevaba meses oculta:

> *nothing ever feeds that content to an agent — no CLAUDE.md is written, no
> system prompt is appended*

Esto es ese canal. El idioma fue lo primero que pasó por él; las skills, lo
segundo.

### La restricción que decide el diseño

**Una terminal interactiva no tiene prompt al que anteponer nada.** Tú tecleas
en vivo contra el PTY, así que no existe un primer mensaje que Hangar pueda
decorar. La idea intuitiva —«metemos el idioma delante del prompt»— no puede
funcionar justo en el caso que motivó todo.

La única puerta es una **bandera al arrancar el CLI**.

---

## 2 · El recorrido completo

```
   LO QUE TÚ DECIDES             LO QUE HANGAR ESCRIBE            CÓMO ENTRA
                                 (siempre en su app data)

┌──────────────────────────┐   ┌─────────────────────────────┐   ┌───────────────────────┐
│ Settings → general       │   │ <app_data>/                 │   │ claude                │
│   idioma conversación ───┼─┐ │                             │   │                       │
│   idioma código          │ │ │   agent-instructions/       │   │  --permission-mode …  │
├──────────────────────────┤ ├▶│     <project_id>.md ────────┼──▶│  --append-system-     │
│ Settings → projects      │ │ │                             │   │    prompt-file …      │
│   override por proyecto ─┼─┘ │   skills/       (biblioteca)│   │                       │
│   skills marcadas ───────┼─┐ │     <nombre>/SKILL.md       │   │                       │
├──────────────────────────┤ │ │                             │   │                       │
│ Settings → skills        │ ├▶│   projects/<project_id>/    │   │                       │
│   la biblioteca ─────────┼─┘ │     .claude/skills/         │   │                       │
└──────────────────────────┘   │       <nombre> ──▶ enlace ──┼──▶│  --add-dir …          │
                               └─────────────────────────────┘   └───────────────────────┘
```

Nada de esto toca el repositorio del usuario. Es la regla dura que el TRD de
Graphify ya fijaba (§3): *«un usuario que desinstala Hangar no debe encontrar
residuos en su repo»*.

---

## 3 · Cuándo ocurre

Todo se arma **al abrir la terminal**, no al guardar el ajuste.

```
  abres una terminal / lanzas un turno
                │
                ▼
  ┌─────────────────────────────────────────────────────────┐
  │  open_terminal()                        terminals.rs    │
  │                                                         │
  │   1 ─ ¿de qué proyecto es esta terminal?                │
  │        resolve_target_with_project()                    │
  │                                                         │
  │   2 ─ resolver idiomas y escribir el fichero            │
  │        preferences::instruction_file()                  │
  │                                                         │
  │   3 ─ enlazar las skills marcadas                       │
  │        skills::skills_dir()                             │
  │                                                         │
  │   4 ─ montar la línea de comando                        │
  │        account_command_line_full()                      │
  └─────────────────────────────────────────────────────────┘
                │
                ▼
     CLAUDE_CONFIG_DIR=… claude --permission-mode … \
        --append-system-prompt-file … --add-dir …
```

**Por qué en cada arranque y no al guardar.** Un fichero escrito al guardar hay
que invalidarlo cuando se renombra un proyecto, cuando cambia el valor global
bajo un proyecto que lo hereda, cuando se borra una skill o se desmarca.
Armándolo al abrir la terminal, ninguna de esas preguntas llega a existir: el
fichero no puede ser más viejo que el proceso que lo lee.

**Consecuencia a saber**: se aplica a lo que **empiece a partir de ahora**. Una
terminal ya abierta sigue como estaba, porque las preferencias se le pasan al
CLI al arrancarlo y cambiarlas después sería teclearle a ciegas. Es la misma
regla que ya tenía el modo de permisos.

---

## 4 · Cómo se resuelve un idioma

Global por defecto, y cada proyecto puede sobreescribirlo. **Campo a campo**, no
todo o nada.

```
              para cada campo (conversación, código)
                              │
                 ¿el proyecto tiene valor propio?
                              │
              ┌───────────────┴───────────────┐
             sí                               no
              │                                │
              ▼                                ▼
      ┌───────────────┐               ¿hay valor global?
      │ el del        │                        │
      │ proyecto      │            ┌───────────┴───────────┐
      └───────────────┘           sí                       no
                                   │                        │
                                   ▼                        ▼
                           ┌───────────────┐    ┌────────────────────────┐
                           │ el de General │    │  NINGUNA BANDERA       │
                           └───────────────┘    │  (ni vacía, ni con un  │
                                                │   fichero vacío)       │
                                                └────────────────────────┘
```

Campo a campo es lo que evita el fallo obvio: sobreescribir solo el idioma del
código no puede tirar por el camino el idioma de conversación global.

En la base de datos, `NULL` significa **heredar**. Por eso no hay un booleano
«usar el global» al lado de cada columna: una columna solo puede estar en uno de
los dos estados, y un segundo campo podría contradecirla.

**Sin nada configurado no se pasa ninguna bandera.** Es deliberado: la línea de
comando es lo que el picker previsualiza, y una bandera que no dice nada cuesta
una línea de algo que el usuario lee para decidir qué va a ejecutarse.

---

## 5 · Dónde vive todo

```
<app_data>/                          ← NUNCA el repositorio del usuario
│
├── hangar.db
│     ├── app_settings               ← idiomas globales
│     ├── project_preferences        ← override por proyecto (NULL = heredar)
│     └── project_skills             ← qué skills activa cada proyecto
│
├── agent-instructions/
│   └── <project_id>.md              ← reescrito en cada arranque
│
├── skills/                          ← LA BIBLIOTECA (carpetas de verdad)
│   ├── deploy-check/
│   │   └── SKILL.md
│   └── add-educational-comments/
│       ├── SKILL.md                 ← instalada con `gh skill install --dir`
│       └── scripts/                 ← el estándar permite traer ficheros
│
└── projects/
    └── <project_id>/                ← ESTO es lo que recibe --add-dir
        └── .claude/
            └── skills/
                └── deploy-check ──▶ ../../../../skills/deploy-check
```

Tres decisiones que conviene no deshacer sin querer:

**La biblioteca está en disco, no en la base de datos.** El estándar dice que una
skill es una **carpeta**, y puede traer `scripts/`, `references/` y `assets/`.
Guardar solo el markdown en una columna cerraría esa puerta el primer día.

**Enlaces simbólicos, no copias.** Editas una skill una vez y cambia en todos los
proyectos que la usan. Con copias, cada proyecto sería una bifurcación
desactualizada de la biblioteca.

**`--add-dir` apunta a `projects/<id>/`, nunca a la raíz del app data.** Esa
bandera concede **acceso a ficheros** además de cargar skills, así que apuntarla
a la raíz le entregaría al agente `hangar.db`. Hay un test que lo vigila.

---

## 6 · La línea de comando resultante

Esto es lo que se ejecuta al abrir una terminal, pieza a pieza:

```
  CLAUDE_CONFIG_DIR=/…/cuenta-a  ───  qué cuenta paga — aislamiento por cuenta
  claude                         ───  el binario del proveedor
  --permission-mode acceptEdits  ───  permission_args()    providers/mod.rs
  --append-system-prompt-file …  ───  instruction_args()   providers/mod.rs
  --add-dir /…/projects/p1       ───  skills_args()        providers/mod.rs
  --session-id …                 ───  la conversación que Hangar puede retomar
  'prompt'                       ───  el primer mensaje, si lo hay
```

Las tres últimas líneas de la columna derecha que apuntan a `providers/mod.rs`
son la parte que importa: **`permission_args`, `instruction_args` y
`skills_args` son hermanas a propósito** — misma forma, mismo fichero, y cada
bandera nombrada en un solo sitio para los cinco proveedores.

Y esta línea no es un detalle interno: es exactamente la que el picker
previsualiza antes de abrir la terminal, *«para que lo que se previsualiza sea
literalmente lo que corre»*. Por eso una bandera que no dice nada no se pasa —
costaría una línea de algo que el usuario lee para decidir.

---

## 7 · Qué recibe cada CLI

| | `claude` · `kimi` · `grok` | `codex` | `gemini` |
|---|---|---|---|
| **Idioma** | `--append-system-prompt-file <ruta>` | `-c developer_instructions=<texto>` | — |
| **Skills** | `--add-dir <dir>` | *(pendiente)* | — |
| **Verificado** | ✅ `claude`, ejecutándolo · `kimi`/`grok` heredados, sin probar | ⚠️ acepta la config; falta probar el comportamiento | ❌ sin instalar |

Notas que explican las casillas raras:

**Kimi y Grok** van por el binario y las convenciones de Claude Code, así que
heredan todo lo suyo sin adaptador.

**Codex necesita el texto en una sola línea.** Ese valor acaba dentro de una
cadena de shell, y un salto de línea abre el prompt de continuación de zsh. Por
eso existe `render_instructions_inline()` junto al render normal, y por eso hay
un test que falla si alguien mete un `\n`.

**Codex y `model_instructions_file`: no.** La documentación lo describe como
*replacement* de las instrucciones internas del CLI. Usarlo cambiaría el idioma
a cambio de perder el comportamiento base de Codex. `developer_instructions` es
aditivo, que es lo que hace falta.

**Las skills de Codex están pendientes** porque busca en `.agents/skills` y toma
rutas por skill en vez de un directorio padre. No se ha cableado sin poder
verificarlo contra una ejecución real: mandar un `-c` sin probar arriesga el
`error: unexpected argument` con el que empezó el soporte de Codex en este repo.
Mientras tanto su terminal se comporta exactamente igual que antes.

---

## 8 · Las skills del repositorio no se tocan

```
   ┌─────────────────────────────┐
   │ el repo trae .claude/skills │──┐
   └─────────────────────────────┘  │      el CLI ve
                                    ├────▶  las dos cosas
   ┌─────────────────────────────┐  │      (se SUMAN)
   │ las marcadas en Hangar      │──┘
   └─────────────────────────────┘
```

Si el proyecto trae sus propias skills, el CLI las carga solo por estar en el
directorio de trabajo. La selección de Hangar **se suma**, no reemplaza. No hay
nada que implementar para eso; sí hay que decirlo en la interfaz, porque de otro
modo parece que una cosa pisa a la otra.

---

## 9 · Cómo se verificó

No por leer la documentación. **El método del canario**: una skill que devuelve
un token único, y la misma pregunta con y sin la bandera.

```
   SKILL.md con un token único
   "responde exactamente DEPLOY-K92X-READY"
                  │
        ┌─────────┴─────────┐
        ▼                   ▼
  con --add-dir        sin --add-dir
        │                   │
        ▼                   ▼
  ┌──────────────┐   ┌──────────────────────┐
  │ DEPLOY-K92X- │   │ «no tengo eso; ¿lo   │
  │ READY     ✓  │   │  buscamos en docs?»✓ │
  └──────────────┘   └──────────────────────┘
```

El control importa tanto como la prueba: sin él, un agente que ya sabía la
respuesta por otro camino habría parecido un éxito.

Lo mismo para el idioma: la misma pregunta **en inglés** contestada en español
con `--append-system-prompt-file`, y en inglés sin ella.

Y el enlace simbólico se probó siendo un enlace, no una copia — que es lo que
sostiene toda la §5.

---

## 10 · Instalar skills de fuera

**No hay instalador propio, y no debería haberlo.** `gh skill` existe, y Hangar
ya depende de `gh` para PRs e issues:

```sh
gh skill install <owner>/<repo> <skill> --dir <app_data>/skills
```

Trae búsqueda, previsualización, versiones fijadas (`@v1.2.0` o `--pin`),
actualización, e inyecta metadatos de origen en el frontmatter que son lo que
permite a `gh skill update` detectar cambios después.

### El riesgo, que es real

El propio `gh` avisa al terminar:

> ⚠ *Skills may contain prompt injections or malicious scripts.*

No es una advertencia genérica. Una skill puede declarar esto:

```yaml
allowed-tools: Bash(${CLAUDE_SKILL_DIR}/scripts/render.sh *)
```

Eso **pre-autoriza ejecutar un script que la propia skill trae**, sin diálogo. En
Hangar se amplifica por dos cosas suyas: en modo **yolo** ya no queda ningún
permiso que aprobar, y una skill marcada en un proyecto entra en **todas** sus
terminales a la vez.

### El frontmatter que Hangar no entiende, se conserva

El estándar tiene más campos de los que edita esta interfaz: `allowed-tools`,
`license`, `compatibility` y el bloque `metadata` que inyecta `gh`.

```
   SKILL.md instalada                    guardas desde Hangar
   ┌────────────────────────┐            ┌────────────────────────┐
   │ name          ─────────┼───────────▶│ name        (editado)  │
   │ description   ─────────┼───────────▶│ description (editado)  │
   │ metadata:              │            │                        │
   │   github-repo ─────────┼───────────▶│ metadata:   (INTACTO)  │
   │   github-tree-sha      │            │   …                    │
   │ allowed-tools ─────────┼───────────▶│ allowed-tools (INTACTO)│
   │ license       ─────────┼───────────▶│ license       (INTACTO)│
   └────────────────────────┘            └────────────────────────┘
```

La preservación ocurre **en el backend, releyendo el fichero existente**, no
pidiéndole al frontend que transporte esos campos. Así no se pueden perder
aunque la interfaz no los conozca. Sin eso, guardar una skill instalada rompía
`gh skill update` en silencio y borraba pre-autorizaciones de las que la skill
dependía.

---

## 11 · Mapa del código

| Qué | Dónde |
|---|---|
| Resolver idiomas, renderizar, escribir el fichero | `src-tauri/src/commands/preferences.rs` |
| Biblioteca de skills, selección, enlaces | `src-tauri/src/commands/skills.rs` |
| Las tres funciones de banderas | `src-tauri/src/providers/mod.rs` |
| Punto de inserción — terminales (los 5 proveedores) | `terminals.rs` · `account_command_line_full()` |
| Punto de inserción — turnos headless (solo Claude) | `providers/claude.rs` · `build_turn_args()` |
| Tablas | `db/mod.rs` migraciones 15 y 16 |
| Ajustes globales | `src/screens/GeneralSettings.tsx` |
| Override y skills por proyecto | `src/screens/ProjectSettings.tsx` |
| La biblioteca | `src/screens/SkillSettings.tsx` |

Las migraciones **no van en `schema.sql`**: ese fichero se aplica a bases
existentes *antes* de que corra ninguna migración, así que declarar ahí algo
nuevo y referenciarlo desde una migración tumba el arranque en instalaciones
antiguas. El aviso está en `schema.sql` y en el TRD de Graphify §5.4.

---

## 12 · Lo que esto no hace

- **Hooks.** Son otra cosa y son más peligrosos: ejecutan código tuyo en cada
  acción del agente. La diferencia con todo lo de aquí es que **una instrucción
  el modelo la puede olvidar cuando el contexto se llena; un hook siempre se
  ejecuta**. Merecen su propio diseño de seguridad.
- **Permisos afinados** (`allow`/`deny` por comando). Se solapan con los cuatro
  modos que Hangar ya pasa por bandera, y dos fuentes de verdad para lo mismo se
  contradicen el día que alguien cambie una.
- **MCP.** El TRD de Graphify ya reserva `--mcp-config` para inyectar su grafo.
- **Conexiones entre proyectos.** Aparcado a propósito: se repiensa dentro de
  Graphify, que ya lo tenía medio previsto.
- **Desbloquear la memoria compartida.** Ahora sería barato —el canal ya
  existe— pero es una decisión aparte.
