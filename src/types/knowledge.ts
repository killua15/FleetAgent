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

/** Each provider, and how it plugs in — shown on its card in Settings → Docs
 * so whoever connects one knows what will happen before pasting a key. */
export const KNOWLEDGE_SOURCE_META: Record<
  KnowledgeSourceKind,
  {
    name: string;
    summary: string;
    /** How it integrates, step by step. */
    steps: string[];
    /** What an agent can do once it's connected. */
    agentsGet: string;
    /** What it doesn't do yet, said up front. */
    limits: string | null;
    urlLabel: string;
    urlPlaceholder: string;
    tokenRequired: boolean;
    tokenPlaceholder: string;
    hint: string;
  }
> = {
  outline: {
    name: 'Outline',
    summary: 'A team wiki, cloud (getoutline.com) or self-hosted — e.g. docs.example.com. Outline ships its own MCP server, so nothing else is installed.',
    steps: [
      'In Outline: Settings → API & Apps → New API key. Give it read and write access — with read only, agents can search but not write.',
      'Here: name it, paste the address you open Outline at (…/mcp or …/api work too) and the key.',
      'Hangar checks the key against Outline (auth.info) and shows whose it is. The key goes to the macOS keychain, never to the database.',
      'Choose every project or only some. Each terminal or session opened afterwards gets Outline’s MCP at <address>/mcp — Claude, Kimi, Custom and Grok through --mcp-config, Codex through -c mcp_servers.* — with the key as an environment variable, never on the command line.',
    ],
    agentsGet:
      'Outline’s own tools: full-text search, reading documents and collection trees, creating, editing, moving and restoring documents, comments, templates and attachments — limited to what the key is allowed.',
    limits: null,
    urlLabel: 'address',
    urlPlaceholder: 'https://docs.acme.com',
    tokenRequired: true,
    tokenPlaceholder: 'ol_api_…',
    hint: 'Your Outline instance (cloud or self-hosted) and a personal API key from Outline → Settings → API & Apps.',
  },
  mcp: {
    name: 'Other (MCP server)',
    summary: 'Any docs or knowledge tool that has an MCP server over HTTP — the door for whatever your team uses instead of Outline.',
    steps: [
      'Find the tool’s MCP endpoint over HTTP (streamable HTTP), e.g. https://mcp.example.com/mcp. Its docs usually call it the "remote MCP server" URL.',
      'If it wants a token, create one in that tool (API key or personal access token). It is sent as Authorization: Bearer <token>.',
      'Here: name it, paste the endpoint and the token. Hangar sends an MCP initialize to check that it answers and accepts the token, and stores the token in the keychain.',
      'Choose the projects. Agents get it exactly like Outline: Claude-family CLIs through --mcp-config, Codex through -c mcp_servers.*, the token as an environment variable.',
    ],
    agentsGet: 'Whatever tools that server exposes. Hangar can’t read the tool itself — it only hands the endpoint to your agents.',
    limits: 'Only servers that take a Bearer token. Servers that sign in only through a browser (OAuth) and local servers started as a command aren’t supported yet.',
    urlLabel: 'MCP endpoint',
    urlPlaceholder: 'https://mcp.example.com/mcp',
    tokenRequired: false,
    tokenPlaceholder: 'bearer token (if it wants one)',
    hint: 'The tool’s MCP endpoint over HTTP, and its token if it asks for one.',
  },
};
