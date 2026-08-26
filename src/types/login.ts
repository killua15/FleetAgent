export interface ProviderAvailability {
  provider: string;
  available: boolean;
  /** Whether step 3 can offer logins already on this machine. Comes from Rust
   * so the two sides cannot disagree — a wizard that offers adoption while the
   * backend returns nothing is a dead end with no way forward. */
  canAdoptLogin: boolean;
  /** Whether an account can be created with no credential at all. True only for
   * opencode, which ships free models and genuinely runs without one. */
  keyOptional: boolean;
}

export interface ApiKeyCheck {
  valid: boolean;
  message: string;
  /** Hangar has no way to check this provider's key — which is not the same as
   * the key being wrong. Treating the two alike is what made every non-Claude
   * key-billed account impossible to create. */
  unverifiable: boolean;
}

export interface LoginSession {
  loginId: string;
  configDir: string;
}

export interface AuthStatus {
  loggedIn: boolean;
  authMethod: string;
  /** Both come from `claude auth status --json`. `null` when the CLI omits
   * them, e.g. a team login with no bound email. */
  email: string | null;
  subscriptionType: string | null;
}

export interface LoginOutputEvent {
  loginId: string;
  text: string;
}
