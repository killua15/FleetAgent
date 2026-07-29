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

## Dependencias externas

- **Fase 5 del roadmap** (login con Google en Supabase) — el canal necesita
  autenticación. Bloqueada en crear el cliente OAuth.
- **Cuenta de desarrollador de Apple** ($99/año) para publicar la app. La misma
  que resolvería el aviso de Gatekeeper en el escritorio.
