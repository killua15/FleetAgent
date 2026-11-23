import { describe, expect, it } from 'vitest';
import { isInsecureRemoteEndpoint } from '../types/account';

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
