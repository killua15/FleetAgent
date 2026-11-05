import { beforeEach, describe, expect, it, vi } from 'vitest';

const check = vi.fn();
const getSession = vi.fn();
const invoke = vi.fn();
vi.mock('@tauri-apps/plugin-updater', () => ({ check: (...args: unknown[]) => check(...args) }));
vi.mock('./supabase', () => ({
  SUPABASE_PUBLISHABLE_KEY: 'sb_publishable_test',
  supabaseConfigured: true,
  supabase: { auth: { getSession: () => getSession() } },
}));
vi.mock('./invoke', () => ({ invoke: (...args: unknown[]) => invoke(...args) }));

import { checkForUpdate, getUpdateStatus, resetUpdateStatusForTests, runUpdateCheck } from './updates';

function fakeUpdate(version: string, install: () => Promise<void> = () => Promise.resolve()) {
  return { version, downloadAndInstall: vi.fn(install) };
}

beforeEach(() => {
  resetUpdateStatusForTests();
  check.mockReset().mockResolvedValue(null);
  getSession.mockReset().mockResolvedValue({ data: { session: { access_token: 'fresh' } } });
  invoke.mockReset().mockResolvedValue(undefined);
});

/**
 * An update downloads with the headers it was checked with, so a check made an
 * hour before the download used an expired token (`400 Bad Request`). Every
 * check carries the token of the moment it runs.
 */
describe('checkForUpdate', () => {
  it('asks for the session on every call and sends that token', async () => {
    getSession.mockResolvedValueOnce({ data: { session: { access_token: 'old' } } });
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

/** The user's only step is restarting: a found build is installed right away. */
describe('runUpdateCheck', () => {
  it('installs a newer build right away and waits for a restart', async () => {
    const update = fakeUpdate('0.9.0');
    check.mockResolvedValue(update);
    await runUpdateCheck();
    expect(update.downloadAndInstall).toHaveBeenCalledTimes(1);
    expect(getUpdateStatus()).toEqual({ phase: 'ready', version: '0.9.0' });
    expect(invoke).toHaveBeenCalledWith('notify_update_available', { version: '0.9.0' });
  });

  it('says so when there is nothing newer', async () => {
    await runUpdateCheck();
    expect(getUpdateStatus()).toEqual({ phase: 'upToDate' });
  });

  it('reports a failed install and writes it to the log', async () => {
    check.mockResolvedValue(fakeUpdate('0.9.0', () => Promise.reject(new Error('disk full'))));
    await runUpdateCheck();
    expect(getUpdateStatus()).toMatchObject({ phase: 'failed', version: '0.9.0' });
    expect(invoke).toHaveBeenCalledWith('log_update_error', { version: '0.9.0', error: 'Error: disk full' });
  });

  it('never runs twice at once', async () => {
    check.mockResolvedValue(fakeUpdate('0.9.0'));
    await Promise.all([runUpdateCheck(), runUpdateCheck(), runUpdateCheck()]);
    expect(check).toHaveBeenCalledTimes(1);
  });

  it('does not look again once a build is waiting for the restart', async () => {
    check.mockResolvedValue(fakeUpdate('0.9.0'));
    await runUpdateCheck();
    await runUpdateCheck();
    expect(check).toHaveBeenCalledTimes(1);
  });
});
