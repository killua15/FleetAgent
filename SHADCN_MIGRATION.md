# Migración de UI a shadcn/ui

> Tarea pendiente de implementación. Este documento sirve como spec/checklist para cuando se aborde el trabajo.

## Problema

Fleet-Agent es una app Tauri + React 19 cuya UI está hecha 100% a mano: CSS Modules por pantalla/componente
(`*.module.css`) más tokens propios en `src/styles/tokens.css` (dark/light vía `:root[data-theme]`) y
`src/theme/ThemeProvider.tsx`. No hay ninguna librería de componentes. Mantener botones, inputs, selects,
cards, badges, etc. a mano implica reinventar accesibilidad, estados (focus/disabled/hover) y consistencia
visual en cada pantalla nueva.

Se quiere reescribir todos los componentes usando **shadcn/ui** (https://ui.shadcn.com/docs/components).

## Goals

- Instalar y configurar shadcn/ui (Tailwind CSS + `components.json` + CLI) en el proyecto Vite/React/Tauri existente.
- Mapear los tokens actuales de `tokens.css` (`--bg`, `--panel`, `--acc`, `--txt`, `--mut`, `--line`, colores de
  estado `--teal/--amber/--red/--grn`, `--gemini`) al sistema de variables de shadcn (`--background`,
  `--foreground`, `--primary`, `--muted`, `--border`, `--destructive`, etc.), preservando el dark/light toggle
  actual (`ThemeProvider` + `data-theme`).
- Reemplazar los elementos nativos HTML (`<button>`, `<input>`, `<select>`, `<textarea>`, `<label>`) por los
  componentes shadcn equivalentes en las 5 pantallas y 3 componentes existentes.
- Mantener toda la funcionalidad y el layout actual — esto es una migración visual/estructural, no una
  reescritura de lógica de negocio.

## Non-goals

- No se cambia el flujo de navegación (`NavigationProvider`), el estado de negocio, ni las llamadas a Tauri
  (`@tauri-apps/plugin-dialog`, etc.).
- No se rediseña la identidad visual (paleta de colores) — se portan los tokens existentes, no se adopta la
  paleta por defecto de shadcn.
- No se migra a Next.js ni se cambia el bundler (sigue siendo Vite).

## Inventario actual (para referencia al migrar)

| Archivo | Elementos a migrar |
|---|---|
| `src/components/AppTopBar.tsx` | botones de la barra superior |
| `src/components/ErrorOverlay.tsx` | overlay de error → candidato a `Dialog`/`Alert` |
| `src/components/Placeholder.tsx` | trivial, sin inputs |
| `src/screens/Landing.tsx` | botones de entrada |
| `src/screens/AccountList.tsx` | lista de cuentas, botones de acción |
| `src/screens/AddAccount.tsx` (432 líneas) | formulario grande: inputs, selects, labels, "provider cards"
  seleccionables (`styles.providerCard` / `.selected`) → candidato a `RadioGroup`/`Card` seleccionable |
| `src/screens/AgentPanel.tsx` (390 líneas) | inputs, selects, checkboxes de módulos (`.moduleCheckbox`) →
  candidato a `Checkbox` |
| `src/screens/MainView.tsx` (762 líneas) | el más grande: `SessionPanelCard` (→ `Card`), badges de proveedor
  (`.accountBadge`, `.provBadgeSm`) (→ `Badge`), diálogo nativo de carpeta vía Tauri (mantener, no es UI web) |

Conteo global de elementos nativos a reemplazar: 47 `<button>`, 12 `<label>`, 9 `<select>`, 9 `<input>`,
1 `<textarea>`.

## Approach

1. **Setup**
   - Instalar Tailwind CSS (`tailwindcss`, `postcss`, `autoprefixer`) y configurarlo para Vite.
   - Ejecutar `npx shadcn@latest init`, apuntando `components.json` a `src/components/ui` para no chocar con
     los componentes propios existentes en `src/components`.
   - Portar los valores de `tokens.css` a las variables CSS que shadcn espera en `src/styles/global.css`
     (bloques `:root` y `.dark` / `[data-theme='light']`), en vez de sustituir el archivo de tokens.
2. **Componentes base a instalar vía CLI** (según inventario): `button`, `input`, `label`, `select`,
   `textarea`, `checkbox`, `radio-group`, `card`, `badge`, `dialog`, `alert`.
3. **Migración pantalla por pantalla** (orden sugerido, de más simple a más compleja):
   `Placeholder` → `Landing` → `AppTopBar` → `ErrorOverlay` → `AccountList` → `AgentPanel` → `AddAccount` →
   `MainView`.
   - Por cada pantalla: sustituir elementos nativos por el componente shadcn equivalente, eliminar las clases
     CSS Module que ya no hagan falta, conservar solo el CSS de layout que shadcn no cubre.
4. **Theming**: verificar que `ThemeProvider.tsx` siga alternando `data-theme` correctamente y que los
  componentes shadcn (que usan la convención `class="dark"` por defecto) queden mapeados al atributo
  `data-theme` ya usado por el proyecto (ajustar `tailwind.config` con `darkMode: ['selector', '[data-theme="dark"]']`
  o equivalente).
5. **Limpieza**: eliminar CSS Modules que queden vacíos tras la migración.

## Acceptance criteria

- Tailwind + shadcn instalados y funcionando (`npm run dev` sin errores).
- Las 5 pantallas y los 3 componentes usan componentes de `src/components/ui` (shadcn) en vez de elementos
  HTML nativos estilizados a mano, donde exista un componente shadcn equivalente.
- El toggle de tema (`Cmd/Ctrl+Shift+L`) sigue funcionando y los colores de marca (`--acc`, `--teal`,
  `--amber`, `--red`, `--grn`, `--gemini`) se conservan visualmente en ambos temas.
- No se rompe ninguna funcionalidad existente (navegación, alta de cuentas, panel de agente, diálogo de
  selección de carpeta vía Tauri).
- `npm run build` (`tsc && vite build`) pasa sin errores de tipos.

## Open questions

- ¿Se quiere adoptar Tailwind de forma completa (utilidades en JSX) o mantener CSS Modules para layout y usar
  shadcn solo para los componentes interactivos (botones, inputs, etc.)?
- ¿Los "provider cards" seleccionables de `AddAccount.tsx` deben migrar a `RadioGroup` con `Card` custom, o
  existe preferencia por otro patrón (`ToggleGroup`)?
- ¿Prioridad de pantallas? (el approach asume `MainView` al final por ser la más grande/riesgosa).
