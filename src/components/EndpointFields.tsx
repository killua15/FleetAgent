import { ENDPOINT_PRESETS, endpointHost, endpointIsValid, isInsecureRemoteEndpoint } from '../types/account';
import { Button } from './ui/button';
import { Checkbox } from './ui/checkbox';
import { Input } from './ui/input';
import { Label } from './ui/label';
// The add-account wizard's field styles: the same fields, drawn the same way,
// wherever an endpoint is entered.
import styles from '../screens/AddAccount.module.css';

export interface EndpointValue {
  baseUrl: string;
  defaultModel: string;
  /** Ticked "connect without encryption" for a plain http endpoint on another
   * machine (e.g. the user's own LAN server without a certificate). */
  allowInsecure: boolean;
}

interface EndpointFieldsProps {
  value: EndpointValue;
  onChange: (value: EndpointValue) => void;
}

/** The address and model of a custom endpoint, with the plain-http warning and
 * its opt-in. Shared by the add-account wizard and the edit dialog so the rule
 * for what can be saved lives in one place (`endpointIsValid`). */
export function EndpointFields({ value, onChange }: EndpointFieldsProps) {
  const { baseUrl, defaultModel, allowInsecure } = value;
  const insecureRemote = isInsecureRemoteEndpoint(baseUrl);
  const ok = endpointIsValid(baseUrl, allowInsecure);

  const setBaseUrl = (next: string) =>
    // Accepting the risk is for one address, not whatever is typed next.
    onChange({ ...value, baseUrl: next, allowInsecure: isInsecureRemoteEndpoint(next) ? allowInsecure : false });

  return (
    <>
      <div className={styles.field}>
        <Label className={styles.label}>ENDPOINT</Label>
        <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap' }}>
          {ENDPOINT_PRESETS.map((preset) => (
            <Button
              key={preset.label}
              variant={baseUrl.trim() === preset.baseUrl ? 'secondary' : 'outline'}
              size="sm"
              onClick={() => setBaseUrl(preset.baseUrl)}
            >
              {preset.label}
            </Button>
          ))}
        </div>
        <Input className={styles.input} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://…" />
        {baseUrl.trim() !== '' && !ok && (
          <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>
            {insecureRemote
              ? 'This http:// endpoint is not encrypted — tick the box below to connect anyway, or use https://.'
              : 'Use an https:// address, or http:// for an endpoint on this machine.'}
          </span>
        )}
        {insecureRemote && (
          <div className={styles.insecureWarning}>
            <span>
              ⚠ This endpoint doesn’t use encryption (http://). Your API key, your prompts and the code the agent reads
              will travel in plain text over the network — anyone on it can read or alter them. Only use it with a server
              you own on a network you trust.
            </span>
            <label className={styles.insecureCheck}>
              <Checkbox checked={allowInsecure} onCheckedChange={(checked) => onChange({ ...value, allowInsecure: checked === true })} />
              I understand the risk and want to connect without encryption
            </label>
          </div>
        )}
      </div>
      <div className={styles.field}>
        <Label className={styles.label}>MODEL (OPTIONAL)</Label>
        <Input
          className={styles.input}
          value={defaultModel}
          onChange={(e) => onChange({ ...value, defaultModel: e.target.value })}
          placeholder="leave empty to use the endpoint's default"
        />
      </div>
      {ok && (
        <div className={styles.note}>
          Every terminal on this account sends its prompts and the code it reads to{' '}
          <span className={styles.noteAccent}>{endpointHost(baseUrl.trim())}</span>, under that provider’s terms — not
          Anthropic’s.
        </div>
      )}
    </>
  );
}
