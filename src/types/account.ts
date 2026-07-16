export type Provider = 'claude' | 'codex' | 'gemini';
export type Billing = 'sub' | 'key';
export type Scope = 'work' | 'personal';
export type AccountStatus = 'healthy' | 'cooling_down' | 'exhausted';

export interface Account {
  id: string;
  provider: Provider;
  billing: Billing;
  name: string;
  scope: Scope;
  planLabel: string;
  configDirPath: string;
  credentialRef: string;
  monthlyCap: number | null;
  status: AccountStatus;
  createdAt: string;
  updatedAt: string;
}

export interface NewAccount {
  provider: Provider;
  billing: Billing;
  name: string;
  scope: Scope;
  planLabel: string;
  secret?: string;
  loginId?: string;
  monthlyCap?: number;
}

export const PROVIDER_META: Record<
  Provider,
  { name: string; cli: string; letter: string; colorVar: string; tintVar: string; loginCmd: string }
> = {
  claude: {
    name: 'Claude',
    cli: 'claude code',
    letter: 'C',
    colorVar: 'var(--acc)',
    tintVar: 'var(--accD)',
    loginCmd: 'claude auth login',
  },
  codex: {
    name: 'Codex',
    cli: 'codex cli',
    letter: 'X',
    colorVar: 'var(--teal)',
    tintVar: 'var(--tealD)',
    loginCmd: 'codex login',
  },
  gemini: {
    name: 'Gemini',
    cli: 'gemini cli',
    letter: 'G',
    colorVar: 'var(--gemini)',
    tintVar: 'var(--geminiD)',
    loginCmd: 'gemini auth',
  },
};
