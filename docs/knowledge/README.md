# Fuentes de conocimiento (wikis para los agentes)

Una **fuente de conocimiento** es donde el equipo guarda lo que sabe —una wiki, un sitio de docs— y
que Hangar entrega a **todos los agentes** que abre, por MCP. Se configuran en **Settings → Docs**.

Cada proveedor tiene su documento con cómo se integra:

| Proveedor | Documento | Qué lee Hangar | Qué reciben los agentes |
|---|---|---|---|
| Outline | [outline.md](outline.md) | REST (`auth.info`) para verificar la key | El MCP propio de Outline (`<dirección>/mcp`) |
| Otro (servidor MCP) | [mcp.md](mcp.md) | `initialize` MCP para verificar | Las herramientas que exponga ese servidor |

## Cómo llega a los agentes (común a todos)

- **Un fichero `--mcp-config` por proyecto** en `<app data>/agent-mcp/<proyecto>.json`, con graphify
  (si hay grafo) y cada fuente como entrada `http`. Para Claude, Kimi, Custom y Grok.
- **Codex** recibe las mismas fuentes como `-c mcp_servers.<nombre>.url=…` y
  `-c mcp_servers.<nombre>.bearer_token_env_var=…`.
- **El token nunca va en la línea de comandos ni en el fichero**:
  - viaja como variable de entorno del proceso (`HANGAR_MCP_<NOMBRE>_TOKEN`);
  - el fichero solo lleva `Authorization: Bearer ${HANGAR_MCP_<NOMBRE>_TOKEN}`;
  - verificado en Claude Code 2.1.308 y Codex 0.147.0 con un servidor local que registraba las
    cabeceras.
- **Es aditivo:** Hangar nunca pasa `--strict-mcp-config`, así que los servidores MCP propios del
  usuario siguen activos.
- **Alcance:** gemini y opencode todavía no lo reciben (fase 3).
- Una terminal ya abierta no lo recibe: hay que abrir una nueva.
- **"WHAT HANGAR NEEDS"** muestra la fila "Team wiki for agents": ✓ si hay alguna fuente conectada,
  y si no, un botón a Settings → Docs.

## Añadir un proveedor nuevo (para desarrolladores)

1. **Un módulo en `src-tauri/src/knowledge_sources/<kind>.rs`** con `normalize_*` y `verify()`, como
   `outline.rs`.
2. **Un brazo en `save_knowledge_source`**, el `match kind`, que verifique y construya `config`.
3. **`mcp_url_for`**: dónde vive su MCP, a partir de la dirección guardada.
4. **Su entrada en `KNOWLEDGE_SOURCE_META`** (`src/types/knowledge.ts`) con `summary`, `steps`,
   `agentsGet` y `limits`. Es lo que ve el usuario en su tarjeta.
5. **Su documento aquí**, con el mismo formato que los demás.

`kind` no tiene CHECK en la tabla `knowledge_sources`, así que un proveedor nuevo no necesita
migración.
