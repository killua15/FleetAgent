export interface ProviderAvailability {
  provider: string;
  /** Its CLI is on PATH. Fixable by the user: go and install it. */
  available: boolean;
  /** Hangar can run it as a specific account. Not fixable by the user — it's
   * a gap in Hangar, so the wizard says so rather than implying otherwise. */
  supported: boolean;
}

export interface ApiKeyCheck {
  valid: boolean;
  message: string;
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
