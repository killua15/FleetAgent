# Roadmap: worktrees en terminal, explorador de ficheros, buscar PR, login y monetización

Cinco cosas, en fases. Cada una dice qué se puede hacer **hoy**, qué se podría hacer **después**, y
qué tamaño real tiene mirando el código que ya existe — no la impresión que da desde fuera.

Estado verificado el 2026-07-26 contra este repo:

- El proyecto de Supabase **Hangar ya existe** (`yqrvofogmyqdqufyxlfk`, ca-central-1,
  ACTIVE_HEALTHY) y está **vacío**: 0 tablas, 0 edge functions.
- La app **no tiene red**: ni `reqwest` en `Cargo.toml` ni `supabase-js` en `package.json`. `keyring`
  sí, vía `credential_vault.rs`.
- `create_subsession` (`sessions.rs:289`) ya crea worktrees de verdad, pero exige una sesión padre.
- `list_session_files` / `read_session_file` (`sessions.rs:882` y `:908`), `FileTreePanel` y
  `lib/diffLines.ts` ya existen, pero viven dentro de `AgentPanel.tsx` y van por sesión.

## Orden recomendado

Producto primero (fases 1-3), cobrar después (4-5). Poner el muro antes que las paredes gasta el
trabajo de gating en features que aún no existen, y son justo esas tres las que hacen que pagar
tenga sentido.

**Si la prioridad es facturar ya, se invierte: 4 → 5 → 1 → 2 → 3.**

---

# Fase 1 — Terminal con su propio worktree · S/M

## Qué puedes hacer hoy

Para trabajar en una rama aislada hay que pasar por el AgentPanel: abrir una sesión de agente, darle
a "crear subsesión", y el diálogo (`CreateSubsessionDialog`) pide nombre, cuenta, modelo, effort,
modo y setup command. Eso crea el worktree **y** una sesión de agente. Si luego quieres una consola
ahí, la abres contra esa sesión.

O sea: **el worktree solo nace como hijo de una sesión de agente**. Trabajando solo con terminales —
que es como quedó la app — no hay sesión padre, y no hay forma de llegar al worktree.

## Qué podrías hacer después

En `+ New terminal`, tras elegir la cuenta, en el paso "¿dónde debería correr?" aparece una fila más
junto a los proyectos recientes:

```
RECENT PROJECTS
  Fleet-Agent        /Users/killua/Proyects/Fleet-Agent
  propkit            /Users/killua/Proyects/propkit
  ⑃ Nuevo worktree…     ← esto es lo nuevo
  📁 Explore another folder…
```

Eliges proyecto y escribes un nombre — digamos `fix-billing` en Fleet-Agent. Lo que pasa:

1. `git worktree add` real en `~/Proyects/Fleet-Agent/.hangar/worktrees/fix-billing`, rama
   `subsession/fix-billing`
2. copia los `.env` del proyecto (no están en git, y sin ellos no arranca nada)
3. se asegura de que `.hangar/` está en el `.gitignore` del proyecto
4. corre el setup command si lo hay (`npm install`, lo que sea)
5. abre la terminal **con el cwd en ese worktree**, con Claude arrancado bajo la cuenta elegida

Tu checkout principal ni se entera. Puedes tener tres terminales sobre tres ramas del mismo repo a la
vez, cada una con su cuenta.

## Por qué es una fase pequeña

Porque el worktree ya lo sabe hacer. De `create_subsession` entero, **lo único que necesita del padre
es una query**: la de `sessions.rs:300-306`, que saca el `project_id` de la sesión padre. Todo lo
demás — `create_linked_worktree`, `copy_env_files`, `ensure_worktree_dir_ignored`,
`run_setup_command` — trabaja sobre `project_path`. Le pasas el proyecto directamente,
`parent_session_id` queda NULL, y el cuerpo no se toca.

## Qué se hereda del AgentPanel y qué no

| Del diálogo de subsesión | En la terminal |
|---|---|
| Nombre / rama | sí |
| Cuenta | sí |
| Setup command | sí |
| Worktree real, o copia si no hay git | sí, igual |
| Copia de `.env` | sí, igual |
| Modelo, effort, modo | **no** |

Los tres últimos no, porque en una terminal los elige el propio Claude Code al arrancar. Guardarlos
en la fila sería un valor que nadie lee y que se contradice con lo que ves en pantalla.

## Lo único que hay que decidir

**Qué pasa con el worktree al cerrar la terminal.** Propuesta: no se borra. Cerrar una pestaña no
puede llevarse trabajo sin commitear.

El borrado ya existe — `delete_session` (`sessions.rs:634`) hace el `git worktree remove` de verdad.
Falta el botón, y el sitio natural es el Git panel, que ya lista los worktrees de cada proyecto. Ahí,
con confirmación.

El coste: si abres worktrees a lo loco se acumulan y los barres tú. La alternativa (borrar al cerrar)
es peor por lo dicho arriba.

---

# Fase 2 — Explorador de ficheros como pantalla · M

## Qué puedes hacer hoy

El árbol de ficheros existe y funciona: colorea por estado de git, marca los borrados, y al pinchar
un fichero te da dos pestañas — el diff de lo que el agente cambió, y el contenido actual. Es una de
las mejores partes de la app.

Y solo se llega **dentro del panel de un agente**. `list_session_files` recibe un `session_id`,
`FileTreePanel` vive dentro de `AgentPanel.tsx`, y si no hay sesión abierta no hay árbol. Para mirar
un fichero de un proyecto acabas en el editor de fuera.

## Qué podrías hacer después

Una pantalla `Files` al lado de Git en la barra de arriba, con la misma estructura que ya tiene el
Git panel: proyectos a la izquierda, contenido a la derecha.

```
PROJECTS            │  src/screens/                     │  ▸ diff  │ file
  Fleet-Agent   ◂   │    ▾ src                          │
  propkit           │      ▾ screens                    │   1  import { useState } …
  reachly           │          PrReviews.tsx      M     │   2
                    │          Terminals.tsx      M     │   3  export function PrReviews() {
                    │        components                 │   …
                    │      ROADMAP.md             ??    │
```

Pinchas un proyecto, navegas su árbol, lees cualquier fichero con resaltado de sintaxis, y si está
modificado tienes la pestaña de diff contra `HEAD`. Sin sesión, sin agente, sin worktree — el
proyecto y ya.

## Por qué es media y no pequeña

Porque son tres piezas y ninguna es difícil, pero ninguna es gratis:

1. **Backend**: generalizar a un target en vez de una sesión, exactamente como ya hicieron las
   terminales. `list_files(target)` / `read_file(target, path)` con `target = project | session`,
   reusando el `resolve_target` que ya existe. Los comandos por sesión se quedan como envoltorio para
   no tocar el AgentPanel.
2. **Extracción**: `FileTreePanel` y `FileTreeRows` salen de `AgentPanel.tsx` (1.600 líneas) a
   `components/FileTree.tsx`, para que las dos pantallas usen el mismo componente y no dos copias que
   se separan con el tiempo.
3. **Pantalla nueva**: `Files.tsx` con la estructura de `GitPanel`, resaltado con
   `prism-react-renderer` (ya es dependencia) y el diff con `lib/diffLines.ts` (ya existe).

## El agujero que hay que tapar en esta fase

`read_session_file` (`sessions.rs:908`) hace:

```rust
std::fs::read_to_string(Path::new(&cwd).join(&path))
```

Sin comprobar nada. Y `join` con una ruta **absoluta descarta la base entera**, así que
`path = "/etc/passwd"` lee `/etc/passwd`, y `"../../.ssh/id_rsa"` lee tu clave privada.

Hoy es inofensivo porque el único que llama es el árbol del AgentPanel, que solo manda rutas que él
mismo generó. Convertirlo en el visor de ficheros general de la app es exactamente lo que lo
convierte en un problema: más superficie, y una ruta que ya no viene siempre de un árbol de
confianza. Se canonicaliza y se comprueba que cae bajo la raíz resuelta, con test, en esta fase — no
en una limpieza posterior.

## Una decisión de forma ya tomada

Pantalla, no rejilla. Es la lección que ya se pagó una vez: las features densas en el grid se
revirtieron después de probarlas en vivo.

---

# Fase 3 — Buscar PR · S

## Qué puedes hacer hoy

Elegir un proyecto en la lista de la izquierda y mirar sus PRs. Nada más. No hay caja de búsqueda, y
ahora que el descubrimiento trae **todos** los PRs abiertos del repo — no solo los tuyos — la lista
de un repo con tráfico es larga y hay que recorrerla a ojo, proyecto por proyecto.

## Qué podrías hacer después

Una caja de búsqueda arriba de la lista que filtra **entre todos los proyectos a la vez**, no solo el
seleccionado:

```
⌕ billing                                            3 resultados

  killua15/Hangar #128   Fix billing rounding       @killua15      mine
  acme/api #4412         billing: retry on 429      @alice         review
  acme/api #4380         chore: billing fixtures    @bob           other
```

Busca por número, título, autor, repo y rama. Es filtrado local sobre lo que ya está en SQLite: cero
llamadas a `gh`, respuesta mientras tecleas.

Y en `⌘K`: escribes `#128` o pegas la URL de un PR y saltas directo a su tarjeta.

## El caso que no se puede resolver, y por qué

Traer **un PR concreto que el descubrimiento no haya visto**: se hace con `gh pr view <número|url>` y
se inserta como una fila normal — **siempre que ese repo sea un proyecto de Hangar**.

Para un repo que no lo es, la búsqueda puede encontrarlo, pero no se ofrece revisarlo: sin clon local
no hay worktree que crear, y el botón de revisar no tendría dónde abrir la terminal. Ahí la única
respuesta honesta es "añádelo como proyecto primero". Prometer lo otro sería un botón que falla
después de que te haya ilusionado.

## Por qué es la fase más pequeña

Porque no hay backend nuevo: los datos ya están en `pull_requests` y el descubrimiento ya los
mantiene. Es una caja de texto, un filtro sobre un array que ya está en memoria, y una entrada en el
palette.

---

# Fase 4 — Login con Google sobre Supabase · M/L

## Qué puedes hacer hoy

Nada, y no es una exageración: **la app no habla con ninguna red**. No hay cliente HTTP en Rust, ni
`supabase-js` en el frontend, ni pantalla de cuenta. Todo lo que hay es local — SQLite, el llavero
del sistema y los CLIs.

Esto es lo que hace que esta fase sea la más grande de las cinco aunque "solo" sea un botón: es la
primera capa de red del producto, y todo lo que la sostiene es nuevo.

## Qué podrías hacer después

En la barra de arriba, a la derecha, un botón `Sign in with Google`. Le das, se abre tu navegador,
eliges la cuenta de Google, y la pestaña se cierra sola diciendo "puedes volver a Hangar". En la
barra queda tu email. Cierras la app, la reabres, sigues dentro.

Y ya está — en esta fase el login **no desbloquea nada todavía**. Es la fase 5 la que le da sentido.

## Las tres piezas

**1. El proyecto de Supabase** (ya existe, vacío):

- Habilitar el provider de Google. Requiere un OAuth client en Google Cloud Console: **eso lo tienes
  que crear tú**, el client id y el secret van en la config de Supabase y no puedo generarlos yo.
- Esquema mínimo, con RLS desde el minuto uno:
  - `profiles(id → auth.users, email, created_at)` — cada usuario solo ve la suya.
  - `subscriptions(user_id, status, plan, current_period_end, stripe_customer_id, …)` — el usuario
    **solo lee** la suya; escribir es exclusivo del webhook con service role.

  Esa segunda regla no es una formalidad. Si la app puede escribir su propia fila de suscripción, no
  hay monetización: hay una casilla que cualquiera marca.

**2. El flujo OAuth de escritorio**, que no es el de web:

- PKCE con `supabase-js` y `skipBrowserRedirect: true`, abrir el navegador del sistema con
  `plugin-opener`, y recibir el código en un **servidor loopback** (`http://127.0.0.1:<puerto>`) que
  Rust levanta solo mientras dura el login.
- Loopback y no deep link (`hangar://`): funciona igual en `tauri dev` que en el bundle, sin
  registrar esquemas en macOS ni depender de que el sistema asocie el protocolo.

**3. Dónde vive el token**: el refresh token va **al llavero**, con el `credential_vault.rs` que ya
usa `keyring`. Nunca a `localStorage` — cualquier cosa que corra en el webview lo lee.

## Lo que propongo como regla

**La app funciona sin login.** Es una herramienta local que orquesta CLIs que ya pagas; pedir una
cuenta para abrir una terminal es hostil y no compra nada. El login es la puerta de Pro, no la puerta
de la app.

## Hecho cuando

Entro con Google, cierro, reabro y sigo dentro; el token no está en `localStorage`; y sin login todo
lo gratuito sigue funcionando igual que hoy.

---

# Fase 5 — Monetización · M de código, + trabajo que no es código

## La oferta

**Hangar Pro, $19/mes.** (Y $190/año más adelante: dos meses gratis.)

Por qué ese número: Cursor Pro está en $20, Warp ronda $18, Raycast Pro entre $8 y $16. Hangar es una
capa de orquestación sobre suscripciones que el usuario **ya paga**, así que tiene que quedar por
debajo de Cursor y ser una cifra que no se piensa. $29 te obliga a justificarte contra Cursor cada
vez; $9 regala margen sin comprar volumen, porque todavía no hay canal de distribución que convierta
ese descuento en usuarios.

## El corte

Sigue el valor real del producto: **paralelismo y multi-cuenta**.

| | Free | Pro |
|---|---|---|
| Cuentas de proveedor | 1 | ilimitadas |
| Terminales simultáneas | 2 | ilimitadas |
| Rotación + switcher de cuenta en el chip | — | sí |
| Continuidad de conversación al rotar | — | sí |
| PRs | solo pestaña "Mine" | las tres + Review con Claude |
| Worktrees activos | 1 | ilimitados |
| Multi-terminal | — | sí |
| Explorador de ficheros y Git | sí | sí |

El explorador y Git van en gratis a propósito: son lo que hace que alguien deje la app abierta todo
el día, y la app abierta es lo que vende el resto. Cobrar por mirar un fichero solo consigue que
cierres la ventana.

## Cómo se implementa

- **`entitlements.rs`: un solo sitio** que responde "¿puede hacer X?". Nada de comprobaciones sueltas
  repartidas por veinte ficheros — así es como se termina con una release en la que la mitad de los
  límites no aplican y nadie sabe cuáles.
- **Caché local con validez de 7 días.** La app tiene que funcionar en un avión. Se refresca al
  arrancar y al volver de Stripe.
- **Stripe Checkout + Customer Portal**; el webhook pega en una Edge Function de Supabase que escribe
  `subscriptions` con service role.
- **Los mensajes de límite dicen qué topaste y cuánto cuesta quitarlo**, no un "upgrade" genérico.
  "Llevas 2 terminales, que es el máximo del plan gratuito" es información; "Hazte Pro" es un cartel.

## Lo que hay que decir en voz alta

Esto es una app de escritorio local. Cualquiera con ganas edita el SQLite o recompila y se pone Pro.

El gating honesto — claro, legible, sin ofuscar — sirve para el 99% que no va a hacer eso. Construir
DRM son semanas peleando con el 1%, empeorando la app para todos los demás, y perdiendo igual. La
palanca real es la comodidad, las actualizaciones, y más adelante cualquier cosa que necesite
servidor de verdad.

## Lo que no es código

Página de precios, términos, política de reembolso, y decidir si facturas como persona o como
sociedad. Va en la fase aunque no se escriba en Rust, porque sin eso no se cobra.

---

# Supuestos

Si alguno de estos falla, cambia el plan:

1. **No hay sync de datos a Supabase.** El login es identidad y derechos de acceso; nada más. Tus
   proyectos, sesiones, transcripts y credenciales siguen 100% locales. Sincronizarlos es otro
   producto — y otro problema de privacidad, porque ahí dentro hay rutas, ramas y conversaciones.
2. **La distribución es fuera de la App Store** (DMG / GitHub Releases), así que Stripe vale y no hay
   comisión del 15-30% de Apple.
3. **"Buscar PR" es entre los PRs de tus proyectos**, no un buscador global de GitHub.
