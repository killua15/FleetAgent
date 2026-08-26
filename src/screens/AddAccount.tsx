import { useEffect, useRef, useState } from 'react';
import { invoke } from '../lib/invoke';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { useNavigation } from '../nav/NavigationProvider';
import { extractUrl, stripAnsi } from '../lib/ansi';
import { KEY_ONLY_PROVIDERS, LOGIN_EXPECTS_CODE, PROVIDER_META, type Billing, type DiscoveredLogin, type NewAccount, type Provider, type Scope } from '../types/account';
import type { ApiKeyCheck, AuthStatus, LoginOutputEvent, LoginSession, ProviderAvailability } from '../types/login';
import { Button } from '../components/ui/button';
import { Input } from '../components/ui/input';
import { Spinner } from '../components/Spinner';
import { Label } from '../components/ui/label';
import { RadioGroup, RadioGroupItem } from '../components/ui/radio-group';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import styles from './AddAccount.module.css';

const STEP_LABELS = ['provider', 'billing', 'login', 'details'] as const;
/** Derived from the metadata rather than listed again: a provider added to
 * `PROVIDER_META` and forgotten here simply never appears in the wizard, which
 * is how Kimi shipped invisible. */
const PROVIDERS = Object.keys(PROVIDER_META) as Provider[];
const STATUS_POLL_MS = 1500;
/** Consecutive failed checks before giving up. Several, because the check
 * spawns a CLI and one slow answer is not the same as a broken login. */
const MAX_POLL_FAILURES = 5;

export function AddAccount() {
  const { back } = useNavigation();

  /** Keyed by provider. Absent while the check is in flight, which reads as
   * "usable" everywhere below — a provider is never greyed out on the strength
   * of an answer that hasn't arrived. */
  const [availability, setAvailability] = useState<Record<string, ProviderAvailability>>({});
  const [step, setStep] = useState(1);
  const [provider, setProviderState] = useState<Provider>('claude');
  const [billing, setBilling] = useState<Billing>('sub');
  /** Picking a key-only provider settles the billing question, so it's answered
   * here instead of leaving the wizard on a choice whose only option is
   * hidden. */
  const setProvider = (next: Provider) => {
    setProviderState(next);
    if (KEY_ONLY_PROVIDERS.includes(next)) setBilling('key');
  };

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
  const [submittingCode, setSubmittingCode] = useState(false);
  // Set when the user adopts a config dir they already logged into by hand
  // instead of running a fresh login here. Takes the place of `loginId`.
  const [importedConfigDir, setImportedConfigDir] = useState<string | null>(null);
  // Existing logins on this machine, shown first on step 3 (sub). `null` while
  // still loading. Importing one is the one-click happy path.
  const [discovered, setDiscovered] = useState<DiscoveredLogin[] | null>(null);
  // The user chose "log in a new account" over importing; the embedded login
  // form (name → browser login) takes over.
  const [wantsNewLogin, setWantsNewLogin] = useState(false);
  const [newLoginName, setNewLoginName] = useState('');
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
      const map: Record<string, ProviderAvailability> = {};
      list.forEach((p) => (map[p.provider] = p));
      setAvailability(map);
    });
  }, []);

  // Every account's config dir now lives at `~/.<slugified name>` (see
  // `login_manager.rs`'s `account_config_dir`) — a name already in use by
  // another account would collide on that exact directory, so it has to be
  // rejected before a login even starts, not discovered after.
  const [existingNames, setExistingNames] = useState<string[]>([]);
  useEffect(() => {
    invoke<Array<{ name: string }>>('list_accounts').then((accounts) => setExistingNames(accounts.map((a) => a.name)));
  }, []);
  const nameTaken = (candidate: string) => {
    const trimmed = candidate.trim().toLowerCase();
    return trimmed !== '' && existingNames.some((n) => n.trim().toLowerCase() === trimmed);
  };

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

  /** Si este proveedor puede reutilizar un login que ya esté en la máquina.
   *
   * Lo decide Rust, no esta pantalla: repetir aquí la lista de proveedores es
   * cómo se vuelve a este mismo fallo — el asistente ofreciendo adopción y el
   * backend devolviendo vacío, con el usuario en una pantalla sin salida.
   *
   * Mientras la respuesta no ha llegado se asume que sí, que es el
   * comportamiento de siempre para Claude y evita un parpadeo al formulario. */
  const canAdoptLogin = availability[provider]?.canAdoptLogin ?? true;

  // Starts the real embedded CLI login. No longer auto-fired on entering
  // step 3: importing an existing login is the default there, and a genuinely
  // new login now runs in `~/.<name>` — so its directory is named after
  // the account, which means the name has to be known *before* the login
  // starts (the directory can't be renamed afterwards without breaking the
  // keychain token).
  const startNewLogin = (accountName: string) => {
    if (loginStartedRef.current) return;
    loginStartedRef.current = true;
    setStartingLogin(true);
    setLoginError(null);
    // Carry the chosen name into step 4, and into the account created there.
    setName(accountName);
    invoke<LoginSession>('begin_login', { provider, name: accountName })
      .then((session) => {
        setLoginSession(session);
        // A single failed poll used to stop the loop for good, silently — so
        // one transient error (a slow CLI, a status command that hiccups) left
        // the wizard waiting forever for a state it had stopped looking for.
        // Now it keeps looking, and gives up loudly rather than quietly.
        let consecutiveFailures = 0;
        pollRef.current = window.setInterval(async () => {
          try {
            const status = await invoke<AuthStatus>('check_login_status', { loginId: session.loginId });
            consecutiveFailures = 0;
            setLoginError(null);
            if (status.loggedIn) {
              setLoggedIn(true);
              stopPolling();
            }
          } catch (e) {
            consecutiveFailures += 1;
            if (consecutiveFailures >= MAX_POLL_FAILURES) {
              stopPolling();
              setLoginError(`Couldn't tell whether the login finished: ${e}`);
            }
          }
        }, STATUS_POLL_MS);
      })
      .catch((e) => setLoginError(String(e)))
      .finally(() => setStartingLogin(false));
  };

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

  // Discover existing logins when the subscription login step opens. Each
  // candidate costs a real `claude auth status`, so it waits until the step
  // is actually reached rather than running on mount.
  useEffect(() => {
    if (step !== 3 || billing !== 'sub' || discovered !== null || !canAdoptLogin) return;
    invoke<DiscoveredLogin[]>('list_discoverable_logins', { provider })
      .then(setDiscovered)
      .catch(() => setDiscovered([]));
  }, [step, billing, discovered, provider, canAdoptLogin]);

  const resetLoginState = () => {
    loginStartedRef.current = false;
    setLoginSession(null);
    setTerminalText('');
    setPasteCode('');
    setLoggedIn(false);
    setLoginError(null);
    setWantsNewLogin(false);
    setNewLoginName('');
    setImportedConfigDir(null);
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
    if (!loginSession || !pasteCode.trim() || submittingCode) return;
    setSubmittingCode(true);
    try {
      await invoke('send_login_input', { loginId: loginSession.loginId, data: pasteCode.trim() });
      setPasteCode('');
    } finally {
      setSubmittingCode(false);
    }
  };

  const handleVerifyKey = async () => {
    if (!apiKey.trim() || checkingKey) return;
    setCheckingKey(true);
    try {
      const result = await invoke<ApiKeyCheck>('validate_api_key', { provider, apiKey: apiKey.trim() });
      setKeyCheck(result);
    } catch (e) {
      // La comprobación se cayó, que no dice nada sobre la clave. Marcarla como
      // inválida acusaría al usuario de un fallo nuestro.
      setKeyCheck({ valid: false, unverifiable: true, message: `Couldn't run the check: ${e}` });
    } finally {
      setCheckingKey(false);
    }
  };

  const keyValid = keyCheck?.valid === true;
  /** Verificada, o imposible de verificar pero escrita. Lo segundo no es un
   * error del usuario, y tratarlo como tal dejaba sin crear cualquier cuenta de
   * clave que no fuera de Anthropic — Kimi incluida. */
  const keyUsable = keyValid || (keyCheck?.unverifiable === true && apiKey.trim() !== '');
  /** opencode corre con sus modelos gratuitos sin credencial ninguna —
   * verificado ejecutándolo— así que exigir una clave bloquearía justo al único
   * proveedor que no la necesita. Lo decide Rust, no esta pantalla. */
  const keyOptional = availability[provider]?.keyOptional === true;
  /** Both halves of "can this provider be used at all", with the same
   * treat-a-missing-answer-as-yes rule the cards use. */
  const isUsable = (p: string) => {
    const a = availability[p];
    return a === undefined || (a.available && a.supported);
  };
  const canNext =
    step === 1
      ? isUsable(provider)
      : step === 2
        ? true
        : step === 3
          ? (billing === 'sub' ? loggedIn || importedConfigDir !== null : keyUsable || keyOptional)
          : !nameTaken(name);

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
      // An imported directory wins: when it's set, the embedded login was
      // abandoned in favour of one the user already had.
      configDirPath: importedConfigDir ?? undefined,
      loginId: billing === 'sub' && importedConfigDir === null ? loginSession?.loginId : undefined,
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
                  const usable = isUsable(p);
                  // Two different failures, and the difference is worth a word:
                  // "not installed" is a thing to go and fix, "not supported
                  // yet" is a thing to wait for. Saying only the first about
                  // both would send someone off to install a CLI that wouldn't
                  // have helped.
                  const why = availability[p]?.available === false ? 'not installed' : 'not supported yet';
                  return (
                    <Label
                      key={p}
                      className={`${styles.providerCard} ${provider === p ? styles.selected : ''}`}
                      style={!usable ? { opacity: 0.45, cursor: 'not-allowed' } : undefined}
                      title={usable ? undefined : `Hangar can't run ${meta.name} as its own account yet`}
                    >
                      <RadioGroupItem value={p} disabled={!usable} className="sr-only" />
                      <span
                        className={styles.providerIcon}
                        style={{ background: meta.tintVar, color: meta.colorVar }}
                      >
                        {meta.letter}
                      </span>
                      <span className={styles.providerName}>{meta.name}</span>
                      <span className={styles.providerCli}>{meta.cli}</span>
                      {!usable && (
                        <span style={{ font: '500 9px var(--font-mono)', color: 'var(--red)' }}>{why}</span>
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
                {/* Hidden rather than disabled for a key-only provider: an
                    option you can see but never choose is a question the user
                    has to answer and then discover was rhetorical. */}
                {!KEY_ONLY_PROVIDERS.includes(provider) && (
                  <Label className={`${styles.card2} ${billing === 'sub' ? styles.selected : ''}`}>
                    <RadioGroupItem value="sub" className="sr-only" />
                    <span className={styles.billingName}>Subscription</span>
                    <span className={styles.billingDesc}>
                      Login via browser/CLI. Hangar tracks the 5h session + weekly windows.
                    </span>
                  </Label>
                )}
                <Label className={`${styles.card2} ${billing === 'key' ? styles.selected : ''}`}>
                  <RadioGroupItem value="key" className="sr-only" />
                  <span className={styles.billingName}>API key</span>
                  <span className={styles.billingDesc}>
                    {/* El texto de Kimi estaba puesto para cualquier proveedor
                        solo-clave, y dejó de ser cierto en cuanto hubo un
                        segundo. */}
                    {provider === 'kimi'
                      ? 'Kimi authenticates with a Moonshot API key — there is no subscription login. It runs through Claude Code against Moonshot’s Anthropic-compatible endpoint, so everything else works the same.'
                      : provider === 'opencode'
                        ? 'opencode reads a provider’s key straight from the environment, so there is no login to run here. And the key is optional: without one it still works, on the free models it ships with.'
                        : 'Pay per token. Hangar tracks accumulated spend and enforces your cap.'}
                  </span>
                </Label>
              </RadioGroup>
            </>
          )}

          {/* Chosen an existing login — nothing else to do on this step. */}
          {step === 3 && billing === 'sub' && importedConfigDir !== null && (
            <div className={styles.importedBox}>
              <span className={styles.hint}>
                Using a login you already have — nothing was moved, and no new login was run.
              </span>
              <code className={styles.importedPath}>{importedConfigDir}</code>
              <Button
                variant="outline"
                size="sm"
                onClick={() => setImportedConfigDir(null)}
                style={{ alignSelf: 'flex-start' }}
              >
                choose a different account
              </Button>
            </div>
          )}

          {/* Default state: pick one of the logins already on this machine.
            * One click and the account is correctly configured — no browser
            * round-trip, no config dir to move afterwards. */}
          {step === 3 && billing === 'sub' && importedConfigDir === null && canAdoptLogin && !wantsNewLogin && (
            <>
              <span className={styles.hint}>Which login should this account use?</span>
              {discovered === null && <Spinner label={`finding your ${prov.name} logins…`} />}
              <div className={styles.loginList}>
                {discovered?.map((login) => (
                  <button
                    key={login.path}
                    className={styles.loginRow}
                    onClick={() => {
                      setImportedConfigDir(login.path);
                      // Precharge step 4's name with something identifiable.
                      if (!name.trim()) setName(login.email ?? login.displayPath.replace(/^~\/\.claude-?/, '') ?? '');
                    }}
                  >
                    <span className={styles.loginPath}>{login.displayPath}</span>
                    <span className={styles.loginMeta}>
                      {login.email ?? 'no email on this login'}
                      {login.subscriptionType ? ` · ${login.subscriptionType}` : ''}
                    </span>
                  </button>
                ))}
              </div>
              {discovered?.length === 0 && (
                <span className={styles.hint}>{`No existing ${prov.name} logins found on this machine.`}</span>
              )}
              <button className={styles.newLoginLink} onClick={() => setWantsNewLogin(true)}>
                + Log in a new account instead
              </button>
            </>
          )}

          {/* A genuinely new login. The name is asked first because it decides
            * the config directory (`~/.<name>`), which can't be renamed
            * once `claude auth login` has written the token there. */}
          {step === 3 && billing === 'sub' && importedConfigDir === null && (wantsNewLogin || !canAdoptLogin) && !loginSession && !startingLogin && (
            <div className={styles.newLoginForm}>
              <span className={styles.hint}>Name this account — it also names its config folder:</span>
              <Input
                className={styles.input}
                autoFocus
                value={newLoginName}
                onChange={(e) => setNewLoginName(e.target.value)}
                placeholder="e.g. Work, Personal…"
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && newLoginName.trim() && !nameTaken(newLoginName)) startNewLogin(newLoginName.trim());
                }}
              />
              {nameTaken(newLoginName) && (
                <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>
                  An account named "{newLoginName.trim()}" already exists — pick a different name.
                </span>
              )}
              <div className={styles.importRow}>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={!newLoginName.trim() || nameTaken(newLoginName)}
                  onClick={() => startNewLogin(newLoginName.trim())}
                >
                  Start login →
                </Button>
                <button className={styles.newLoginLink} onClick={() => setWantsNewLogin(false)}>
                  ← back to existing logins
                </button>
              </div>
            </div>
          )}

          {step === 3 && billing === 'sub' && importedConfigDir === null && (wantsNewLogin || !canAdoptLogin) && (loginSession || startingLogin) && (
            <>
              <span className={styles.hint}>Complete the CLI login — Hangar runs it in an embedded terminal:</span>
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
              {/* Only for a CLI that actually asks for one. Codex completes
                  through the browser redirect, so a paste box there is a
                  prompt for something that will never arrive. */}
              {!loggedIn && loginSession && !LOGIN_EXPECTS_CODE.includes(provider) && (
                <span className={styles.hint}>
                  Finish signing in in your browser — this window picks it up on its own, nothing to paste here.
                </span>
              )}
              {!loggedIn && loginSession && LOGIN_EXPECTS_CODE.includes(provider) && (
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
                  <Button variant="outline" className={styles.simulateBtn} disabled={submittingCode || !pasteCode.trim()} onClick={handleSubmitCode}>
                    {submittingCode ? <Spinner label="submitting…" /> : 'Submit code'}
                  </Button>
                </div>
              )}
            </>
          )}

          {step === 3 && billing === 'key' && (
            <>
              <span className={styles.hint}>
                {keyOptional
                  ? 'Paste an API key if you have one — stored in the OS keychain, never in config files. You can also leave it empty and use the free models:'
                  : 'Paste the API key — stored in the OS keychain, never in config files:'}
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
                {nameTaken(name) && (
                  <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>
                    An account named "{name.trim()}" already exists — pick a different name.
                  </span>
                )}
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
