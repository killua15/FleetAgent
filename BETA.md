# La beta privada — lo que queda, en orden

> **Este documento sustituye a [ROADMAP.md](ROADMAP.md) §Monetización y §Distribución** mientras la
> beta esté en marcha. Aquel se escribió el 2026-07-26 dando por hecha una distribución pública y un
> precio de Teams; la dirección de ahora es beta privada por invitación, y el precio no está
> decidido. El ROADMAP sigue siendo el registro de cómo se pensaba entonces y de las fases 1 a 10 del
> producto — no de cómo se reparte ni cómo se cobra.

Decidido: **repositorio privado**, builds para macOS, Windows y Linux, un programa de beta propio,
pulir los items uno a uno, y **la firma de código al final**. Los primeros testers son amigos, y la
vía de captación que se está probando es un canal de YouTube programando en directo.

Dejar la firma para el final no es un atajo: **un amigo se salta Gatekeeper porque confía en ti**, un
desconocido no. La firma es el gasto que toca hacer el día que se reparte fuera del círculo. Como
consecuencia, arrancar cuesta prácticamente solo los minutos de CI.

---

## Estado

**Hecho** — rama `feat/three-platform-builds-and-beta-polish`, sin subir:

| Commit | Qué |
|---|---|
| `464959c` | Linux en la matriz de release, y un artefacto de actualización por plataforma. Job de CI en Linux que ejecuta los tests |
| `6382ac3` | El refresh token del harness sale del árbol de trabajo, a `~/.hangar-dev/` con permisos 0600 |
| `16bf0e5` | La cola de ejecución vuelve a la barra superior, con icono propio |
| `d42177d` | El asistente de cuentas deja de ofrecer proveedores que Hangar no puede escopar |
| `e682c26` | README: cuatro afirmaciones corregidas, en los dos sentidos |

Verificado sobre el árbol completo: typecheck limpio, 87 tests de frontend, 552 de backend, 0 fallos.

**Bloqueado** — la facturación de GitHub Actions está cortada desde el 2026-08-14. Los workflows
nuevos están commiteados pero **nunca se han ejecutado**: los últimos runs fallan en 3-5 segundos, que
es la firma de un rechazo por facturación y no de un build roto.

---

## Fase 1 · Desbloquear la construcción

Nada de lo demás se puede probar de verdad hasta aquí.

- **Restablecer la facturación de Actions.** Ojo al coste real: en repositorios privados los minutos
  de **macOS cuentan 10×** y los de Windows 2×. Con builds de Tauri de 10-15 minutos y tres
  plataformas, el tramo gratuito se agota rápido. Presupuestarlo, no asumir que entra.
- **Dejar funcionando el camino local**, que ya está documentado en el [README](README.md): Windows
  desde el Mac con `cargo-xwin` + NSIS. No es opcional — es el seguro para la próxima caída, que ya
  ha pasado una vez.

**Hecho cuando** un push deja `CI` en verde y un tag produce artefactos de las tres plataformas.

---

## Fase 2 · Tres plataformas — hecho, sin verificar

El código está (`464959c`); lo que falta es haberlo visto correr.

- La matriz cubre `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc` y
  `x86_64-unknown-linux-gnu`.
- Linux compila en `ubuntu-22.04` a propósito: la glibc contra la que enlazas es el **suelo** de
  compatibilidad de tus usuarios, así que compilar en el runner más nuevo deja fuera a cualquiera con
  una distribución anterior. 22.04 es además la última LTS con `webkit2gtk-4.1`, que es lo que
  necesita Tauri 2.
- **En Linux el updater de Tauri solo actualiza AppImage**, no `.deb` ni `.rpm`. Se publican los dos;
  el canal de beta reparte AppImage.
- El artefacto de actualización se localiza por su `.sig` y no por nombre, porque se llama distinto en
  cada plataforma, y acotado a un `updaterDir` por plataforma para que Windows —  que genera NSIS *y*
  MSI— no haga el manifiesto no determinista.
- `publish-update-manifest` **sigue publicando solo Apple Silicon, y es deliberado**: publica
  commiteando el binario a `main`, ~9 MB permanentes por release sobre un `.git` que ya ronda los
  196 MB. Hacerlo cuatro veces sería cuadruplicar un problema que la fase 3 elimina en vez de
  resolver.
- En Apple Silicon un binario **necesita** alguna firma para arrancar, aunque sea ad-hoc. CI ya usa
  `signingIdentity: "-"`, así que eso está cubierto: lo que falta es la confianza de Gatekeeper, no
  la capacidad de ejecutarse.

**Pendiente de comprobar**: que el job de Linux pase. Es probable que sea el que descubra las
suposiciones de macOS que el README ya admite — `open -a Terminal`, `scutil --get ComputerName`, las
rutas de `resolve_binary`. Un rojo ahí significa que un build de Linux no está listo para dárselo a
nadie, que es exactamente lo que se quiere saber **antes** de que alguien lo instale.

---

## Fase 3 · El programa de beta

**TestFlight no es una opción, y no por precio.** TestFlight para Mac exige distribuir por el Mac App
Store, y eso obliga al App Sandbox. Hangar lanza PTYs, ejecuta `claude`, `git`, `gh` y `glab`, crea
worktrees y lee rutas arbitrarias del disco: es justo lo que el sandbox prohíbe. No hay una versión
de Hangar que entre en el App Store sin dejar de ser Hangar.

La buena noticia es que **ya está construido el 70% de un TestFlight propio**:

| Pieza de un programa de beta | Lo que ya existe |
|---|---|
| Lista de invitados | `profiles.approved`, que solo el service role puede cambiar |
| Canal de entrega cerrado | `src-tauri/src/update_access.rs` + endpoint privado del updater |
| Quién corre qué versión y dónde | `profiles.app_version`, `profiles.last_machine`, tabla `machines` |
| Identidad del tester | OAuth de Google funcionando, refresh token en el llavero |
| Firma de los **artefactos** | La clave minisign del updater — **independiente de Apple y de Microsoft** |
| Tolerancia a estar sin red | `within_grace` en `auth.rs`, 7 días, con tests |

Ese último punto es lo que hace viable dejar la firma para el final: **el updater verifica con
minisign, no con Gatekeeper**, así que las actualizaciones automáticas funcionan sin firma de Apple.

### Lo que hay que construir

La pieza central es sustituir el endpoint del updater por una **Edge Function de Supabase**. Hoy
`plugins.updater.endpoints` apunta a la API de contenidos de GitHub y **exige que cada usuario pegue
un PAT a mano** en los ajustes. Con amigos ya es incómodo; con desconocidos es inviable.

1. **Artefactos a Supabase Storage**, no commiteados al repo. Elimina de paso el crecimiento
   permanente de `.git`. — **Escrito, sin desplegar**: bucket privado `updates` y tabla
   `app_releases` en `supabase/migrations/20260918000000_app_releases.sql`, y el job
   `publish-to-supabase` del workflow de release sube los cuatro artefactos y escribe una fila por
   plataforma.
2. **Edge Function `updates`** que recibe la sesión de Supabase del tester y devuelve el manifiesto
   de su plataforma. **Esto es lo que elimina el PAT.** — **Escrita, sin desplegar**:
   `supabase/functions/updates/index.ts`. Las cabeceras ya no salen de `update_request_headers` sino
   de la sesión (`UpdateBanner.tsx`), y el endpoint de `tauri.conf.json` apunta a la función.

   Lo que **falta** para que esto viva: aplicar la migración, desplegar la función, y crear los
   secretos `SUPABASE_URL` y `SUPABASE_SERVICE_ROLE_KEY` en el repo.

   Dos cosas que este diseño da por hechas, y conviene no romper al tocarlo. El updater **reutiliza
   las cabeceras de la comprobación para descargar el binario**
   (`tauri-plugin-updater/src/updater.rs`), y por eso el bucket puede ser privado sin URLs firmadas:
   el JWT del tester ya viaja en la descarga y lo resuelve RLS. Y por eso mismo **no se fija ninguna
   cabecera `Accept`**: el plugin pone `application/json` para el manifiesto y
   `application/octet-stream` para el binario, y fijar una rompe la otra.

   `update_access.rs` y su campo en ajustes siguen ahí pero ya no los usa nadie: se borran cuando se
   borre el job `publish-update-manifest`, que es cuando ningún tester siga en una versión que apunte
   al endpoint de GitHub.
3. **Códigos de invitación** de un solo uso que al canjearse ponen `approved = true`. Es lo que se
   reparte por WhatsApp a los amigos y, más adelante, en el directo.
4. **Feedback desde dentro de la app**: un comando que adjunte versión, plataforma, `machine_id` y
   las últimas líneas de log. Sin esto los reportes llegan por WhatsApp y se pierden.
5. **Guía de instalación por plataforma**, imprescindible mientras no haya firma: en macOS clic
   derecho → Abrir, o Ajustes → Privacidad y seguridad → "Abrir de todos modos"; en Windows, "Más
   información" → "Ejecutar de todas formas"; en Linux, nada — **es la plataforma más cómoda para una
   beta sin firmar**.

**Se aplaza** hasta que haya testers suficientes para que importe: los canales
`internal`/`beta`/`stable` y el despliegue por porcentaje. Con cinco amigos no aportan nada, y el
diseño del punto 2 los admite después sin rehacerse.

**Verificar en esta fase**: que una actualización automática sobre una app sin firmar **no vuelva a
poner el atributo de cuarentena** en macOS. Si lo hiciera, cada actualización pediría permiso otra vez
y arruinaría el bucle.

---

## Fase 4 · Pulir los items

| Item | Estado |
|---|---|
| Cola de ejecución escondida | **Hecho** (`16bf0e5`) |
| README con cuatro afirmaciones desactualizadas | **Hecho** (`e682c26`) |
| Proveedores sin ámbito ofrecidos en el asistente | **Hecho** (`d42177d`) |
| `git_checkpoints` | Tabla **sin ningún escritor**: el único INSERT está dentro de la migración de reparación de claves foráneas. La promesa ya salió del README; queda decidir si se implementa o se borra la tabla |
| *Shared Memory* | CRUD completo (12 comandos, 3 tablas) cuyo contenido **nunca llega a un agente** — `SettingsTabs.tsx` lo admite. Cablearlo al spawn o esconderlo |
| Paleta ⌘K | Escondida por un bug conocido: busca sobre filas de `list_workspace_sessions` que no se podan al cerrar una terminal |
| `usage_scraper.rs` | Parsea la TUI de `/usage`. Se romperá cuando Anthropic cambie una línea, y en una beta eso son diez reportes idénticos. Necesita degradar con elegancia y un test canario |
| Jerarquías de agentes | 6 pantallas (`TreeCanvas`, `TreeColumns`, `TreeOutline`, `ExecRun`, `MainView`, `SplitView`) **inalcanzables** desde la interfaz. Aparcadas a propósito; decidir si vuelven o se retiran |
| `src/lib/teams.ts` | Cliente completo de pool de cuentas con leases, **cero importadores**. Es el primitivo de Team esperando pantalla — no se toca hasta que haya clientes |
| Grok, Gemini, Kimi | Con `d42177d` ya no se pueden elegir. Habilitarlos exige una sonda de login por proveedor; Kimi además solo autentica con API key, así que con cuentas por suscripción no tiene camino |

---

## Fase 5 · Emitir en directo sin filtrar credenciales

El producto se demuestra usándolo — agentes en paralelo, una cuenta agotándose y la rotación
continuando la conversación es buen vídeo por sí mismo. Pero emitir usando Hangar pone cosas
concretas en cámara.

- **Hecho**: el refresh token del harness ya no vive en el árbol de trabajo (`6382ac3`).
- **Pendiente — modo directo**: un interruptor que enmascare los emails de las cuentas
  (`provider_email`, visibles en Accounts y en el selector de terminal) y el campo del token de
  actualización en ajustes. Pequeño, y necesario cada vez que se emita.
- Lo que **sí** conviene enseñar: el gasto en dólares por turno y las barras de cuota. Es el argumento
  de venta y no es sensible.
- Los repos que se abran en directo llevan sus propios `.env`, y el navegador de ficheros los muestra.
- Un descuido de tres segundos con un token en pantalla no se borra del vídeo. El modo directo es un
  requisito, no una comodidad.

**El embudo**: cada directo termina con un código de invitación, que es la razón por la que las
invitaciones se construyen en la fase 3 y no después.

---

## Fase 6 · Firmar

**El disparador**: el primer tester que no te conoce personalmente. Antes de eso es dinero parado.

| Plataforma | Qué hace falta | Coste |
|---|---|---|
| macOS | Developer ID Application + notarización. Sustituye el `signingIdentity: "Hangar"` de `tauri.conf.json`, que es una identidad del llavero local | **$99/año** |
| Windows | **Azure Trusted Signing**, abierto a autónomos verificados de UE y Reino Unido | **~$10/mes** |
| Linux | Nada | **$0** |

Dos avisos para cuando llegue el momento:

- Desde la primavera de 2026 Microsoft trata los certificados **EV igual que los OV**: ya no saltan la
  reputación de SmartScreen. **Pagar $325 por un EV no ahorra el periodo de advertencias**, así que
  Trusted Signing es la elección correcta y no una rebaja.
- Hay casos reportados de fricción al darse de alta como individuo en Trusted Signing (pedía una
  licencia de Entra ID para crear el rol). Validar el alta **antes** de contar con ella.

**Cómo saber que toca**: medir en qué paso abandona la gente. El día que un desconocido se caiga en
el aviso de Gatekeeper, es la señal. Conviene medirlo, no intuirlo.

---

## Sin decidir

**El precio.** El tramo de $5-10 self-serve está libre en esta categoría: Orca es MIT y gratis del
todo y monetiza con un *Enterprise* sin precio público; Conductor no cobra nada todavía. Ninguno de
los dos va a bajar a ese tramo. Pero con cero usuarios el precio es hipotético, así que la decisión
espera a tener testers que digan si esto les resuelve algo.

Lo que sí está decidido del cobro: **merchant of record** (Polar o Lemon Squeezy) cuando llegue, para
no cargar con el IVA de treinta países siendo uno. El esqueleto ya existe — `subscriptions` con RLS
de solo lectura para el cliente, `src/auth/plan.ts` con todo el vocabulario de estados y sus tests, y
el grace period de 7 días. Lo que falta es el webhook: **no hay ninguna Edge Function todavía**, así
que hoy nada puede activar una suscripción.

---

## Los tests `--ignored`, y un aviso

Hay **12 tests marcados `#[ignore]`** y son los únicos que ejercitan lo que de verdad se vende. Pero
`cargo test -- --ignored` **no es "correr y ya"**: cada uno necesita algo distinto.

| Dónde | Qué necesita |
|---|---|
| `login_manager.rs` (4) | El `codex` o el `claude` real instalado |
| `terminals.rs`, `turn_runner.rs`, `usage_scraper.rs` (3) | Lanzan un `claude` real; el de `usage_scraper` además necesita un config dir con sesión |
| `commands/graph.rs` (1) | `graphify` real contra dos repos reales — `HANGAR_GRAPH_TEST_REPO_A/B` |
| `memory_pressure.rs` (1) | Nada: solo imprime el swap de la máquina. Es un diagnóstico, no una prueba |
| `forges/gitlab.rs` (3) | `glab auth login` más `HANGAR_GITLAB_TEST_REPO` / `HANGAR_GITLAB_TEST_SELFHOSTED` / `HANGAR_GITLAB_TEST_BRANCH` |

> ⚠️ **`forges/gitlab.rs:603` crea un merge request de verdad.** Correr los ignorados a ciegas deja
> basura en un repositorio real. Ese se ejecuta a mano y con intención, no en lote.

---

## El riesgo que no se controla desde aquí

- **La velocidad del competidor.** Orca cerró el hueco del *account switcher* en un mes, mientras el
  ROADMAP todavía lo daba por libre. Lo que sigue siendo exclusivo es más estrecho que entonces:
  rotación **automática** al agotarse la cuota, política **por proyecto**, y la conversación
  **sobreviviendo** al cambio de cuenta — las tres pedidas en issues abiertos suyos y ninguna resuelta
  por su switcher. La AGPL les impide copiar el código, no reimplementarlo. La ventaja real es que no
  es su dirección: van a superficie de IDE y a contratos.
- **Riesgo de plataforma.** Anthropic ha dicho que tener varias cuentas no viola los términos — lo
  que persiguen es compartirlas y revenderlas. Pero hay reportes de cuota cruzada entre cuentas del
  mismo titular. Si unifican la cuota por persona, la rotación pierde valor y el producto pasa a ser
  la auditoría de gasto. Conviene que el ledger esté sólido por si ese día llega.
- **Linux multiplica el soporte por poco retorno** (glibc, Wayland contra X11, portales de ficheros).
  Si hay que sacrificar una plataforma por tiempo, es la que tiene menos usuarios de agentes CLI de
  pago.
