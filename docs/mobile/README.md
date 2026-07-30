# Puente móvil — índice

El contrato está en [MOBILE_BRIDGE_TRD.md](../../MOBILE_BRIDGE_TRD.md). Estos
documentos son cómo se construye cada parte.

| | Documento | Lado | Estado |
|---|---|---|---|
| 0 | [Protocolo y cifrado](TDD-0-protocolo.md) | Hangar | sin empezar |
| 1 | [El puente](TDD-1-puente.md) | Hangar | sin empezar |
| 2 | [Emparejamiento](TDD-2-emparejamiento.md) | Hangar | sin empezar |
| 3 | [El escritorio listo](TDD-3-escritorio.md) | Hangar | sin empezar |
| 4-8 | [El cliente Flutter](TDD-4-a-8-movil.md) | repo aparte | esbozo |

## El orden, y por qué

Las fases 0-3 construyen **todo lo que el móvil va a necesitar de Hangar**:
comandos, eventos con sus campos, emparejamiento, selección, modo. De la 4 en
adelante solo se toca el repositorio de Flutter.

La razón es evitar esto: estás construyendo una pantalla, descubres que falta un
campo en un evento, y tienes que parar, cambiar de repositorio, publicar el
escritorio y esperar. No es el código lo que cuesta, es el cambio de contexto.

**Con un solo desarrollador que prueba todo, eso cuesta un rebuild y poco más.**
La regla se vuelve cara el día que haya usuarios con versiones distintas: el
escritorio se actualiza solo, la app pasa por la App Store, y no van a la vez.

Así que hoy vale como aviso, no como prohibición: si vuelves al escritorio una
vez, mala suerte; si vuelves cinco, el contrato estaba mal pensado y conviene
arreglarlo antes de seguir parcheando.

## Probar sin tocar tu Hangar de todos los días

Tú usas Hangar mientras lo desarrollas, así que la instancia de pruebas no puede
pisar la estable:

```sh
HANGAR_DATA_DIR=~/.hangar-dev npm run tauri dev
```

Base de datos, cuentas y llaves aparte. Y como el `machine_id` vive en esa base,
la instancia de pruebas estrena canal: dos Hangar abiertos no se roban los
mensajes del puente.

Lo que **no** hace falta: la base es compatible hacia atrás. Una migración nueva
solo añade, y un build viejo salta las versiones que no conoce
(`if migration.version <= current { continue }`), así que abrir la estable
después de la de desarrollo no rompe nada. El peligro real era la concurrencia
—dos procesos escribiendo el mismo SQLite, dos registros de PTYs, dos puentes en
un canal—, y eso es lo que separa el directorio.

Sin la variable, nada cambia: un build instalado nunca la tiene puesta, y con
ella puesta se salta también el renombrado Fleet → Hangar, que si no movería tus
datos de verdad hacia la instancia de pruebas.

Con el directorio limpio empiezas sin cuentas ni proyectos. Para el puente da
igual: `list_projects` devolviendo `[]` prueba el camino completo —canal, cifrado,
allowlist, `invoke`— igual de bien que uno con datos.

## Dependencias externas

- **Fase 5 del roadmap** (login con Google en Supabase) — el canal necesita
  autenticación. Bloqueada en crear el cliente OAuth.
- **Cuenta de desarrollador de Apple** ($99/año) para publicar la app. La misma
  que resolvería el aviso de Gatekeeper en el escritorio.
