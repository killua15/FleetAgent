//! Signing in to Hangar's own account (Supabase), which is a different problem
//! from signing in to a provider CLI.
//!
//! **Why a loopback server and not a web redirect.** OAuth in a browser hands
//! the code back by navigating to a URL you control. A desktop app controls no
//! URL, so it has to become one: Hangar binds `127.0.0.1:47823` for the duration
//! of a login, the browser lands there with `?code=…`, and the page tells the
//! user to go back to the app.
//!
//! The alternative is a custom scheme (`hangar://`), which needs the bundle to
//! be registered with the OS — so it works in a `.app` and not in `tauri dev`,
//! which is where most of the debugging happens. Loopback behaves the same in
//! both.
//!
//! **What stops another local process from injecting a code**: the exchange is
//! PKCE. The verifier never leaves the webview, so a code delivered to this port
//! by anything other than the login the user just started cannot be exchanged
//! for a session. The listener also serves exactly one request and then stops.

use crate::credential_vault;
use crate::db::DbConnection;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::{params, OptionalExtension};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Runtime};

/// Fixed because Supabase has to allow-list the redirect URL, and an ephemeral
/// port can't be allow-listed. Chosen out of the way of the usual suspects
/// (Supabase's own local stack uses 54321, Vite here uses 1420).
pub const CALLBACK_PORT: u16 = 47823;

/// How long the listener waits for the browser before giving up. Long enough to
/// pick an account and get through a 2FA prompt, short enough that an abandoned
/// login doesn't leave a socket open all afternoon.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(180);

/// Where the refresh token lives in the keychain. Not `localStorage`, which is
/// the default for `supabase-js` and is readable by anything running in the
/// webview.
const REFRESH_TOKEN_REF: &str = "hangar.auth.refresh_token";

/// When the session was last checked against Supabase, in `app_settings`.
///
/// Hangar needs an account to open at all, and that would otherwise mean a
/// laptop on a plane is a laptop that can't run the local CLIs it was bought
/// for. So a session that checked out recently is trusted while the server is
/// unreachable, and this is the stamp that "recently" is measured against.
const LAST_VERIFIED_KEY: &str = "auth.last_verified_at";

/// How long a verified session survives without reaching the server.
///
/// A week: long enough for a holiday or a stretch of bad hotel wifi, short
/// enough that revoking someone's access takes effect while they still
/// remember asking for it.
const GRACE: ChronoDuration = ChronoDuration::days(7);

fn callback_page(message: &str) -> String {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Hangar</title>\
         <body style=\"font:15px -apple-system,system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0;background:#171822;color:#e8e8f2\">\
         <div style=\"text-align:center\"><p style=\"font-size:17px\">{message}</p>\
         <p style=\"color:#9a9ab0\">You can close this tab and go back to Hangar.</p></div>"
    );
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

/// Pulls one query parameter out of a request target like
/// `/callback?code=abc&state=xyz`. Pure so the parsing is testable without a
/// socket.
fn query_param(target: &str, key: &str) -> Option<String> {
    let query = target.split_once('?')?.1;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| urldecode(v))
    })
}

/// Minimal percent-decoding — an OAuth code is URL-safe, but `state` and error
/// descriptions are not, and pulling in a crate for this would be silly.
fn urldecode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(&value[i + 1..i + 3], 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Starts the one-shot callback listener and returns the redirect URL the
/// browser should come back to.
///
/// Emits `oauth-callback` with `{ code }` on success or `{ error }` on failure,
/// so the frontend — which holds the PKCE verifier — can do the exchange.
#[tauri::command]
pub fn start_oauth_callback<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    let listener = TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .map_err(|e| format!("could not listen on 127.0.0.1:{CALLBACK_PORT} for the sign-in callback: {e}"))?;
    listener.set_nonblocking(false).ok();

    std::thread::spawn(move || {
        // One request, then the socket goes away. `incoming()` blocks, so the
        // timeout is enforced by the accept loop's own deadline below.
        let deadline = std::time::Instant::now() + CALLBACK_TIMEOUT;
        listener.set_nonblocking(true).ok();
        loop {
            if std::time::Instant::now() > deadline {
                let _ = app.emit("oauth-callback", serde_json::json!({ "error": "the sign-in window timed out" }));
                return;
            }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    let mut line = String::new();
                    let _ = BufReader::new(&stream).read_line(&mut line);
                    // "GET /callback?code=… HTTP/1.1"
                    let target = line.split_whitespace().nth(1).unwrap_or("").to_string();

                    let payload = match (query_param(&target, "code"), query_param(&target, "error_description").or_else(|| query_param(&target, "error"))) {
                        (Some(code), _) => {
                            let _ = stream.write_all(callback_page("Signed in.").as_bytes());
                            serde_json::json!({ "code": code })
                        }
                        (None, Some(error)) => {
                            let _ = stream.write_all(callback_page("Sign-in failed.").as_bytes());
                            serde_json::json!({ "error": error })
                        }
                        (None, None) => {
                            let _ = stream.write_all(callback_page("Sign-in failed.").as_bytes());
                            serde_json::json!({ "error": "the callback carried no code" })
                        }
                    };
                    let _ = stream.flush();
                    let _ = app.emit("oauth-callback", payload);
                    return;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(120));
                }
                Err(e) => {
                    let _ = app.emit("oauth-callback", serde_json::json!({ "error": e.to_string() }));
                    return;
                }
            }
        }
    });

    Ok(format!("http://127.0.0.1:{CALLBACK_PORT}/callback"))
}

/// Keeps the refresh token in the OS keychain across restarts. The access token
/// deliberately isn't stored: it expires in an hour and `supabase-js` mints a
/// new one from the refresh token at startup.
#[tauri::command]
pub fn store_refresh_token(token: String) -> Result<(), String> {
    credential_vault::store(REFRESH_TOKEN_REF, &token)
}

#[tauri::command]
pub fn load_refresh_token() -> Option<String> {
    credential_vault::get(REFRESH_TOKEN_REF).ok().filter(|t| !t.is_empty())
}

#[tauri::command]
pub fn clear_refresh_token() -> Result<(), String> {
    credential_vault::delete(REFRESH_TOKEN_REF)
}

/// Stamps "the server said this session is good, just now".
///
/// Called after a successful refresh, not after a successful launch: the point
/// is to record contact with Supabase, and an offline launch that coasted on
/// the grace window must not extend that window by another week.
#[tauri::command]
pub fn record_auth_check(db: tauri::State<DbConnection>) -> Result<String, String> {
    store_auth_check(&db, Utc::now())
}

/// Split from the command so the tests don't have to conjure a `State` — the
/// same cut `set_machine_name` makes in `bridge::session`.
fn store_auth_check(db: &DbConnection, now: DateTime<Utc>) -> Result<String, String> {
    let stamp = now.to_rfc3339();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![LAST_VERIFIED_KEY, stamp],
    )
    .map_err(|e| e.to_string())?;
    Ok(stamp)
}

/// The stamp, or none on a machine that has never reached the server.
///
/// Never having checked is not the same as having checked long ago, and the
/// frontend treats it as such: no stamp means no grace to spend.
#[tauri::command]
pub fn last_auth_check(db: tauri::State<DbConnection>) -> Option<String> {
    read_auth_check(&db)
}

fn read_auth_check(db: &DbConnection) -> Option<String> {
    db.0.lock()
        .ok()
        .and_then(|conn| conn.query_row("SELECT value FROM app_settings WHERE key = ?1", params![LAST_VERIFIED_KEY], |row| row.get::<_, String>(0)).optional().ok().flatten())
        .filter(|stamp| !stamp.trim().is_empty())
}

/// Whether an offline launch still counts as signed in.
///
/// A stamp in the future is refused rather than trusted: it can only come from
/// a clock that was wrong or a file that was edited, and neither is evidence
/// that the server ever agreed.
pub fn within_grace(last_verified: &str, now: DateTime<Utc>) -> bool {
    let Ok(stamp) = DateTime::parse_from_rfc3339(last_verified) else {
        return false;
    };
    let elapsed = now.signed_duration_since(stamp.with_timezone(&Utc));
    elapsed >= ChronoDuration::zero() && elapsed <= GRACE
}

/// Exposed so the gate in the frontend asks one question instead of parsing
/// timestamps and re-deriving the window on its own.
#[tauri::command]
pub fn auth_grace_is_live(db: tauri::State<DbConnection>) -> bool {
    read_auth_check(&db).is_some_and(|stamp| within_grace(&stamp, Utc::now()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_code_is_read_out_of_the_callback_target() {
        assert_eq!(query_param("/callback?code=abc123&state=xyz", "code").as_deref(), Some("abc123"));
        assert_eq!(query_param("/callback?state=xyz&code=abc123", "code").as_deref(), Some("abc123"));
        assert_eq!(query_param("/callback", "code"), None);
        assert_eq!(query_param("/callback?state=xyz", "code"), None);
    }

    /// A refusal comes back as `error_description`, percent-encoded — and it's
    /// the only thing that can tell the user *why* the sign-in failed.
    #[test]
    fn an_error_description_is_decoded_into_something_readable() {
        let target = "/callback?error=access_denied&error_description=The+user+denied+access%3A+try+again";
        assert_eq!(query_param(target, "error_description").as_deref(), Some("The user denied access: try again"));
        assert_eq!(query_param(target, "error").as_deref(), Some("access_denied"));
    }

    #[test]
    fn a_malformed_percent_escape_is_left_alone_rather_than_dropping_the_message() {
        assert_eq!(urldecode("100%+sure"), "100% sure");
        assert_eq!(urldecode("trailing%"), "trailing%");
    }

    fn seeded() -> DbConnection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        crate::db::run_migrations(&conn).unwrap();
        DbConnection(std::sync::Mutex::new(conn))
    }

    #[test]
    fn the_stamp_survives_being_written_and_read_back() {
        let db = seeded();
        assert_eq!(read_auth_check(&db), None, "a fresh machine has never checked in");

        let written = store_auth_check(&db, Utc::now()).unwrap();
        assert_eq!(read_auth_check(&db).as_deref(), Some(written.as_str()));

        // Checking in again replaces the stamp rather than accumulating rows —
        // `app_settings` is keyed, and a second insert would otherwise fail.
        let later = store_auth_check(&db, Utc::now() + ChronoDuration::seconds(1)).unwrap();
        assert_eq!(read_auth_check(&db).as_deref(), Some(later.as_str()));
    }

    /// The window is the whole feature: a day offline is fine, a fortnight is
    /// not, and the boundary has to fall where the constant says it does.
    #[test]
    fn a_recent_check_grants_grace_and_an_old_one_does_not() {
        let now = Utc::now();
        assert!(within_grace(&now.to_rfc3339(), now));
        assert!(within_grace(&(now - ChronoDuration::days(6)).to_rfc3339(), now));
        assert!(within_grace(&(now - GRACE + ChronoDuration::minutes(1)).to_rfc3339(), now));
        assert!(!within_grace(&(now - GRACE - ChronoDuration::minutes(1)).to_rfc3339(), now));
        assert!(!within_grace(&(now - ChronoDuration::days(30)).to_rfc3339(), now));
    }

    /// A stamp from the future is not a very recent check — it's a wrong clock
    /// or an edited file, and neither means Supabase ever agreed.
    #[test]
    fn a_stamp_in_the_future_buys_nothing() {
        let now = Utc::now();
        assert!(!within_grace(&(now + ChronoDuration::days(1)).to_rfc3339(), now));
    }

    /// Garbage in the settings table has to read as "never checked", not panic
    /// and not pass.
    #[test]
    fn an_unparseable_stamp_is_refused_rather_than_trusted() {
        assert!(!within_grace("not a timestamp", Utc::now()));
        assert!(!within_grace("", Utc::now()));
    }
}
