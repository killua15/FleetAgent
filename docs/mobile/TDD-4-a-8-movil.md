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

## Fase 8 · Voz · ~4 días

**Objetivo**: dictar un prompt.

`speech_to_text`, en el dispositivo, sin API ni coste. Transcribe, **lo enseña**,
y tú confirmas antes de enviar.

La confirmación no es burocracia: *"dijiste algo mal y el agente lo hizo"* es el
fallo que hunde la confianza en la voz para siempre, y ocurre una vez.

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
