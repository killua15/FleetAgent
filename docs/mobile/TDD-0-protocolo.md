# TDD 0 · Protocolo y cifrado

**Fase**: 0 · **Lado**: solo Hangar · **Estimación**: 3-4 días
**Depende de**: nada. **Bloquea**: todo lo demás.
**Contrato**: [MOBILE_BRIDGE_TRD.md](../../MOBILE_BRIDGE_TRD.md) §4, §5

---

## Objetivo

Dejar escrito y probado **cómo se habla**, sin que haya todavía con quién.

Al terminar, dos procesos en la misma máquina intercambian mensajes cifrados que
un tercero no puede leer, y un comando de Hangar se ejecuta a través de ese
canal. Sin red, sin Supabase, sin móvil.

Se hace primero porque es lo único que, si se equivoca, obliga a rehacer todo lo
demás.

## Entregables

```
src-tauri/src/bridge/mod.rs          el módulo
src-tauri/src/bridge/protocol.rs     el sobre, versión, códigos de error
src-tauri/src/bridge/crypto.rs       cifrado, envuelto
src-tauri/src/bridge/allowlist.rs    carga y consulta de la lista blanca
src-tauri/src/bridge/allowlist.json  los datos: comando → nivel
contract/fixtures/*.json             ficheros dorados
```

## Diseño

### El sobre

Tal cual §4 del TRD. Un tipo Rust con `serde`, y un test que serializa y
deserializa cada variante.

Solo `v`, `id`, `kind` y `nonce` van en claro. `name` viaja **dentro** del
payload cifrado: saber que alguien pide `list_account_spend` cada cinco minutos
ya dice algo.

### Cifrado

`crypto_secretbox` de libsodium — clave simétrica, no asimétrica, porque el
emparejamiento entrega la clave por el QR y no hay terceros que necesiten cifrar
sin poder descifrar.

- Clave de 32 bytes, generada por el Mac al emparejar, **guardada en el llavero**
  igual que todo lo demás (`credential_vault`).
- **Nonce de 24 bytes, aleatorio por mensaje.** Reutilizar un nonce con la misma
  clave rompe el cifrado entero, así que nunca es un contador ni deriva del reloj.
- Un mensaje que no descifra se descarta con `E_DECRYPT` y **no se registra su
  contenido** — un log con payloads es una filtración con retraso.

Nada casero: `sodiumoxide` o `crypto_box`, con tests contra vectores conocidos
para detectar si una actualización cambia el comportamiento.

### Lista blanca

Un fichero de datos, no código:

```json
{ "list_account_spend": "mirar",
  "send_line_to_terminal": "dirigir",
  "open_pr_review_terminal": "lanzar" }
```

Cargado al arrancar. Un comando que no está **no existe** para el puente
(`E_NOT_ALLOWED`), y esa es la diferencia entre "no puedes" y "no sé qué es"
(`E_UNKNOWN_COMMAND`), que son dos mensajes distintos para el usuario.

**Un test recorre la lista y falla si nombra un comando que Hangar no registra.**
Sin él, renombrar un comando rompe el móvil en silencio y solo se descubre desde
un teléfono.

### Ficheros dorados

Los tests que ya existen escriben lo que serializan a `contract/fixtures/`.
El CI falla si un fichero cambia y nadie lo ha mirado — que es exactamente cuándo
hay que enterarse.

No hay generación de código: para ~30 comandos es más maquinaria que contrato, y
una tubería rota no avisa de nada. Un fichero dorado que no parsea, sí.

## Criterios de aceptación

1. Un script de Node cifra un `invoke`, Rust lo descifra, ejecuta y devuelve el
   resultado cifrado; el script lo lee. Sin red.
2. Un mensaje con la clave equivocada da `E_DECRYPT` y no aparece en el log.
3. Un comando fuera de la lista da `E_NOT_ALLOWED`; uno inexistente,
   `E_UNKNOWN_COMMAND`.
4. Un `hello` con versión distinta da `E_PROTOCOL_VERSION` con las dos versiones.
5. El test de la lista blanca falla si se renombra un comando.

## Riesgos

**Reutilización de nonce.** El fallo criptográfico clásico. Mitigación: aleatorio
siempre, y un test que genera mil mensajes y comprueba que no se repite ninguno.

**La clave en memoria.** Vive en el proceso mientras el puente esté activo. Es el
mismo trato que el resto de secretos, y muere con el proceso.

## Fuera de alcance

Red, Supabase, emparejamiento, interfaz. Aquí solo se define el idioma.
