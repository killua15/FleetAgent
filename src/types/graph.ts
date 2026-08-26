/** The code knowledge graph — see `GRAPHIFY_TRD.md`. */

/** Whether graphify is on this machine. Everything degrades to nothing when
 * it isn't: the screen still renders, it just says how to get it. */
export interface GraphifyStatus {
  installed: boolean;
  /** `graphify-mcp` ships in the same package, but a partial install would
   * build a graph that never reaches an agent — worth telling apart. */
  mcpInstalled: boolean;
  binaryPath: string | null;
  installHint: string;
}

/** What a graph was built over, for one member project. A group graph has one
 * of these per member; a project graph has exactly one. */
export interface GraphCommit {
  projectId: string;
  projectName: string;
  builtSha: string;
  /** `null` when the project has no git repo — which reads as *unknown*, not
   * as *old*. */
  currentSha: string | null;
  stale: boolean;
}

export type GraphStatus = 'never_built' | 'building' | 'ready' | 'failed';

export interface GraphState {
  scopeKind: 'project' | 'group';
  scopeId: string;
  status: GraphStatus;
  builtAt: string | null;
  nodeCount: number;
  edgeCount: number;
  lastError: string | null;
  commits: GraphCommit[];
  /** True when any member moved since the build. The graph stays readable —
   * agents are told its age rather than being cut off. */
  stale: boolean;
  /** graphify's own HTML, once a build has produced it. Opened in the system
   * browser, never embedded: it pulls vis-network from a CDN. */
  htmlPath: string | null;
  /** Whether this scope indexes documents as well as code. False everywhere
   * until someone asks for it: code extraction is local AST and sends nothing,
   * while indexing documents means their text reaches a model. */
  withDocs: boolean;
  /** The model that reads those documents. `null` means `defaultDocBackend`. */
  docBackend: string | null;
  /** What `null` resolves to, sent from Rust so this isn't hardcoded twice. */
  defaultDocBackend: string;
}

/** What a build would find in a checkout, asked before spending one.
 *
 * `codeFiles` stops counting at one — every caller only asks "any?" — so it is
 * a yes/no dressed as a number, and `otherFiles` is only meaningful when the
 * answer was no. */
export interface Indexable {
  codeFiles: number;
  otherFiles: number;
}

/** Backends graphify can read documents with, and what each one costs you.
 *
 * `claude-cli` is first because it is the only one that needs no new
 * credential: it drives the `claude` binary on the subscription Hangar already
 * manages. `ollama` is the only one where nothing leaves the machine. */
export const DOC_BACKENDS: { id: string; label: string; leaves: string }[] = [
  { id: 'claude-cli', label: 'Claude Code (tu suscripción)', leaves: 'Los documentos se envían a Anthropic, con tu cuenta y tu cuota.' },
  { id: 'ollama', label: 'Ollama (local)', leaves: 'Nada sale de la máquina. Requiere Ollama instalado con un modelo.' },
  { id: 'openai', label: 'OpenAI o compatible', leaves: 'Los documentos se envían al servidor configurado. Requiere OPENAI_API_KEY, o OPENAI_BASE_URL para uno local.' },
  { id: 'gemini', label: 'Gemini', leaves: 'Los documentos se envían a Google. Requiere clave de API.' },
];

/** Projects the user has marked as one system. */
export interface GraphGroup {
  id: string;
  name: string;
  projectCount: number;
  /** Shown even when zero — a group that bought nothing has to be visible in
   * the list, not discovered by opening the graph. `null` is "never built",
   * which is a different thing from a real zero. */
  bridgeCount: number | null;
}

export interface BridgeEnd {
  repo: string;
  nodeId: string;
  sourceFile: string;
}

/** A type name declared on both sides of the seam. `merge-graphs` produces no
 * edges between repos at all — these are what put the seam back. */
export interface Bridge {
  label: string;
  ends: BridgeEnd[];
}

export interface TurnSample {
  turns: number;
  avgInputTokens: number;
  avgOutputTokens: number;
  totalCostUsd: number;
  avgCostUsd: number;
}

/** Measured against the user's own turns, not asserted. */
export interface GraphSavingsReport {
  withGraph: TurnSample;
  withoutGraph: TurnSample;
  /** Negative when the graph costs more. `null` when one side has no turns. */
  inputDelta: number | null;
  costDeltaUsd: number | null;
  /** Either sample too small for the delta to mean anything. */
  tooEarly: boolean;
  /** Turns that predate headless rows recording a project, so they can't be
   * attributed to either side. Shown, not silently dropped. */
  unattributedTurns: number;
}

/** Another live session working the other side of a shared type. Needs the
 * graph *and* the fleet — the fleet is the half only Hangar has. */
export interface GraphCollision {
  sessionId: string;
  sessionName: string;
  branch: string | null;
  projectName: string;
  sharedType: string;
  files: string[];
}
