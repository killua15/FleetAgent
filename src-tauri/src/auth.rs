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
}
