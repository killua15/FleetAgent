# Otro (servidor MCP) como fuente de conocimiento

La puerta para cualquier herramienta de documentación que **no sea Outline**: si tiene un servidor
MCP por HTTP, Hangar se lo pasa a los agentes. Hangar no sabe nada más de la herramienta: no la lee
por su cuenta, solo entrega el endpoint.

## Cómo se integra

1. **Buscar el endpoint MCP por HTTP** (streamable HTTP), por ejemplo `https://mcp.example.com/mcp`.
   En la documentación de la herramienta suele llamarse "remote MCP server".
2. **Crear un token, si lo pide**, en la propia herramienta (API key o personal access token). Hangar
   lo envía como `Authorization: Bearer <token>`.
3. **Conectar.** En Hangar: **Settings → Docs → Other (MCP server) → + Connect**, con nombre,
   endpoint y token.
4. **Verificación.**
   - Hangar envía un `initialize` de MCP y lee el `serverInfo.name` de la respuesta, venga en JSON o
     como evento SSE.
   - Un 401 o 403 significa token rechazado; un 404 o 405, que esa URL no es un endpoint MCP.
   - El token se guarda en el Keychain.
5. **Alcance y agentes.** Igual que Outline: proyectos elegidos, `--mcp-config` para Claude y
   compañía, `-c mcp_servers.*` para Codex, y el token como variable de entorno (ver
   [README](README.md)).

## Qué reciben los agentes

Las herramientas que exponga ese servidor, tal cual.

## Lo que todavía no hace

- **Servidores que solo permiten entrar por navegador (OAuth):** no se puede pegar un token
  estático. Llegará con el flujo OAuth de la fase 4.
- **Servidores locales que se arrancan como comando (stdio):** aún no se pueden añadir desde
  Settings → Docs.
- **Gemini y opencode:** todavía no reciben ninguna fuente (fase 3).
