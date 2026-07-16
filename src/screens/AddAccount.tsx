import { useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useNavigation } from '../nav/NavigationProvider';
import { extractUrl, stripAnsi } from '../lib/ansi';
import { PROVIDER_META, type Billing, type NewAccount, type Provider, type Scope } from '../types/account';
import type { ApiKeyCheck, AuthStatus, LoginOutputEvent, LoginSession, ProviderAvailability } from '../types/login';
import { Button } from '../components/ui/button';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { RadioGroup, RadioGroupItem } from '../components/ui/radio-group';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import styles from './AddAccount.module.css';

const STEP_LABELS = ['provider', 'billing', 'login', 'details'] as const;
const PROVIDERS: Provider[] = ['claude', 'codex', 'gemini'];
const STATUS_POLL_MS = 1500;

export function AddAccount() {
  const { back } = useNavigation();

  const [availability, setAvailability] = useState<Record<string, boolean>>({});
  const [step, setStep] = useState(1);
  const [provider, setProvider] = useState<Provider>('claude');
  const [billing, setBilling] = useState<Billing>('sub');
  const [name, setName] = useState('');
  const [scope, setScope] = useState<Scope>('work');
  const [monthlyCap, setMonthlyCap] = useState('150');
  const [done, setDone] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // --- subscription login state ---
  const [loginSession, setLoginSession] = useState<LoginSession | null>(null);
  const [terminalText, setTerminalText] = useState('');
  const [pasteCode, setPasteCode] = useState('');
  const [loggedIn, setLoggedIn] = useState(false);
  const [loginError, setLoginError] = useState<string | null>(null);
  const [startingLogin, setStartingLogin] = useState(false);
  const loginStartedRef = useRef(false);
  const pollRef = useRef<number | null>(null);
  const terminalRef = useRef<HTMLDivElement>(null);

  // --- api key state ---
  const [apiKey, setApiKey] = useState('');
  const [keyCheck, setKeyCheck] = useState<ApiKeyCheck | null>(null);
  const [checkingKey, setCheckingKey] = useState(false);

  const prov = PROVIDER_META[provider];

  useEffect(() => {
    invoke<ProviderAvailability[]>('list_available_providers').then((list) => {
      const map: Record<string, boolean> = {};
      list.forEach((p) => (map[p.provider] = p.available));
      setAvailability(map);
    });
  }, []);

  const stopPolling = () => {
    if (pollRef.current !== null) {
      window.clearInterval(pollRef.current);
      pollRef.current = null;
    }
  };

  const cancelPendingLogin = (loginId: string | undefined) => {
    stopPolling();
    if (loginId) invoke('cancel_login', { loginId }).catch(() => {});
  };

  // Kick off the real embedded CLI login once we land on step 3 for a
  // subscription account.
  useEffect(() => {
    if (step !== 3 || billing !== 'sub' || loginStartedRef.current) return;
    loginStartedRef.current = true;
    setStartingLogin(true);
    setLoginError(null);
    invoke<LoginSession>('begin_login', { provider })
      .then((session) => {
        setLoginSession(session);
        pollRef.current = window.setInterval(async () => {
          try {
            const status = await invoke<AuthStatus>('check_login_status', { loginId: session.loginId });
            if (status.loggedIn) {
              setLoggedIn(true);
              stopPolling();
            }
          } catch {
            // session may have been cancelled — stop quietly
            stopPolling();
          }
        }, STATUS_POLL_MS);
      })
      .catch((e) => setLoginError(String(e)))
      .finally(() => setStartingLogin(false));

    return () => {
      // Only fires when this effect's deps change while step===3 stays
      // true would not re-run since loginStartedRef guards it; the real
      // cleanup path is handled explicitly in back()/close() below.
    };
  }, [step, billing, provider]);

  useEffect(() => {
    if (!loginSession) return;
    const unlisten = listen<LoginOutputEvent>('login-output', (event) => {
      if (event.payload.loginId !== loginSession.loginId) return;
      setTerminalText((prev) => prev + event.payload.text);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [loginSession]);

  useEffect(() => {
    terminalRef.current?.scrollTo({ top: terminalRef.current.scrollHeight });
  }, [terminalText]);

  useEffect(() => stopPolling, []);

  const resetLoginState = () => {
    loginStartedRef.current = false;
    setLoginSession(null);
    setTerminalText('');
    setPasteCode('');
    setLoggedIn(false);
    setLoginError(null);
  };

  const handleClose = () => {
    if (!done) cancelPendingLogin(loginSession?.loginId);
    back();
  };

  const handleBack = () => {
    if (step === 3) {
      cancelPendingLogin(loginSession?.loginId);
      resetLoginState();
    }
    setStep(step - 1);
  };

  const handleSubmitCode = async () => {
    if (!loginSession || !pasteCode.trim()) return;
    await invoke('send_login_input', { loginId: loginSession.loginId, data: pasteCode.trim() });
    setPasteCode('');
  };

  const handleVerifyKey = async () => {
    if (!apiKey.trim() || checkingKey) return;
    setCheckingKey(true);
    try {
      const result = await invoke<ApiKeyCheck>('validate_api_key', { provider, apiKey: apiKey.trim() });
      setKeyCheck(result);
    } catch (e) {
      setKeyCheck({ valid: false, message: String(e) });
    } finally {
      setCheckingKey(false);
    }
  };

  const keyValid = keyCheck?.valid === true;
  const canNext =
    step === 1
      ? availability[provider] !== false
      : step === 2
        ? true
        : step === 3
          ? (billing === 'sub' ? loggedIn : keyValid)
          : true;

  const handleNext = async () => {
    if (!canNext || saving) return;
    if (step < 4) {
      setStep(step + 1);
      return;
    }
    setSaving(true);
    setError(null);
    const account: NewAccount = {
      provider,
      billing,
      name: name.trim() || `${prov.name} account`,
      scope,
      planLabel: `${prov.name} · ${billing === 'sub' ? 'Subscription' : 'API key'}`,
      secret: billing === 'key' ? apiKey.trim() : undefined,
      loginId: billing === 'sub' ? loginSession?.loginId : undefined,
      monthlyCap: monthlyCap ? Number(monthlyCap.replace(/[^0-9.]/g, '')) : undefined,
    };
    try {
      await invoke('create_account', { account });
      setDone(true);
      window.setTimeout(() => back(), 700);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const url = extractUrl(terminalText);

  return (
    <div className={styles.screen}>
      <div className={styles.card}>
        <div className={styles.header}>
          <span className={styles.headerTitle}>Add account</span>
          <span className={styles.spacer} />
          <div className={styles.steps}>
            {STEP_LABELS.map((label, i) => {
              const n = i + 1;
              const active = step === n;
              const past = step > n;
              return (
                <span className={styles.step} key={label}>
                  <span
                    className={styles.stepDot}
                    style={{
                      background: active ? 'var(--acc)' : past ? 'var(--accD)' : 'var(--panel2)',
                      color: active ? '#15151c' : past ? 'var(--acc)' : 'var(--dim)',
                      borderColor: active || past ? 'var(--acc)' : 'var(--line)',
                    }}
                  >
                    {past ? '✓' : n}
                  </span>
                  <span className={styles.stepLabel} style={{ color: active ? 'var(--acc)' : 'var(--dim)' }}>
                    {label}
                  </span>
                </span>
              );
            })}
          </div>
          <Button variant="ghost" size="icon" className={styles.closeBtn} onClick={handleClose} title="Cancel">
            ✕
          </Button>
        </div>

        <div className={styles.body}>
          {step === 1 && (
            <>
              <span className={styles.hint}>Which provider is this account for?</span>
              <RadioGroup
                className={styles.providerGrid}
                value={provider}
                onValueChange={(value) => setProvider(value as Provider)}
              >
                {PROVIDERS.map((p) => {
                  const meta = PROVIDER_META[p];
                  const isAvailable = availability[p] !== false;
                  return (
                    <Label
                      key={p}
                      className={`${styles.providerCard} ${provider === p ? styles.selected : ''}`}
                      style={!isAvailable ? { opacity: 0.45, cursor: 'not-allowed' } : undefined}
                    >
                      <RadioGroupItem value={p} disabled={!isAvailable} className="sr-only" />
                      <span
                        className={styles.providerIcon}
                        style={{ background: meta.tintVar, color: meta.colorVar }}
                      >
                        {meta.letter}
                      </span>
                      <span className={styles.providerName}>{meta.name}</span>
                      <span className={styles.providerCli}>{meta.cli}</span>
                      {!isAvailable && (
                        <span style={{ font: '500 9px var(--font-mono)', color: 'var(--red)' }}>not installed</span>
                      )}
                    </Label>
                  );
                })}
              </RadioGroup>
            </>
          )}

          {step === 2 && (
            <>
              <span className={styles.hint}>
                How does <span style={{ color: 'var(--acc)', fontWeight: 600 }}>{prov.name}</span> bill this
                account?
              </span>
              <RadioGroup
                className={styles.billingGrid}
                value={billing}
                onValueChange={(value) => setBilling(value as Billing)}
              >
                <Label className={`${styles.card2} ${billing === 'sub' ? styles.selected : ''}`}>
                  <RadioGroupItem value="sub" className="sr-only" />
                  <span className={styles.billingName}>Subscription</span>
                  <span className={styles.billingDesc}>
                    Login via browser/CLI. Fleet tracks the 5h session + weekly windows.
                  </span>
                </Label>
                <Label className={`${styles.card2} ${billing === 'key' ? styles.selected : ''}`}>
                  <RadioGroupItem value="key" className="sr-only" />
                  <span className={styles.billingName}>API key</span>
                  <span className={styles.billingDesc}>
                    Pay per token. Fleet tracks accumulated spend and enforces your cap.
                  </span>
                </Label>
              </RadioGroup>
            </>
          )}

          {step === 3 && billing === 'sub' && (
            <>
              <span className={styles.hint}>Complete the CLI login — Fleet runs it in an embedded terminal:</span>
              <div className={styles.terminal} ref={terminalRef} style={{ maxHeight: 220, overflow: 'auto' }}>
                {startingLogin && !terminalText && <span style={{ color: 'var(--dim)' }}>starting {prov.loginCmd}…</span>}
                {loginError && <span style={{ color: 'var(--red)' }}>{loginError}</span>}
                <span style={{ whiteSpace: 'pre-wrap' }}>{stripAnsi(terminalText)}</span>
                {!loggedIn && !loginError && <span className={styles.cursor} />}
                {loggedIn && (
                  <>
                    <br />
                    <span style={{ color: 'var(--grn)' }}>✓ Logged in</span>
                  </>
                )}
              </div>
              {!loggedIn && url && (
                <Button variant="outline" className={styles.simulateBtn} onClick={() => openUrl(url)}>
                  Open sign-in page ↗
                </Button>
              )}
              {!loggedIn && loginSession && (
                <div style={{ display: 'flex', gap: 8 }}>
                  <Input
                    className={styles.input}
                    style={{ flex: 1 }}
                    value={pasteCode}
                    onChange={(e) => setPasteCode(e.target.value)}
                    placeholder="paste code here if prompted"
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') handleSubmitCode();
                    }}
                  />
                  <Button variant="outline" className={styles.simulateBtn} onClick={handleSubmitCode}>
                    Submit code
                  </Button>
                </div>
              )}
            </>
          )}

          {step === 3 && billing === 'key' && (
            <>
              <span className={styles.hint}>
                Paste the API key — stored in the OS keychain, never in config files:
              </span>
              <div style={{ display: 'flex', gap: 8 }}>
                <Input
                  className={styles.input}
                  style={{ flex: 1 }}
                  value={apiKey}
                  onChange={(e) => {
                    setApiKey(e.target.value);
                    setKeyCheck(null);
                  }}
                  placeholder="sk-…"
                />
                <Button
                  variant="outline"
                  className={styles.simulateBtn}
                  disabled={!apiKey.trim() || checkingKey}
                  onClick={handleVerifyKey}
                >
                  {checkingKey ? 'Verifying…' : 'Verify'}
                </Button>
              </div>
              {keyCheck && (
                <span style={{ color: keyCheck.valid ? 'var(--grn)' : 'var(--red)', font: '500 11px var(--font-mono)' }}>
                  {keyCheck.valid ? `✓ ${keyCheck.message}` : `✕ ${keyCheck.message}`}
                </span>
              )}
            </>
          )}

          {step === 4 && (
            <>
              <span className={styles.hint}>Name it so you can tell accounts apart in the sidebar:</span>
              <div className={styles.field}>
                <Label className={styles.label}>ACCOUNT NAME</Label>
                <Input
                  className={styles.nameInput}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder={`${prov.name} account`}
                />
              </div>
              <div className={styles.fieldRow}>
                <div className={styles.field}>
                  <Label className={styles.label}>SCOPE</Label>
                  <Select value={scope} onValueChange={(value) => setScope(value as Scope)}>
                    <SelectTrigger className={styles.select}>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="work">work</SelectItem>
                      <SelectItem value="personal">personal</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div className={styles.field}>
                  <Label className={styles.label}>MONTHLY SPEND CAP</Label>
                  <Input
                    className={styles.nameInput}
                    value={monthlyCap}
                    onChange={(e) => setMonthlyCap(e.target.value)}
                    placeholder="$150"
                  />
                </div>
              </div>
              <div className={styles.note}>
                No project can use this account until you allow it in{' '}
                <span className={styles.noteAccent}>Settings → Account rotation</span> — accounts start fully
                isolated.
              </div>
              {error && <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>{error}</span>}
            </>
          )}
        </div>

        <div className={styles.footer}>
          {step > 1 && !done && (
            <Button variant="outline" className={styles.backBtn} onClick={handleBack}>
              ← Back
            </Button>
          )}
          <span className={styles.spacer} />
          <Button
            className={`${styles.nextBtn} ${done ? styles.done : ''}`}
            disabled={!canNext || saving}
            onClick={handleNext}
          >
            {done ? '✓ Account added' : saving ? 'Saving…' : step === 4 ? 'Add account' : 'Continue →'}
          </Button>
        </div>
      </div>
    </div>
  );
}
