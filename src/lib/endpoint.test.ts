import { describe, expect, it } from 'vitest';
import { endpointIsValid, isInsecureRemoteEndpoint, shouldRemoveConfigDirByDefault } from '../types/account';

describe('isInsecureRemoteEndpoint', () => {
  it('flags plain http to another machine', () => {
    expect(isInsecureRemoteEndpoint('http://192.168.1.50:4000')).toBe(true);
    expect(isInsecureRemoteEndpoint(' http://my-server.local:3001/anthropic ')).toBe(true);
    expect(isInsecureRemoteEndpoint('http://localhost.evil.com')).toBe(true);
  });

  it('leaves https and this machine alone', () => {
    expect(isInsecureRemoteEndpoint('https://openrouter.ai/api')).toBe(false);
    expect(isInsecureRemoteEndpoint('http://localhost:3001')).toBe(false);
    expect(isInsecureRemoteEndpoint('http://127.0.0.1:4000/')).toBe(false);
    expect(isInsecureRemoteEndpoint('http://[::1]:4000')).toBe(false);
    expect(isInsecureRemoteEndpoint('')).toBe(false);
  });
});

describe('endpointIsValid', () => {
  it('accepts https anywhere and http on this machine', () => {
    expect(endpointIsValid('https://openrouter.ai/api', false)).toBe(true);
    expect(endpointIsValid('http://localhost:3001', false)).toBe(true);
  });

  it('needs the risk accepted for http elsewhere', () => {
    expect(endpointIsValid('http://192.168.1.50:4000', false)).toBe(false);
    expect(endpointIsValid('http://192.168.1.50:4000', true)).toBe(true);
  });

  it('refuses broken addresses whatever the checkbox says', () => {
    for (const url of ['ftp://example.com', 'openrouter.ai/api', 'http://', 'http://exa mple.com', '']) {
      expect(endpointIsValid(url, true)).toBe(false);
    }
  });
});

describe('shouldRemoveConfigDirByDefault', () => {
  it('erases only directories Hangar made', () => {
    expect(shouldRemoveConfigDirByDefault({ configDirOwned: true, configDirPath: '/Users/me/.glm' })).toBe(true);
    expect(shouldRemoveConfigDirByDefault({ configDirOwned: false, configDirPath: '/Users/me/.claude-work' })).toBe(false);
  });

  it('keeps an older account’s directory unless it is a wizard folder', () => {
    expect(shouldRemoveConfigDirByDefault({ configDirOwned: null, configDirPath: '/Users/me/.claude-work' })).toBe(false);
    expect(shouldRemoveConfigDirByDefault({ configDirOwned: null, configDirPath: '/Users/me/Library/Application Support/com.killua.hangar/pending-logins/abc' })).toBe(true);
  });
});
