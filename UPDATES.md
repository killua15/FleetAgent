# Actualizaciones de Hangar — cómo funcionan ahora

> Estado a 2026-09-30 (v0.8.11). Sustituye a todo lo anterior sobre el canal de GitHub
> (`updates/`, el token en Ajustes, el workflow que publicaba al empujar un tag).

**Diagrama:** [docs/architecture/hangar-updates.html](docs/architecture/hangar-updates.html)
(fuente: `hangar-updates.workflow.json`, al lado).

En una frase: **publicas desde tu Mac con un comando, Supabase guarda la build, y cada Hangar la
descarga e instala solo; lo único que hace el usuario es pulsar Restart.**

```
npm run release  →  Supabase (Storage + app_releases)  →  Edge Function `updates`  →  app: descarga e instala  →  Restart
```

---

## 1. Publicar una versión

**Lo más fácil:** doble clic en `scripts/hangar-release.command` (abre Terminal), o
`npm run release:guided`. Comprueba y prepara la máquina (te pide la clave de Supabase si falta),
propone la siguiente versión, te deja elegir entre publicar, probar sin publicar o solo sacar el zip
para otro Mac, y al final ofrece dejar ese zip en el Escritorio.

Si prefieres el comando directo:

```bash
npm run release -- 0.8.12 "Qué cambió"          # publica de verdad
npm run release -- 0.8.12 --dry-run             # todo menos publicar
```

`scripts/release.sh` hace, en este orden, y se detiene en el primer fallo:

1. **Comprobaciones previas**, antes de tocar nada:
   - estás en `main`, sin cambios pendientes, y al día con `origin`;
   - la versión es mayor que la actual y su tag no existe todavía;
   - la clave de firma está en `~/.tauri/hangar.key` y la clave de Supabase en el Llavero.
2. **Sube la versión** en los 5 ficheros (`package.json`, `package-lock.json`, `Cargo.toml`,
   `Cargo.lock`, `tauri.conf.json`).
3. **Tests:** `cargo test`, `tsc --noEmit` y `vitest`. Si fallan, no se publica y los ficheros de
   versión vuelven a como estaban.
4. **Build firmada** para Apple Silicon (`tauri build --bundles app`). Genera
   `Hangar.app.tar.gz` + `.sig`.
5. **Publica en Supabase:**
   - sube el `.tar.gz` a Storage, en `updates/<versión>/darwin-aarch64/`;
   - registra la fila en `app_releases` (versión, firma, notas, canal `beta`).
6. **Verifica** que la fila y el fichero existen de verdad.
7. **Commit, tag y push:** `release: bump to X`, `vX` y push de `main` y del tag. Ese push **no lanza
   GitHub Actions**.

Tarda unos minutos y **no gasta minutos de GitHub**.

### Qué necesita tu Mac

| Qué | Dónde |
|---|---|
| Clave de firma del updater (sin contraseña) | `~/.tauri/hangar.key` |
| Clave `service_role` de Supabase | Llavero de macOS, servicio `hangar-supabase-service-role` |
| Contraseña de la clave de firma (solo si algún día tiene) | Llavero, servicio `hangar-signing-password` |

Nada de esto está en el repositorio. Para poner la clave de Supabase en otra máquina:

```bash
security add-generic-password -U -a hangar -s hangar-supabase-service-role -w '<service_role key>'
```

La clave de firma tiene que ser **la misma** que la pública que va dentro de la app
(`plugins.updater.pubkey` en `tauri.conf.json`). Si se pierde, las apps ya instaladas rechazarán
todas las actualizaciones nuevas.

---

## 2. Cómo le llega a cada Hangar

1. **La app pregunta** (`src/lib/updates.ts → runUpdateCheck`) al arrancar, al volver a la ventana y
   cada 30 minutos. Lo hace a la Edge Function `updates` con `target`, `arch` y `current_version`, y
   con **tu sesión de Supabase** como credencial.
2. **La Edge Function** (`supabase/functions/updates`) decide qué contestar:
   - sin sesión → no hay actualización;
   - cuenta sin `profiles.approved` → no hay actualización (la beta es cerrada);
   - si no → la última fila publicada de `app_releases` para esa plataforma (`darwin` + `aarch64` →
     `darwin-aarch64`), si es más nueva que la instalada.
3. **La app descarga e instala sola**, en segundo plano:
   - la descarga sale del bucket privado de Storage, con el mismo token recién pedido en el paso 1;
   - verifica la firma minisign contra la clave pública de la app;
   - sustituye `Hangar.app` en disco. La versión abierta sigue funcionando hasta que reinicias.
4. **El banner dice "vX.Y.Z Installed — restart Hangar to run it"** y un botón **Restart**. Reiniciar
   es lo único que hace el usuario. No se hace solo a propósito, para no cortar agentes a mitad de
   turno. Si no reinicias, la próxima vez que abras Hangar ya arranca la nueva.

En **Ajustes → Updates** se ve la versión que corre, un botón **Check for updates** y el estado
("You're on the latest version", "Downloading…", "Installed — Restart", o el error).

---

## 3. Si algo falla

- El banner muestra **"Update failed: …"** con **Retry**.
- El error queda en `~/Library/Logs/com.killua.hangar/hangar.log` con el prefijo **`updater:`**.
  Es lo primero que hay que mirar.

| Síntoma | Causa probable | Qué hacer |
|---|---|---|
| Nunca aparece una actualización | Sesión no iniciada, o la cuenta sin `approved` | Iniciar sesión; revisar `profiles.approved` |
| Nunca aparece, en un Mac Intel / Windows / Linux | Las releases locales solo publican Apple Silicon | Ver §4 |
| `400 Bad Request` al instalar (solo ≤ 0.8.10) | Token de sesión caducado: esas versiones esperaban al clic para descargar | Cerrar y abrir Hangar, e instalar en cuanto salga. Desde 0.8.11 no pasa |
| "Update failed" por la firma | La build no se firmó con la clave de la app | Publicar de nuevo con la clave correcta (§1) |

Para ver qué versión tiene cada cuenta: `profiles.app_version` en Supabase. Se actualiza cada vez que
la app pregunta, y es por cuenta, no por máquina.

---

## 4. Otras plataformas (Intel, Windows, Linux)

La build local solo produce **Apple Silicon**, que es lo único que se usa: el 100% de las consultas
de actualización son `darwin/aarch64`. Para las demás existe el workflow **Release** de GitHub, que
ahora es **solo manual**:

- GitHub → Actions → Release → *Run workflow*, eligiendo **el tag de la versión** (no `main`) en *Use
  workflow from*.
- Compila las 4 plataformas y publica en Supabase las que quepan. Linux (~83 MB) no cabe en el
  límite actual de Storage.
- **Cuesta unos 130 minutos** de los 2.000 gratis al mes: los minutos de macOS cuentan ×10 y los de
  Windows ×2. Úsalo solo si hace falta.

El CI tampoco corre ya en cada push; se lanza a mano desde Actions si se quiere.

---

## 5. Instalar a mano (una máquina que se quedó atrás)

Cuando una máquina no llega a actualizarse sola, se le pasa la app directamente:

1. En un Mac ya actualizado: `ditto -c -k --keepParent /Applications/Hangar.app ~/Desktop/Hangar.zip`
   (o clic derecho → Comprimir).
2. En la otra máquina:
   - cerrar Hangar;
   - descomprimir y arrastrar `Hangar.app` a Aplicaciones (Reemplazar);
   - abrir con clic derecho → Abrir la primera vez. Si macOS se niega:
     `xattr -dr com.apple.quarantine /Applications/Hangar.app`.
3. Los datos (terminales, cuentas, proyectos) están fuera del `.app` y se conservan. A partir de esa
   versión ya se actualiza sola.

---

## 6. El canal viejo de GitHub

Hasta la 0.8.x la app leía un manifiesto commiteado en `updates/` con un token personal de GitHub.
Ese canal está **congelado en 0.8.9**: nadie escribe ya en él.

Quien siga en una versión anterior a la 0.8 da un único salto a la 0.8.9 por ahí, y desde la 0.8.9
ya lee Supabase. El campo del token ya no existe en Ajustes, y el token guardado se borra del Llavero
al arrancar.

---

## Ficheros clave

| Pieza | Fichero |
|---|---|
| Comando de release | `scripts/release.sh` (`npm run release`) |
| Chequeo, descarga, instalación y estado | `src/lib/updates.ts` |
| Banner | `src/components/UpdateBanner.tsx` |
| Ajustes → Updates | `src/screens/GeneralSettings.tsx` (`UpdatesRow`) |
| Log de errores y limpieza del token viejo | `src-tauri/src/updater.rs` |
| Endpoint y clave pública | `src-tauri/tauri.conf.json` → `plugins.updater` |
| Qué versión recibe cada uno | `supabase/functions/updates/index.ts` |
| Build manual multiplataforma | `.github/workflows/release.yml` (solo `workflow_dispatch`) |
