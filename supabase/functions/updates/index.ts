// The update channel Hangar checks, replacing the GitHub Contents API endpoint
// that made every tester paste a personal access token by hand (`update_access.rs`).
//
// Two things make this the whole feature rather than a lookup:
//
//   1. It is the gate. A build is only handed to a session whose `profiles.approved`
//      is true, which is the same switch that lets someone into the app at all
//      (`src/auth/access.ts`). A closed beta needs the delivery to be closed too.
//   2. It answers per platform. The updater asks with `{{target}}`, so Intel Macs,
//      Windows and Linux each get their own artifact — none of which existed as a
//      channel while the manifest was a single file committed to `main`.
//
// The artifact URL it returns points at a private Storage bucket, deliberately
// unsigned: the updater reuses the request's own headers to download
// (tauri-plugin-updater's `Update::download`), so the caller's JWT is already
// there and row-level security can decide, once, in one place.

import { createClient } from 'jsr:@supabase/supabase-js@2';

// Rows are written by CI with the service role; this key only ever reads, and
// only after a caller's own JWT has been checked below.
const supabaseUrl = Deno.env.get('SUPABASE_URL')!;
const serviceRoleKey = Deno.env.get('SUPABASE_SERVICE_ROLE_KEY')!;

/** What the beta runs. The channel split (`internal`/`beta`/`stable`) is
 * deliberately deferred — with five testers it adds a dimension and no answer —
 * but the column exists so adding it later is a query change, not a migration. */
const CHANNEL = 'beta';

/** `0.9.1` vs `0.10.0`: compared number by number, because a string comparison
 * puts `0.9.1` above `0.10.0` and would strand everyone on the older build the
 * day a minor hits double digits. Suffixes like `-rc1` are ignored, not parsed:
 * the beta doesn't publish them, and guessing at precedence would be worse than
 * not supporting them. */
function isNewer(candidate: string, current: string): boolean {
  const parse = (v: string) => v.split('-')[0].split('.').map((n) => Number.parseInt(n, 10) || 0);
  const [a, b] = [parse(candidate), parse(current)];
  for (let i = 0; i < Math.max(a.length, b.length); i++) {
    const [x, y] = [a[i] ?? 0, b[i] ?? 0];
    if (x !== y) return x > y;
  }
  return false;
}

Deno.serve(async (req) => {
  const url = new URL(req.url);
  // The updater fills `{{target}}` with the OS alone (`darwin`) and sends the
  // architecture separately, while CI keys rows by `darwin-aarch64` — so the two
  // are joined here, or every Mac is told there is nothing new.
  const os = url.searchParams.get('target');
  const arch = url.searchParams.get('arch');
  const target = os && arch && !os.includes('-') ? `${os}-${arch}` : os;
  const currentVersion = url.searchParams.get('current_version');
  if (!target) {
    return Response.json({ error: 'target is required' }, { status: 400 });
  }

  // The token is the session the app already holds; no separate credential to
  // set up, which is the point of replacing the PAT.
  const authorization = req.headers.get('Authorization') ?? '';
  const token = authorization.replace(/^Bearer\s+/i, '').trim();
  if (!token) {
    return Response.json({ error: 'sign in to receive updates' }, { status: 401 });
  }

  const admin = createClient(supabaseUrl, serviceRoleKey, { auth: { persistSession: false } });
  const { data: userData, error: userError } = await admin.auth.getUser(token);
  if (userError || !userData.user) {
    return Response.json({ error: 'sign in to receive updates' }, { status: 401 });
  }
  const userId = userData.user.id;

  const { data: profile } = await admin.from('profiles').select('approved').eq('id', userId).maybeSingle();
  if (!profile?.approved) {
    // Not an error state: someone on the waiting list is in exactly this
    // position, and the app renders "no update" rather than a failure.
    return Response.json({ error: 'this account is not in the beta yet' }, { status: 403 });
  }

  const { data: release, error: releaseError } = await admin
    .from('app_releases')
    .select('version, notes, pub_date, object_path, signature')
    .eq('target', target)
    .eq('channel', CHANNEL)
    .eq('published', true)
    .order('pub_date', { ascending: false })
    .limit(1)
    .maybeSingle();
  if (releaseError) {
    return Response.json({ error: releaseError.message }, { status: 500 });
  }

  // Recorded on the way past: this request is the one moment the server learns
  // what a tester is actually running, which is what makes "who is on what"
  // answerable without asking them.
  if (currentVersion) {
    await admin.from('profiles').update({ app_version: currentVersion }).eq('id', userId);
  }

  // 204 is the updater's own "nothing for you" (see the plugin's `check`):
  // anything else here would surface as an error in the app.
  if (!release || (currentVersion && !isNewer(release.version, currentVersion))) {
    return new Response(null, { status: 204 });
  }

  return Response.json({
    version: release.version,
    notes: release.notes ?? `Hangar ${release.version}`,
    pub_date: release.pub_date,
    url: `${supabaseUrl}/storage/v1/object/updates/${release.object_path}`,
    signature: release.signature,
  });
});
