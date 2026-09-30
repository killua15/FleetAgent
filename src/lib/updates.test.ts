import { beforeEach, describe, expect, it, vi } from 'vitest';

const check = vi.fn();
const getSession = vi.fn();
vi.mock('@tauri-apps/plugin-updater', () => ({ check: (...args: unknown[]) => check(...args) }));
vi.mock('./supabase', () => ({
  SUPABASE_PUBLISHABLE_KEY: 'sb_publishable_test',
  supabase: { auth: { getSession: () => getSession() } },
}));

import { checkForUpdate } from './updates';

/**
 * An update downloads with the headers it was checked with, so a check made an
 * hour before the click downloads with an expired token (`400 Bad Request`).
 * Every check has to carry the token of the moment it runs.
 */
describe('checkForUpdate', () => {
  beforeEach(() => {
    check.mockReset().mockResolvedValue(null);
    getSession.mockReset();
  });

  it('asks for the session on every call and sends that token', async () => {
    getSession.mockResolvedValueOnce({ data: { session: { access_token: 'old' } } });
    getSession.mockResolvedValueOnce({ data: { session: { access_token: 'fresh' } } });

    await checkForUpdate();
    await checkForUpdate();

    expect(getSession).toHaveBeenCalledTimes(2);
    expect(check).toHaveBeenLastCalledWith({ headers: { Authorization: 'Bearer fresh', apikey: 'sb_publishable_test' } });
  });

  it('does not ask the channel at all when signed out', async () => {
    getSession.mockResolvedValue({ data: { session: null } });
    expect(await checkForUpdate()).toBeNull();
    expect(check).not.toHaveBeenCalled();
  });
});
