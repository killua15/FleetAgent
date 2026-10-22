/** A team wiki or other knowledge tool, as `knowledge_sources` keeps it.
 * Matches `KnowledgeSourceView` in Rust. The token never comes back. */
export interface KnowledgeSource {
  id: string;
  kind: KnowledgeSourceKind;
  label: string;
  baseUrl: string;
  /** Where agents reach it. */
  mcpUrl: string;
  hasCredential: boolean;
  config: Record<string, unknown>;
  allProjects: boolean;
  projectIds: string[];
}

export type KnowledgeSourceKind = 'outline' | 'mcp';

export const KNOWLEDGE_SOURCE_META: Record<
  KnowledgeSourceKind,
  { name: string; urlLabel: string; urlPlaceholder: string; tokenRequired: boolean; tokenPlaceholder: string; hint: string }
> = {
  outline: {
    name: 'Outline',
    urlLabel: 'address',
    urlPlaceholder: 'https://docs.acme.com',
    tokenRequired: true,
    tokenPlaceholder: 'ol_api_…',
    hint: 'Your Outline instance (cloud or self-hosted) and a personal API key from Outline → Settings → API & Apps. Give the key read and write access if agents should be able to write docs too.',
  },
  mcp: {
    name: 'Other (MCP server)',
    urlLabel: 'MCP endpoint',
    urlPlaceholder: 'https://mcp.example.com/mcp',
    tokenRequired: false,
    tokenPlaceholder: 'bearer token (if it wants one)',
    hint: 'Any wiki or docs tool with an MCP server over HTTP — Notion, Confluence, BookStack… Hangar hands it to your agents; the token is sent as a Bearer header.',
  },
};
