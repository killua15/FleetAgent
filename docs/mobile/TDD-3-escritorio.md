# TDD 3 · El escritorio, listo para el móvil

**Fase**: 3 · **Lado**: solo Hangar · **Estimación**: 3 días
**Depende de**: TDD 1, TDD 2.
**Contrato**: TRD §12.10, §12.11, §14

---

## Objetivo

Lo que Hangar tiene que enseñar y permitir para que el móvil sea seguro de usar.
**Al terminar esta fase, el escritorio no se toca más por el móvil.**

## Diseño

### La selección se ve

Cuando el teléfono selecciona una terminal, ese panel lo dice:

```
  Simulation-Server   ● trabajando   📱 iPhone de David
```

Un agente dirigido desde fuera no puede parecer igual que uno que nadie está
tocando. Es la señal que evita que dos personas —o la misma en dos sitios—
escriban encima.

Seleccionar una terminal **minimizada no la restaura**: minimizar es una decisión
del escritorio y el móvil no la revoca. Se marca en la barra lateral y ya.

### Modo global

Un ajuste de máquina en Settings, con las cuatro opciones que ya existen
(`plan`, `ask_edits`, `auto_edit`, `yolo`).

Y con la consecuencia escrita al lado, no escondida:

> Con **yolo**, todas las terminales de este Mac ejecutan sin preguntar,
> incluida la del repositorio que más te importa. Lo que te protege pasa a ser
> el worktree aislado y git, no tu aprobación.

Es lo que sostiene una app sin notificaciones (TRD §13): si no hay permisos que
aprobar, no hace falta que nada te despierte. Merece decirse donde se activa.

### Nombre de máquina

Editable en Settings, con el hostname como valor inicial. Con varios Macs
emparejados, el móvil vería dos UUID si no.

### Gestión del dispositivo

En Settings: qué teléfono está emparejado, su nivel, cuándo se vio por última
vez, y **revocar**.

## Criterios de aceptación

1. Seleccionar desde el móvil marca el panel en el Mac en menos de un segundo.
2. Desconectar el teléfono quita la marca.
3. Cambiar el modo global afecta a las terminales que se abran después, y se dice
   en la interfaz que las abiertas siguen como estaban.
4. El nombre de máquina se edita y el móvil lo ve al reconectar.
5. Revocar deja el móvil sin acceso y la pantalla lo refleja.

## Riesgos

**Modo global mal entendido.** Alguien lo pone en yolo para una prueba y se le
queda. Mitigación: enseñarlo de forma permanente en la barra superior mientras
esté en yolo, no solo en Settings.

## Fuera de alcance

Todo lo del cliente.
