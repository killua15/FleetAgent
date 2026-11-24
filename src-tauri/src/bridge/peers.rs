//! The user's other machines: identity, who they are, and whether this one
//! lets them in.
//!
//! The phone pairs with a QR (`pairing.rs`). Machines of the same account do
//! not: each publishes an X25519 public key in `public.machines`, and two
//! machines derive their shared key from each other's (`crypto::derive_peer_key`).
//! The trust root is therefore the Supabase account, not a QR — written down as
//! the "Máquinas propias" decision in `MOBILE_BRIDGE_TRD.md` §12, with its cost:
//! whoever gets into the account gets a shell on every machine that has remote
//! access turned on. That is why it is **off by default, per machine**.
//!
//! This module serves both roles. As a host it answers machines that call in;
//! as a client it seals what this machine sends to another one. Encryption
//! stays in Rust either way: the webview only carries envelopes.

use crate::bridge::crypto::{self, MachineSecret, SharedKey};
use crate::bridge::protocol::{Envelope, Kind};
use crate::bridge::session;
use crate::db::DbConnection;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const REMOTE_ACCESS_KEY: &str = "bridge.remote_access";
const SECRET_REF: &str = "bridge-machine-secret";

/// How long a machine counts as connected after its last valid message. The
/// client greets every 30 s, so this tolerates one lost heartbeat.
pub const PEER_TTL: Duration = Duration::from_secs(75);

/// Read once per run: a keychain read per message would be slow and, on an
/// unsigned dev build, a prompt per message.
static SECRET: OnceLock<MachineSecret> = OnceLock::new();

/// This machine's identity secret, created the first time it is needed.
///
/// A read that fails is an error and **not** a reason to make a new one: a
/// locked keychain would otherwise silently replace the key the other machines
/// know, and every one of them would stop being able to talk to this one.
pub fn machine_secret() -> Result<MachineSecret, String> {
    if let Some(secret) = SECRET.get() {
        return Ok(secret.clone());
    }
    let secret = if crate::bridge::dev_pairing_key::has(SECRET_REF) {
        MachineSecret::from_base64(&crate::bridge::dev_pairing_key::get(SECRET_REF)?)?
    } else {
        let fresh = MachineSecret::generate();
        crate::bridge::dev_pairing_key::store(SECRET_REF, &fresh.to_base64())?;
        fresh
    };
    Ok(SECRET.get_or_init(|| secret).clone())
}

struct Peer {
    name: String,
    public_key: String,
    key: SharedKey,
    last_seen: Option<Instant>,
}

static PEERS: Mutex<Option<HashMap<String, Peer>>> = Mutex::new(None);

/// One row of `public.machines`, as the webview reads it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerInput {
    pub id: String,
    pub name: String,
    pub public_key: Option<String>,
}

/// What the interface shows about another machine.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerView {
    pub id: String,
    pub name: String,
    /// `None` when that machine has not published a key yet (an older Hangar).
    pub fingerprint: Option<String>,
    /// Whether it called in to this machine within `PEER_TTL`.
    pub connected: bool,
}

/// Replaces the known machines with what Supabase says the account owns.
///
/// The list comes from `public.machines`, whose RLS already limits it to the
/// signed-in account, so a machine of another account can never be in it.
/// Machines without a key are listed but cannot talk: there is nothing to
/// derive from.
pub fn set_peers(own_id: &str, rows: Vec<PeerInput>) -> Result<Vec<PeerView>, String> {
    let secret = machine_secret()?;
    let mut guard = PEERS.lock().map_err(|e| e.to_string())?;
    let previous = guard.take().unwrap_or_default();
    let mut next = HashMap::new();
    let mut views = Vec::new();

    for row in rows.into_iter().filter(|row| row.id != own_id) {
        let Some(public_key) = row.public_key.filter(|k| !k.trim().is_empty()) else {
            views.push(PeerView { id: row.id, name: row.name, fingerprint: None, connected: false });
            continue;
        };
        let Ok(key) = crypto::derive_peer_key(&secret, &public_key) else {
            log::warn!("la máquina «{}» publicó una clave pública no válida", row.name);
            continue;
        };
        // Keep the presence of a machine whose key did not change; a new key is
        // a new identity and starts disconnected.
        let last_seen = previous.get(&row.id).filter(|p| p.public_key == public_key).and_then(|p| p.last_seen);
        views.push(PeerView {
            id: row.id.clone(),
            name: row.name.clone(),
            fingerprint: Some(crypto::fingerprint(&public_key)),
            connected: last_seen.is_some_and(|t| t.elapsed() < PEER_TTL),
        });
        next.insert(row.id, Peer { name: row.name, public_key, key, last_seen });
    }

    *guard = Some(next);
    Ok(views)
}

/// The key shared with that machine, if it is one of the account's.
pub fn key_for(peer_id: &str) -> Option<SharedKey> {
    PEERS.lock().ok()?.as_ref()?.get(peer_id).map(|p| p.key.clone())
}

pub fn name_of(peer_id: &str) -> Option<String> {
    PEERS.lock().ok()?.as_ref()?.get(peer_id).map(|p| p.name.clone())
}

/// A valid message from that machine just arrived.
pub fn mark_seen(peer_id: &str) {
    if let Ok(mut guard) = PEERS.lock() {
        if let Some(peer) = guard.as_mut().and_then(|peers| peers.get_mut(peer_id)) {
            peer.last_seen = Some(Instant::now());
        }
    }
}

/// The machines that called in recently, with their keys: who events go to.
pub fn connected() -> Vec<(String, SharedKey)> {
    let Ok(guard) = PEERS.lock() else { return Vec::new() };
    guard
        .as_ref()
        .map(|peers| {
            peers
                .iter()
                .filter(|(_, p)| p.last_seen.is_some_and(|t| t.elapsed() < PEER_TTL))
                .map(|(id, p)| (id.clone(), p.key.clone()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn is_connected(peer_id: &str) -> bool {
    PEERS
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref()?.get(peer_id)?.last_seen)
        .is_some_and(|t| t.elapsed() < PEER_TTL)
}

/// Whether this machine accepts being driven by the account's other machines.
/// Off until someone turns it on in front of this machine.
pub fn remote_access_enabled(db: &DbConnection) -> bool {
    db.0
        .lock()
        .ok()
        .and_then(|conn| conn.query_row("SELECT value FROM app_settings WHERE key = ?1", params![REMOTE_ACCESS_KEY], |row| row.get::<_, String>(0)).optional().ok().flatten())
        .is_some_and(|value| value == "on")
}

fn store_remote_access(db: &DbConnection, enabled: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![REMOTE_ACCESS_KEY, if enabled { "on" } else { "off" }],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// What this machine publishes about itself and shows on its own screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MachineIdentity {
    pub machine_id: String,
    pub machine_name: String,
    pub public_key: String,
    pub fingerprint: String,
    pub remote_access: bool,
}

#[tauri::command]
pub fn bridge_machine_identity(db: tauri::State<DbConnection>) -> Result<MachineIdentity, String> {
    let public_key = machine_secret()?.public_key();
    Ok(MachineIdentity {
        machine_id: session::machine_id(&db)?,
        machine_name: session::machine_name(&db),
        fingerprint: crypto::fingerprint(&public_key),
        public_key,
        remote_access: remote_access_enabled(&db),
    })
}

#[tauri::command]
pub fn bridge_set_peers(db: tauri::State<DbConnection>, peers: Vec<PeerInput>) -> Result<Vec<PeerView>, String> {
    set_peers(&session::machine_id(&db)?, peers)
}

#[tauri::command]
pub fn set_remote_access<R: tauri::Runtime>(app: tauri::AppHandle<R>, db: tauri::State<DbConnection>, enabled: bool) -> Result<bool, String> {
    store_remote_access(&db, enabled)?;
    if !enabled {
        // Turning it off has to cut whoever is watching now, not on their next
        // message: their live terminals stop streaming immediately.
        crate::terminals::detach_all_remote(&app);
    }
    use tauri::Emitter;
    let _ = app.emit("bridge-remote-access-changed", serde_json::json!({ "enabled": enabled }));
    Ok(enabled)
}

/// Client side: seal something for another machine. `body` is the clear JSON
/// (an `Invoke`, or a `Hello`).
#[tauri::command]
pub fn remote_seal(db: tauri::State<DbConnection>, peer_id: String, kind: Kind, id: String, body: serde_json::Value) -> Result<Envelope, String> {
    let key = key_for(&peer_id).ok_or_else(|| format!("la máquina {peer_id} no es de esta cuenta o no tiene clave"))?;
    crate::bridge::outgoing_to_peer(kind, id, &key, &body, &session::machine_id(&db)?, &peer_id)
}

/// What a message from another machine said, once opened.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Opened {
    pub id: String,
    pub kind: Kind,
    pub from: String,
    pub body: serde_json::Value,
}

/// Client side: open what another machine sent to this one. `None` means "not
/// for this machine" — someone else's traffic on a shared channel — and is
/// dropped without a word.
#[tauri::command]
pub fn remote_open(db: tauri::State<DbConnection>, envelope: Envelope) -> Result<Option<Opened>, String> {
    open_from_peer(&session::machine_id(&db)?, &envelope)
}

fn open_from_peer(own_id: &str, envelope: &Envelope) -> Result<Option<Opened>, String> {
    let (Some(from), Some(to)) = (envelope.from.as_deref(), envelope.to.as_deref()) else { return Ok(None) };
    if to != own_id {
        return Ok(None);
    }
    let Some(key) = key_for(from) else { return Ok(None) };
    let plaintext = crypto::open(&key, &envelope.nonce, &envelope.payload)?;
    mark_seen(from);
    Ok(Some(Opened {
        id: envelope.id.clone(),
        kind: envelope.kind,
        from: from.to_string(),
        body: serde_json::from_slice(&plaintext).map_err(|e| e.to_string())?,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The peer table is process-wide; these tests share it, so they run
    /// under one lock instead of racing each other.
    static SERIAL: Mutex<()> = Mutex::new(());

    fn seeded() -> DbConnection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        crate::db::run_migrations(&conn).unwrap();
        DbConnection(Mutex::new(conn))
    }

    fn install(own_id: &str, peer_id: &str, peer: &MachineSecret) {
        // The process secret is whatever `machine_secret` returns; tests only
        // need it to be stable, which it is.
        SECRET.get_or_init(MachineSecret::generate);
        set_peers(own_id, vec![PeerInput { id: peer_id.into(), name: "otra".into(), public_key: Some(peer.public_key()) }]).unwrap();
    }

    #[test]
    fn remote_access_is_off_until_turned_on() {
        let db = seeded();
        assert!(!remote_access_enabled(&db));
        store_remote_access(&db, true).unwrap();
        assert!(remote_access_enabled(&db));
        store_remote_access(&db, false).unwrap();
        assert!(!remote_access_enabled(&db));
    }

    #[test]
    fn this_machine_is_never_its_own_peer() {
        let _serial = SERIAL.lock().unwrap();
        SECRET.get_or_init(MachineSecret::generate);
        let me = MachineSecret::generate();
        let views = set_peers("yo", vec![PeerInput { id: "yo".into(), name: "yo".into(), public_key: Some(me.public_key()) }]).unwrap();
        assert!(views.is_empty());
        assert!(key_for("yo").is_none());
    }

    #[test]
    fn a_machine_without_a_key_is_listed_but_cannot_talk() {
        let _serial = SERIAL.lock().unwrap();
        SECRET.get_or_init(MachineSecret::generate);
        let views = set_peers("yo", vec![PeerInput { id: "vieja".into(), name: "Hangar viejo".into(), public_key: None }]).unwrap();
        assert_eq!(views.len(), 1);
        assert!(views[0].fingerprint.is_none());
        assert!(key_for("vieja").is_none());
    }

    /// The other machine seals with its secret and this one's public key; this
    /// side opens with the cached pair key. That is the whole handshake.
    #[test]
    fn a_message_from_a_peer_opens_and_marks_it_connected() {
        let _serial = SERIAL.lock().unwrap();
        let peer = MachineSecret::generate();
        install("yo", "b", &peer);
        assert!(!is_connected("b"));

        let mine = machine_secret().unwrap().public_key();
        let key = crypto::derive_peer_key(&peer, &mine).unwrap();
        let envelope = crate::bridge::outgoing_to_peer(Kind::Hello, "01J".into(), &key, &serde_json::json!({"v":1,"device":"b"}), "b", "yo").unwrap();

        let opened = open_from_peer("yo", &envelope).unwrap().expect("addressed to this machine");
        assert_eq!(opened.from, "b");
        assert_eq!(opened.body["device"], "b");
        assert!(is_connected("b"));
    }

    /// Traffic for another machine on the same channel is ignored, not an error.
    #[test]
    fn a_message_for_someone_else_is_ignored() {
        let _serial = SERIAL.lock().unwrap();
        let peer = MachineSecret::generate();
        install("yo", "b", &peer);
        let key = crypto::derive_peer_key(&peer, &machine_secret().unwrap().public_key()).unwrap();
        let envelope = crate::bridge::outgoing_to_peer(Kind::Result, "01J".into(), &key, &serde_json::json!({}), "b", "otra").unwrap();
        assert!(open_from_peer("yo", &envelope).unwrap().is_none());
    }

    /// A machine that is not the account's has no key here, whatever it claims.
    #[test]
    fn an_unknown_sender_is_ignored() {
        let _serial = SERIAL.lock().unwrap();
        install("yo", "b", &MachineSecret::generate());
        let stranger = MachineSecret::generate();
        let key = crypto::derive_peer_key(&stranger, &machine_secret().unwrap().public_key()).unwrap();
        let envelope = crate::bridge::outgoing_to_peer(Kind::Invoke, "01J".into(), &key, &serde_json::json!({}), "intruso", "yo").unwrap();
        assert!(open_from_peer("yo", &envelope).unwrap().is_none());
    }
}
