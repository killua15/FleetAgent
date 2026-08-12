import { describe, expect, it } from 'vitest';
import { ALL_HOSTS, collectForgeHosts, forgeHostLabel, matchesForgeHost, resolveForgeHost, type ForgeHosted } from './forgeHosts';

// Shaped after the case this exists for: a team on github.com, on gitlab.com
// and on its own self-hosted GitLab at once, plus a repo whose host says
// nothing about itself.
const projects: ForgeHosted[] = [
  { forge: 'github', host: 'github.com' },
  { forge: 'github', host: 'github.com' },
  { forge: 'gitlab', host: 'gitlab.com' },
  { forge: 'gitlab', host: 'gitlab.acme.com' },
  { forge: null, host: null },
];

describe('collectForgeHosts', () => {
  it('groups by instance and counts each one', () => {
    const options = collectForgeHosts(projects);
    expect(options.map((o) => [o.host, o.count])).toEqual([
      ['github.com', 2],
      ['gitlab.com', 1],
      ['gitlab.acme.com', 1],
    ]);
  });

  /** The whole reason the picker is keyed by host rather than by forge:
   * gitlab.com and a self-hosted instance are different servers holding
   * different work, and one "GitLab" option would hide which. */
  it('keeps a self-hosted instance apart from the hosted one', () => {
    const options = collectForgeHosts(projects);
    expect(options.filter((o) => o.forge === 'gitlab')).toHaveLength(2);
  });

  it('offers nothing for items it cannot place', () => {
    expect(collectForgeHosts([{ forge: null, host: null }])).toEqual([]);
  });

  it('puts the hosted instances first and orders the rest by name', () => {
    const options = collectForgeHosts([
      { forge: 'gitlab', host: 'zzz.example.com' },
      { forge: 'gitlab', host: 'gitlab.acme.com' },
      { forge: 'gitlab', host: 'gitlab.com' },
    ]);
    expect(options.map((o) => o.host)).toEqual(['gitlab.com', 'gitlab.acme.com', 'zzz.example.com']);
  });
});

describe('forgeHostLabel', () => {
  it('calls the hosted instances by their product name', () => {
    expect(forgeHostLabel({ host: 'github.com', forge: 'github' })).toBe('GitHub');
    expect(forgeHostLabel({ host: 'gitlab.com', forge: 'gitlab' })).toBe('GitLab');
  });

  it('calls a self-hosted instance by its hostname, which is all that tells it apart', () => {
    expect(forgeHostLabel({ host: 'gitlab.acme.com', forge: 'gitlab' })).toBe('gitlab.acme.com');
  });

  /** Named from the host and the forge alone, so the instance panel can label a
   * row without inventing a count it has no use for. */
  it('needs nothing but the host and the forge', () => {
    const option = { host: 'gitlab.com', forge: 'gitlab' as const, count: 7 };
    expect(forgeHostLabel(option)).toBe('GitLab');
  });
});

describe('matchesForgeHost', () => {
  it('lets everything through under All, including what it could not place', () => {
    expect(projects.filter((p) => matchesForgeHost(p, ALL_HOSTS))).toHaveLength(5);
  });

  it('narrows to one instance', () => {
    expect(projects.filter((p) => matchesForgeHost(p, 'gitlab.com'))).toHaveLength(1);
  });

  /** A repo Hangar can't place belongs to no instance, so a named filter has
   * to hide it — it's only ever visible under All. */
  it('hides an unplaceable repo from a named instance', () => {
    expect(matchesForgeHost({ forge: null, host: null }, 'github.com')).toBe(false);
  });
});

describe('resolveForgeHost', () => {
  it('keeps a selection that still has work behind it', () => {
    expect(resolveForgeHost('gitlab.com', collectForgeHosts(projects))).toBe('gitlab.com');
  });

  /** The last merge request on an instance gets merged, or its last project is
   * removed — falling back to All beats rendering an empty screen with no
   * visible reason why. */
  it('falls back to All when the selected instance has gone away', () => {
    expect(resolveForgeHost('gitlab.acme.com', collectForgeHosts([{ forge: 'github', host: 'github.com' }]))).toBe(ALL_HOSTS);
    expect(resolveForgeHost('github.com', [])).toBe(ALL_HOSTS);
  });
});
