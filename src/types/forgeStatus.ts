import type { Forge } from './pullRequest';

/** One project resting on an instance — named so you can see *what stops
 * working*, rather than being told a hostname is unreachable in the abstract. */
export interface ForgeInstanceProject {
  id: string;
  name: string;
}

/** One instance, named and counted and nothing more — matches `ForgeHost` in
 * `commands/forge_status.rs`.
 *
 * What the PR screen's picker runs on. `ForgeInstanceStatus` below answers a
 * bigger question and pays for it with a login probe per host, which is a
 * network wait on a screen whose whole point is not having one. */
export interface ForgeHost {
  host: string;
  /** Null for a host nothing places — still a real choice, since projects
   * point at it. */
  forge: Forge | null;
  /** What the user called it when declaring it. Empty otherwise. */
  label: string;
  projectCount: number;
}

/** When one instance was last polled for open PRs — matches `PrHostCheck` in
 * `models.rs`. It's what lets a screen painted from the local database say how
 * old what it's showing is. */
export interface PrHostCheck {
  host: string;
  checkedAt: string;
  error: string | null;
}

/** One forge instance the registered projects point at, and whether Hangar can
 * talk to it. Matches `ForgeInstanceStatus` in `commands/forge_status.rs`. */
export interface ForgeInstanceStatus {
  /** Bare hostname: `github.com`, `gitlab.acme.com`. */
  host: string;
  /** Null when nothing can place this host yet — a GitLab on a vanity domain
   * before its first `glab` login. Not the same as unsupported, which is why
   * it's a state of its own and gets rendered rather than hidden. */
  forge: Forge | null;
  authenticated: boolean;
  /** One sentence, when this instance isn't usable. */
  reason: string | null;
  /** What to run. Always set when `authenticated` is false — a row that says
   * something is broken but not what to do is a dead end. */
  loginCommand: string | null;
  projects: ForgeInstanceProject[];
  /** Whether the user said what this is, or Hangar guessed. A guess invites
   * correction; a declaration doesn't. */
  declared: boolean;
  /** What the user calls it — empty unless declared. */
  label: string;
  /** Git works here and there's no API to ask. Settled, not broken: this is the
   * one row that asks for nothing. */
  gitOnly: boolean;
  /** Who you are on this instance, from the API when it answers and from the
   * SSH greeting when it doesn't. */
  viewerLogin: string | null;
}

/** The three things a user can declare a host to be. `git-only` is a real
 * answer, not an absence: it's what stops the poller probing a Gitea or a bare
 * ssh remote on every pass forever. */
export type DeclaredKind = 'github' | 'gitlab' | 'git_only';

export const DECLARED_KIND_LABEL: Record<DeclaredKind, string> = {
  github: 'GitHub',
  gitlab: 'GitLab',
  git_only: 'Solo git',
};

/** One repository on an instance, normalized across forges — GitHub's
 * `full_name`/`ssh_url` and GitLab's `path_with_namespace`/`ssh_url_to_repo`
 * are the same facts under different names. */
export interface InstanceRepo {
  /** `group/subgroup/project` — what identifies it and names the folder. */
  path: string;
  name: string;
  description: string;
  webUrl: string;
  /** What `git clone` gets handed. */
  cloneUrl: string;
}
