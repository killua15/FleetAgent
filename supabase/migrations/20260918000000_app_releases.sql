-- The update channel's server side: what builds exist, and who may download them.
--
-- Until now the manifest was a JSON file committed to `main` next to a ~9 MB
-- binary, which is why only Apple Silicon ever had a channel — three more
-- platforms meant three more binaries in git, permanently, every release.
-- Rows here replace the file; the bytes live in Storage.

create table if not exists public.app_releases (
  id          uuid primary key default gen_random_uuid(),
  version     text not null,
  -- The updater's own platform key (`darwin-aarch64`, `darwin-x86_64`,
  -- `windows-x86_64`, `linux-x86_64`), stored verbatim so the function can match
  -- on what the app asks with rather than on a translation of it.
  target      text not null,
  -- Path inside the private `updates` bucket, not a URL: the host belongs to the
  -- environment, and baking it into a row would pin every release to whichever
  -- project wrote it.
  object_path text not null,
  -- Detached minisign signature produced by CI. The app verifies against the
  -- public key in `tauri.conf.json`, so a compromised bucket still can't ship code.
  signature   text not null,
  pub_date    timestamptz not null default now(),
  notes       text,
  channel     text not null default 'beta',
  published   boolean not null default true,
  created_at  timestamptz not null default now(),
  unique (version, target, channel)
);

create index if not exists app_releases_lookup
  on public.app_releases (target, channel, published, pub_date desc);

alter table public.app_releases enable row level security;

-- Readable by the beta, and only by the beta: the same `approved` switch that
-- governs entry to the app. No write policy at all, so only the service role CI
-- uses can publish — a client with a valid session still cannot announce a release.
drop policy if exists "approved testers read releases" on public.app_releases;
create policy "approved testers read releases"
  on public.app_releases for select
  to authenticated
  using (exists (select 1 from public.profiles p where p.id = auth.uid() and p.approved));

-- The bucket is private: a public one would make the build downloadable by
-- anyone who learned the URL, which is the thing a closed beta is trying not to do.
insert into storage.buckets (id, name, public)
values ('updates', 'updates', false)
on conflict (id) do nothing;

-- Downloads carry the tester's own JWT — the updater reuses the check request's
-- headers — so the same rule can guard the bytes as guards the rows.
drop policy if exists "approved testers download updates" on storage.objects;
create policy "approved testers download updates"
  on storage.objects for select
  to authenticated
  using (
    bucket_id = 'updates'
    and exists (select 1 from public.profiles p where p.id = auth.uid() and p.approved)
  );
