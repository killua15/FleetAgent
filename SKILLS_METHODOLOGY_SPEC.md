# Spec — Metodología por proyecto (Superpowers) e Impeccable

> Estado: **propuesta, pendiente de aprobación** (2026-10-01). Sale de revisar el "Top 10 OpenCode
> Skill Repos" (Superpowers, Ponytail, UI/UX Pro Max, Graphify, Caveman, Addy Osmani, Understand
> Anything, Awesome Claude Skills, Archify, Impeccable).
>
> - **Graphify** y **Archify** ya están integrados en Hangar.
> - Este spec cubre **Superpowers** e **Impeccable**, y arregla el listado de skills que hoy deja
>   fuera a Caveman y a Awesome Claude Skills.

## Problem

- **Superpowers** (`obra/superpowers`, 15 skills) es una metodología completa:
  - brainstorming → plan → ejecución con TDD;
  - `systematic-debugging` y `verification-before-completion`;
  - agentes en paralelo.

  Encaja con agentes que trabajan solos en un worktree, pero **choca** con el flujo de *spec* +
  plan mode del `CLAUDE.md` global. Por eso no puede activarse en todas partes.
- **No es un paquete de skills sueltas: es un plugin de Claude Code.** Trae un *hook*
  `SessionStart` que inyecta `using-superpowers` en cada sesión, y eso es lo que hace que el agente
  siga el método. Hangar hoy instala skills con `gh skill`, que copia los `SKILL.md` pero **pierde
  ese hook**, así que Superpowers quedaría a medias.
- **Impeccable** (`pbakaus/impeccable`) es una sola skill de diseño de UI, sin hooks. Encaja tal
  cual en las skills por proyecto que ya existen.
- **Fallo actual en `skill_sources.rs::parse_skill_listing`:** descarta las líneas cuyo nombre lleva
  prefijo de `gh skill` (`[plugins] caveman/caveman`, `[root] brand-guidelines`). Con eso, **Caveman
  y Awesome Claude Skills aparecen sin ninguna skill**. Además, Impeccable sale **duplicada**
  (`gh` la encuentra en `.claude/skills` y en `.agents/skills`).

### Verificado

| Hecho | Cómo |
|---|---|
| Superpowers trae `hooks/session-start` que lee `skills/using-superpowers/SKILL.md` | Árbol y fichero del repo vía `gh api` |
| Marketplace `superpowers-dev`, plugin `superpowers`, última versión `v6.4.2` | `.claude-plugin/marketplace.json`; `gh skill install obra/superpowers` |
| Claude Code acepta `enabledPlugins` y `extraKnownMarketplaces` en settings, con precedencia **user < project < local < flag < policy** | Descripciones del esquema de settings en el binario 2.1.286 |
| El `--settings` que Hangar ya pasa a cada terminal Claude (`auto_continue::write_terminal_settings`) es *flagSettings* | Desde la 0.8.9 |
| Los 8 repos del top se listan con `gh skill install <repo>` sin instalar nada | Ejecutado |

## Goals / Non-goals

**Goals**
- Elegir la **metodología por proyecto**: *Spec-driven* (por defecto) o *Superpowers*. Nunca las dos
  en la misma terminal.
- Activar Superpowers como **plugin completo**, con su hook, solo en las terminales de los proyectos
  marcados y **sin escribir nada en el repo del proyecto** ni en la configuración global del usuario.
- Impeccable en los proyectos con interfaz, con el mecanismo de skills por proyecto que ya existe.
- Que el listado de fuentes de skills muestre bien los repos con formato `plugins/` o skills en la
  raíz, y sin duplicados.

**Non-goals**
- Instalar Superpowers en Fleet (Hangar). Allí se trabaja con el flujo de spec y no se cambia.
- Caveman / modo "ahorro de tokens". Va en un spec aparte; aquí solo se desbloquea su listado.
- Understand-Anything (compite con Graphify), Ponytail como skill fija, y UI/UX Pro Max (se solapa
  con Impeccable).
- Gestionar plugins de Claude Code en general (marketplaces arbitrarios). Solo Superpowers, por ahora.

## Approach

### Fase 0 — Impeccable (sin código)
1. Ajustes → fuentes de skills → añadir `pbakaus/impeccable` → instalar `impeccable`.
2. Activarla en **Fleet**, y en Live-Strategy-Tracker / WinRadar si tienen frontend.

### Fase 1 — Arreglar el listado de skills (`src-tauri/src/commands/skill_sources.rs`)
- `parse_skill_listing`:
  - aceptar el prefijo `[<origen>] ` y quedarse con el nombre que `gh skill install <repo> <skill>`
    espera. Hay que verificar si es `caveman/caveman` o `caveman` ejecutándolo antes de fijarlo;
  - ignorar las líneas `Note:` y `Using ref`;
  - **deduplicar por nombre**.
- Tests con la salida real capturada de Caveman, Awesome Claude Skills e Impeccable.

### Fase 2 — Metodología por proyecto
- **Datos:** `projects.methodology TEXT NOT NULL DEFAULT 'spec'` (`'spec' | 'superpowers'`), con
  migración aditiva (patrón de `db/mod.rs`, versión siguiente).
- **UI:** en Ajustes del proyecto, un selector *Metodología: Spec-driven | Superpowers*, con una
  línea que explica qué cambia. En la cabecera de cada terminal de un proyecto Superpowers, un
  distintivo **SP**.
- **Al abrir una terminal Claude** de un proyecto `superpowers`, `write_terminal_settings` añade al
  `--settings` de esa terminal:
  ```json
  {
    "extraKnownMarketplaces": {
      "superpowers-dev": { "source": { "source": "github", "repo": "obra/superpowers", "ref": "v6.4.2" } }
    },
    "enabledPlugins": { "superpowers@superpowers-dev": true }
  }
  ```
  - La versión queda **fijada**, para que una actualización del repo no cambie el comportamiento sin
    aviso. Subirla es un cambio deliberado.
  - La forma exacta de `source`/`ref` se verifica contra el esquema del CLI antes de implementarla.
- **Conflicto con el `CLAUDE.md` global:** la instrucción por proyecto que Hangar ya genera
  (`preferences::instruction_file`) añade en esos proyectos: *"En este proyecto el flujo es
  Superpowers (brainstorming → writing-plans → executing-plans); la plantilla de spec del CLAUDE.md
  global no aplica."*
- **Plan mode:** en proyectos Superpowers, la terminal arranca en modo normal y deja el control al
  plan de Superpowers. Así no se pide aprobación dos veces. Pendiente de confirmar, ver Open
  questions.
- **Cambio con la terminal abierta:** como con el ⏰, `--settings` solo se lee al arrancar. Cambiar la
  metodología relanza las terminales del proyecto con `--resume` (reutiliza `relaunch_terminal`),
  y si alguna está a mitad de turno, al terminarlo.

### Fase 3 — Piloto
- **simulation-server**: una tarea real de ClickUp con Superpowers y otra comparable con el flujo de
  spec.
- Comparar con `usage_meter`: tokens y turnos, si terminó con tests en verde, y cuántas veces hubo que
  intervenir.
- Con eso se decide si se extiende al resto de repos del equipo.

## Acceptance criteria

- [ ] Fuentes de skills: `JuliusBrussee/caveman` y `ComposioHQ/awesome-claude-skills` listan sus
  skills; `pbakaus/impeccable` aparece una sola vez; los tests del parser cubren los tres formatos.
- [ ] Impeccable instalada y activa en Fleet, y el agente la carga en una terminal de ese proyecto.
- [ ] Con *Superpowers* en un proyecto, una terminal Claude nueva de ese proyecto:
  - recibe el hook (el agente menciona `using-superpowers` al arrancar);
  - lista el plugin en `/plugin`;
  - muestra el distintivo **SP**.
- [ ] Una terminal de un proyecto *Spec-driven* no tiene el plugin, ni con la misma cuenta abierta a
  la vez en otro proyecto Superpowers.
- [ ] Ningún fichero del repo del proyecto ni de `~/.claude*/settings.json` cambia al activar
  Superpowers.
- [ ] Cambiar la metodología relanza las terminales del proyecto sin perder la conversación.
- [ ] `cargo test`, `tsc` y `vitest` en verde; release por `npm run release`.

## Open questions

1. **Plan mode en proyectos Superpowers:** ¿arrancan en modo normal (recomendado) o se mantiene plan
   mode y se acepta la doble aprobación?
2. **Piloto:** ¿simulation-server?
3. **Instalación del plugin:** la primera vez en cada config dir de cuenta, ¿Claude Code lo instala
   solo desde `extraKnownMarketplaces` o pide confirmación? Si pide, Hangar tendría que avisarlo en
   la terminal. Se verifica con el CLI real al empezar la fase 2.
4. **Impeccable:** ¿qué proyectos, además de Fleet, tienen interfaz?
