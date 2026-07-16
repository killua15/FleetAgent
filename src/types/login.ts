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
}

export interface LoginOutputEvent {
  loginId: string;
  text: string;
}
