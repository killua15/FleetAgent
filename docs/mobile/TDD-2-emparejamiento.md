# TDD 2 · Emparejamiento y dispositivos

**Fase**: 2 · **Lado**: solo Hangar · **Estimación**: 4 días
**Depende de**: TDD 0 (clave), TDD 1 (canal).
**Contrato**: TRD §7, §12.3

---

## Objetivo

Que un teléfono obtenga la clave de un Mac de forma que no pase por la red, y que
el Mac pueda cortarle el acceso en cualquier momento.

## Diseño

### El QR es el canal seguro

```jsonc
{ "m": "<machine_id>", "n": "MacBook de David",
  "k": "<clave de 32 bytes, base64>", "exp": 1785… , "lvl": "dirigir" }
```

La clave viaja **en el QR y solo ahí**: un canal fuera de banda que se ve con los
ojos y no atraviesa ningún servidor. Es la razón de que todo lo demás pueda ser
cifrado extremo a extremo sin intercambio de claves por red.

- **Caduca a los 120 segundos.** Una captura de pantalla en un chat no debe
  seguir sirviendo mañana.
- Se genera de nuevo cada vez que se abre la pantalla. No se guarda.

### Escanear no es autorizar

El teléfono escanea y publica `hello`. Hangar **lo enseña y espera**:

```
  iPhone de David quiere emparejarse · nivel: dirigir
  [ Aceptar ]   [ Rechazar ]
```

Aunque el QR sea correcto. Un QR fotografiado por encima del hombro no debe
bastar; la aceptación ocurre delante del Mac.

### Un solo teléfono, varios Macs

Emparejar uno nuevo **expulsa al anterior** (TRD §12.3). El expulsado recibe
`E_DEVICE_REPLACED` y lo dice en su pantalla, en vez de quedarse fallando en
silencio.

Cada Mac tiene su propio emparejamiento y su propia clave: perder uno no toca los
demás.

### Registro

```sql
CREATE TABLE paired_device (
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL,      -- "iPhone de David"
  level        TEXT NOT NULL CHECK (level IN ('mirar','dirigir','lanzar')),
  credential_ref TEXT NOT NULL,    -- la clave, en el llavero — nunca aquí
  paired_at    TEXT NOT NULL,
  last_seen_at TEXT,
  push_token   TEXT                -- sin uso hoy; ver TRD §12.5
);
```

Una fila como mucho, por la regla de un solo teléfono. Sigue siendo tabla y no
ajuste suelto porque tiene campos con vida propia y porque un día podrían ser
varios.

**La clave va al llavero**, como todos los secretos de este proyecto. La base
guarda una referencia.

### Revocar

Un botón en Settings. Borra la clave del llavero y la fila. El corte es inmediato
y no necesita que el teléfono coopere: sin clave no puede descifrar nada de lo
que llegue, ni cifrar nada que Hangar acepte.

## Criterios de aceptación

1. El QR caduca a los 120 s y uno caducado da un error claro.
2. Escanear con el QR correcto **no** empareja hasta que se acepta en el Mac.
3. Emparejar un segundo teléfono expulsa al primero, y el primero lo dice.
4. Revocar corta el acceso en el siguiente mensaje.
5. La clave no aparece en la base de datos, ni en logs, ni en el frontend.

## Riesgos

**El QR en pantalla.** Es visible para quien esté detrás. Mitigado por la
caducidad corta y por la aceptación explícita, que exige tener el Mac.

**Un móvil perdido con la app abierta.** Es acceso al nivel que se le diera.
Mitigación: revocar desde el Mac, y que `lanzar` no sea el nivel por defecto.

## Fuera de alcance

Notificaciones push (descartadas, TRD §12.5). `push_token` existe en el esquema
por si eso cambia, y hoy nadie lo lee.
