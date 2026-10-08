import { describe, expect, it } from 'vitest';
import { endpointHost, PROVIDER_META, providerMeta, UNKNOWN_PROVIDER_META, type Provider } from './account';

/** These lookups are the one place a string straight out of SQLite is used to
 * index a map the build wrote at compile time. That gap is not theoretical: an
 * August build read an account written as `opencode` — a provider added later,
 * in a database both builds share — and took the screen down on `.tintVar` of
 * undefined. */
describe('providerMeta', () => {
  it('returns the real entry for every provider this build knows', () => {
    for (const p of Object.keys(PROVIDER_META) as Provider[]) {
      expect(providerMeta(p)).toBe(PROVIDER_META[p]);
    }
  });

  it('falls back instead of throwing on a provider from a newer version', () => {
    const meta = providerMeta('some-provider-from-the-future');
    expect(meta.letter).toBe(UNKNOWN_PROVIDER_META.letter);
    expect(meta.tintVar).toBe(UNKNOWN_PROVIDER_META.tintVar);
    expect(meta.colorVar).toBe(UNKNOWN_PROVIDER_META.colorVar);
  });

  it('names the unknown provider after itself', () => {
    // The raw string is the only true thing known about it, and it reads far
    // better than "?" in the rows that show a name rather than a badge.
    expect(providerMeta('opencode-ng').name).toBe('opencode-ng');
    expect(providerMeta('opencode-ng').cli).toBe('opencode-ng');
  });

  it('offers no login command for a provider it cannot know how to log into', () => {
    // A row that renders `loginCmd` would otherwise print `undefined` — or
    // worse, a terminal could try to run it.
    expect(providerMeta('whatever').loginCmd).toBe('');
  });

  it('survives the shapes a column can actually hold', () => {
    // Empty string is reachable from a hand-edited database; the CHECK
    // constraint rejects it, but nothing here should depend on that.
    expect(() => providerMeta('')).not.toThrow();
    expect(providerMeta('').letter).toBe('?');
  });
});

/** The host is what every "your code goes here" label is built from, so it has
 * to be the real destination and never a credential that rode along in the
 * address. */
describe('endpointHost', () => {
  it('shows the host and port of an endpoint', () => {
    expect(endpointHost('https://openrouter.ai/api')).toBe('openrouter.ai');
    expect(endpointHost('http://localhost:3001')).toBe('localhost:3001');
    expect(endpointHost('https://api.z.ai/api/anthropic?x=1')).toBe('api.z.ai');
  });

  it('drops credentials embedded in the address', () => {
    expect(endpointHost('https://user:secret@api.deepseek.com/anthropic')).toBe('api.deepseek.com');
  });

  it('falls back to the whole string rather than showing nothing', () => {
    expect(endpointHost('not-a-url')).toBe('not-a-url');
    expect(endpointHost('')).toBe('');
  });
});
