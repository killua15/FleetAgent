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

**Al terminar la fase 3, Hangar no se toca más por el móvil.** Ese es el criterio
que ordena todo lo demás: cada decisión que obligue a volver al escritorio
después de esa línea es una decisión que se tomó tarde.

## Dependencias externas

- **Fase 5 del roadmap** (login con Google en Supabase) — el canal necesita
  autenticación. Bloqueada en crear el cliente OAuth.
- **Cuenta de desarrollador de Apple** ($99/año) para publicar la app. La misma
  que resolvería el aviso de Gatekeeper en el escritorio.
