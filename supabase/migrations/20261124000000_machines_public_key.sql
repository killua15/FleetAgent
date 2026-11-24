-- Machine-to-machine control: each Hangar publishes an X25519 public key.
--
-- Two machines of the same account derive their shared key from each other's
-- public key (libsodium `crypto_box_beforenm`; see `src-tauri/src/bridge/peers.rs`),
-- so they can talk end to end without a QR. Only the public half is stored here;
-- the secret never leaves the machine's keychain.
--
-- `public.machines` was created by hand for the mobile bridge, with RLS that
-- scopes every row to `owner = auth.uid()`. This only adds a column: the
-- existing policies already limit who can read a machine's key (its owner) and
-- who can write it (its owner, through the upsert the app already does).
--
-- Null means a Hangar too old to publish one; those machines are listed but
-- cannot be controlled remotely.

alter table public.machines add column if not exists public_key text;
