export interface ProviderAvailability {
  provider: string;
  available: boolean;
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
