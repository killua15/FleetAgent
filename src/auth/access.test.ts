import { describe, expect, it } from 'vitest';
import { resolveAccess, type AccessInput } from './access';

// The happy path: signed in, approved, server answering. Every case below is
// this minus one thing, which is how the gate is actually met in the wild.
const online = (overrides: Partial<AccessInput> = {}): AccessInput => ({
  hasSession: true,
  approved: true,
  hasStoredToken: true,
  serverReachable: true,
  graceIsLive: true,
  ...overrides,
});

describe('resolveAccess', () => {
  it('lets an approved account with a live session in', () => {
    expect(resolveAccess(online())).toBe('ready');
  });

  it('holds an account the server has not approved yet', () => {
    expect(resolveAccess(online({ approved: false }))).toBe('pending');
    expect(resolveAccess(online({ approved: null }))).toBe('pending');
  });

  it('asks for a sign-in when there is no session', () => {
    expect(resolveAccess(online({ hasSession: false, hasStoredToken: false }))).toBe('signed-out');
  });

  // A revoked token comes back from Supabase as an error *with* an answer, so
  // the provider marks the server reachable and drops the session. Landing on
  // 'pending' here instead of 'signed-out' would leave someone staring at a
  // waiting screen that never resolves.
  it('sends a rejected session back to the sign-in screen, not to the waiting room', () => {
    expect(resolveAccess(online({ hasSession: false, approved: null }))).toBe('signed-out');
  });

  describe('when the server cannot be reached', () => {
    // The case the grace window exists for: a laptop on a plane, orchestrating
    // CLIs that run perfectly well without a network.
    it('runs on a recent check-in', () => {
      expect(resolveAccess(online({ serverReachable: false, hasSession: false, approved: null }))).toBe('ready');
    });

    it('stops once the check-in has gone stale', () => {
      expect(resolveAccess(online({ serverReachable: false, hasSession: false, approved: null, graceIsLive: false }))).toBe('grace-expired');
    });

    // Never having signed in is not a grace period with nothing spent from it.
    it('asks for a sign-in when this machine has no token at all', () => {
      expect(resolveAccess(online({ serverReachable: false, hasStoredToken: false, hasSession: false, graceIsLive: false }))).toBe('signed-out');
    });

    // Offline says nothing about approval, and a stale `approved: false` read
    // from a previous launch must not outrank a live grace window.
    it('does not let a stale approval flag override the grace window', () => {
      expect(resolveAccess(online({ serverReachable: false, approved: false }))).toBe('ready');
    });
  });
});
