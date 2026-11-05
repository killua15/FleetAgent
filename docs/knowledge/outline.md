# Outline como fuente de conocimiento

Outline (getoutline.com, o autoalojado como **docs.example.com**) trae **su propio servidor
MCP** en `<dirección>/mcp`. Hangar no instala nada: verifica la key y le pasa ese servidor a los
agentes.

## Lo que se comprobó de la instancia

- **Autenticación de la ruta `/mcp`:** acepta MCP-OAuth, OAuth y **API key personal**
  (`server/routes/mcp/index.ts` en outline/outline). Por eso una sola key sirve para Hangar y para
  cualquier CLI.
- **Login web de docs.example.com:** Microsoft (Azure) y Passkeys. La key se crea después de
  entrar.

## Cómo se integra

1. **Crear la key.** En Outline: **Settings → API & Apps → New API key**.
   - Con lectura y escritura, los agentes pueden buscar y también escribir documentos y comentarios.
   - Con solo lectura, solo buscan y leen.
2. **Conectar.** En Hangar: **Settings → Docs → Outline → + Connect**.
   - **Nombre:** sale como nombre del servidor MCP para los agentes, por ejemplo `team-docs`.
   - **Dirección:** `https://docs.example.com`. Si pegas `…/mcp` o `…/api`, se normaliza.
   - **API key:** la que acabas de crear.
3. **Verificación.**
   - Hangar llama a `POST /api/auth.info`. Un 401 significa key inválida; un 404, una dirección que
     no es Outline.
   - Muestra a quién pertenece la key ("as David").
   - Guarda la key en el Keychain de macOS.
4. **Alcance.** Para todos los proyectos o solo algunos.
5. **Agentes.** Cada terminal o sesión que se abra después recibe el servidor
   `https://docs.example.com/mcp`, con la key en el entorno del proceso (ver
   [README](README.md)).

## Qué reciben los agentes

Las herramientas MCP de Outline. Las que aparecen dependen de lo que permita la key:

| Grupo | Herramientas |
|---|---|
| Documentos | `list_documents` (búsqueda de texto completo, o recientes), `list_collection_documents`, `create_document`, `update_document`, `move_document`, `delete_document`, `restore_document` |
| Colecciones | `list_collections`, `create_collection`, `update_collection`, `delete_collection` |
| Comentarios | `list_comments`, `create_comment`, `update_comment`, `delete_comment` |
| Otros | `fetch` (documento, colección con su árbol, usuario, adjunto con URL firmada, plantilla), `list_users`, `list_templates`, `create_attachment` |

Además, Outline sirve **skills** por MCP: `find-and-cite`, `collection-digest`, `meeting-notes` y
`capture-conversation`.

## Próximas fases

| Fase | Contenido | API de Outline |
|---|---|---|
| 2 | Panel 📚 Docs junto a las terminales; documentos relacionados en el brief de cada tarea; colección por proyecto | `documents.search` |
| 3 | "Publicar en la wiki" desde una sesión o un PR; menciones y comentarios en el centro de notificaciones | `documents.create`, `notifications.list` |
| 4 | Login por OAuth con registro dinámico (`/oauth/register`, PKCE), sin pegar la key | — |
