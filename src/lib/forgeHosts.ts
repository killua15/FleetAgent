import { FORGE_LABEL, type Forge } from '../types/pullRequest';

/** Anything addressable on a forge — a project's repo, a pull/merge request.
 * Both carry the same pair, which is what lets one picker filter both
 * screens. Nullable because a project need not be a git repo at all, and a
 * host Hangar can't place has no forge (see `forges::Forge::for_host`). */
export interface ForgeHosted {
  forge: Forge | null;
  host: string | null;
}

/** One entry in the provider picker: a real instance someone actually has
 * work on, never a fixed list. A team with `gitlab.com` and a self-hosted
 * `gitlab.acme.com` has two distinct destinations, and collapsing them to
 * "GitLab" would hide which one a merge request is on. */
export interface ForgeHostOption {
  host: string;
  forge: Forge;
  count: number;
}

/** Every host, the way `NO_FILTER` works on the task board: an unset filter
 * and "every value" are the same thing, so this is a real value rather than
 * an absent one. */
export const ALL_HOSTS = 'all';

/** The instances the forges themselves run — the only two that get to be
 * called by their product name alone. */
const CANONICAL_HOSTS: Record<Forge, string> = {
  github: 'github.com',
  gitlab: 'gitlab.com',
};

/**
 * The instances present in `items`, with how many of them are on each.
 *
 * Keyed by host rather than by forge, and derived from the data rather than
 * hardcoded, for the same reason the task board derives its containers: no
 * fixed list is right for two people.
 *
 * Items with no host are counted by nothing — they're only ever visible under
 * "All", which is the honest answer for a repo whose host Hangar couldn't
 * place.
 */
export function collectForgeHosts(items: ForgeHosted[]): ForgeHostOption[] {
  const byHost = new Map<string, ForgeHostOption>();
  for (const item of items) {
    if (!item.host || !item.forge) continue;
    const found = byHost.get(item.host);
    if (found) found.count += 1;
    else byHost.set(item.host, { host: item.host, forge: item.forge, count: 1 });
  }
  // The hosted instances first — they're the common case and belong at the
  // near end of the row — then everything else alphabetically, so a team's
  // self-hosted instances keep a stable order between renders.
  return [...byHost.values()].sort((a, b) => {
    const canonical = Number(isCanonical(b)) - Number(isCanonical(a));
    return canonical !== 0 ? canonical : a.host.localeCompare(b.host);
  });
}

function isCanonical(option: NamedInstance): boolean {
  return CANONICAL_HOSTS[option.forge] === option.host;
}

/** An instance Hangar has placed — enough to name it, without the count a
 * picker option carries. */
type NamedInstance = { host: string; forge: Forge };

/** What to call an instance. `github.com` is just "GitHub"; a self-hosted
 * instance is its own hostname, because that's the only thing distinguishing
 * it from every other GitLab. */
export function forgeHostLabel(option: NamedInstance): string {
  return isCanonical(option) ? FORGE_LABEL[option.forge] : option.host;
}

/** Whether one item survives the current selection. */
export function matchesForgeHost(item: ForgeHosted, selected: string): boolean {
  return selected === ALL_HOSTS || item.host === selected;
}

/** Keeps a selection honest against the hosts that actually exist right now.
 * A host can disappear between loads — the last project on it gets removed,
 * its last merge request gets merged — and a filter pinned to a host with
 * nothing behind it renders an empty screen with no visible reason why. */
export function resolveForgeHost(selected: string, options: ForgeHostOption[]): string {
  return options.some((option) => option.host === selected) ? selected : ALL_HOSTS;
}
