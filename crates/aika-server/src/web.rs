//! Token server (port 8090 by default).
//!
//! These are the routes the client login screen calls over HTTP POST before
//! opening any game socket. Names and response formats come from the Delphi
//! server's `TTokenServer` — including the numeric error replies, which the
//! client turns into on-screen messages.

use crate::http::{self, Request, Response};
use crate::state::State;
use crate::store::{AuthOutcome, PasswordForm};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

pub const ROUTE_GET_TOKEN: &str = "/member/aika_get_token.asp";
pub const ROUTE_GET_CHRCNT: &str = "/servers/aika_get_chrcnt.asp";
pub const ROUTE_RESET_FLAG: &str = "/servers/aika_reset_flag.asp";
/// Where the launcher checks the client version before unlocking START.
pub const ROUTE_PATCH_INFO: &str = "/etc/patch/patch.htm";
/// Page rendered inside the launcher; purely decorative.
pub const ROUTE_LAUNCHER: &str = "/etc/launcher/launcher.html";

pub async fn serve(state: Arc<State>, listener: TcpListener) -> anyhow::Result<()> {
    info!(addr = %listener.local_addr()?, "token server listening");
    loop {
        let (stream, peer) = listener.accept().await?;
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(state, stream, peer.ip()).await {
                debug!(%peer, error = %e, "HTTP connection closed");
            }
        });
    }
}

async fn handle_connection(
    state: Arc<State>,
    mut stream: TcpStream,
    remote: std::net::IpAddr,
) -> anyhow::Result<()> {
    let request = match http::read_request(&mut stream, remote).await {
        Ok(Some(request)) => request,
        // Opened and closed with no data: the launcher probing.
        Ok(None) => return Ok(()),
        Err(e) => {
            warn!(%remote, error = ?e, "unreadable HTTP request");
            let response = Response::status(400, "Bad Request");
            let _ = http::write_response(&mut stream, &response).await;
            return Ok(());
        }
    };

    let response = route(&state, &request);
    debug!(
        %remote,
        method = %request.method,
        path = %request.path,
        status = response.status,
        "HTTP"
    );
    http::write_response(&mut stream, &response).await?;
    Ok(())
}

/// Dispatches a route. Unlike the original, which answers 403 to anything
/// that is not a POST, we accept GET with the same parameters — that is what
/// makes the routes testable with a browser or `curl` during development,
/// and the client never depends on the refusal.
pub fn route(state: &State, request: &Request) -> Response {
    match request.path.as_str() {
        ROUTE_GET_TOKEN => get_token(state, request),
        ROUTE_GET_CHRCNT => get_char_count(state, request),
        ROUTE_RESET_FLAG => reset_flag(state, request),
        ROUTE_PATCH_INFO => patch_info(state),
        ROUTE_LAUNCHER => launcher_page(state),
        path if is_server_status_route(path) => Response::text(server_status(state)),
        _ => {
            warn!(path = %request.path, "unknown route");
            Response::status(404, "Invalid endpoint")
        }
    }
}

/// `/servers/serv00.asp`, `/servers/serv01.asp`, ...
fn is_server_status_route(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/servers/serv") else {
        return false;
    };
    let Some(digits) = rest.strip_suffix(".asp") else {
        return false;
    };
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn get_token(state: &State, request: &Request) -> Response {
    let (Some(username), Some(password)) =
        (request.params.get_or_nth("id", 0), request.params.get_or_nth("pw", 1))
    else {
        warn!("aika_get_token without id/pw");
        return Response::text(AuthOutcome::NotFound.as_response());
    };

    let username = username.trim().to_ascii_lowercase();
    let (outcome, form) = state.store.authenticate(&username, password, request.remote);

    match (&outcome, form) {
        (AuthOutcome::Ok { .. }, Some(form)) => {
            // Which form the client uses is only knowable from a real
            // login; log it so the behaviour can be pinned down later.
            info!(
                user = %username,
                password = match form {
                    PasswordForm::Plain => "plaintext",
                    PasswordForm::PreHashed => "pre-hashed MD5",
                },
                "token issued"
            );
        }
        _ => info!(user = %username, result = outcome.as_response(), "login refused"),
    }

    Response::text(outcome.as_response())
}

fn get_char_count(state: &State, request: &Request) -> Response {
    let (Some(username), Some(token)) =
        (request.params.get_or_nth("id", 0), request.params.get_or_nth("pw", 1))
    else {
        return Response::text("0");
    };

    let Some(account) = state.store.get(username.trim()) else {
        return Response::text("0"); // account not found
    };

    let matches_token = account
        .last_token
        .as_deref()
        .map(|stored| stored.eq_ignore_ascii_case(token))
        .unwrap_or(false);

    if !matches_token {
        return Response::text("-1"); // wrong token
    }

    Response::text(format!(
        "CNT {} 0 0 0<br>{} 0 0 0",
        account.characters.len(),
        account.nation
    ))
}

fn reset_flag(state: &State, request: &Request) -> Response {
    let (Some(username), Some(token)) =
        (request.params.get_or_nth("id", 0), request.params.get_or_nth("pw", 1))
    else {
        return Response::text("0");
    };

    if state.store.reset_token_flag(username.trim(), token) {
        Response::text(token)
    } else {
        Response::text("-1")
    }
}

/// The reply the launcher writes into `update.dat`. The format must match
/// the original file exactly — `[AIKA] ` with a trailing space, a CRLF, then
/// the version followed by the patch file. When the version matches what the
/// client already has, START unlocks without downloading anything.
fn patch_info(state: &State) -> Response {
    Response {
        status: 200,
        content_type: "text/html",
        body: format!("[AIKA] \r\n{} {}", state.cfg.patch.version, state.cfg.patch.file),
    }
}

/// The launcher opens this page in an embedded browser. Without it, an
/// Internet Explorer error is drawn over the screen.
fn launcher_page(state: &State) -> Response {
    let channel = state.cfg.servers.first().map(|s| s.name.as_str()).unwrap_or("Local server");
    Response {
        status: 200,
        content_type: "text/html",
        body: format!(
            "<!doctype html><html><head><meta charset=\"iso-8859-1\"><title>Aika</title>\
             <style>body{{background:#1a1207;color:#f0d9a8;font-family:Tahoma,sans-serif;\
             margin:0;padding:12px;font-size:12px}}h1{{font-size:15px;margin:0 0 6px}}\
             .ok{{color:#8fd67a}}</style></head><body>\
             <h1>{channel}</h1><p class=\"ok\">Local server is up.</p>\
             <p>aika-rs &mdash; Aika server in Rust.</p></body></html>"
        ),
    }
}

/// Population per channel, space separated. `-1` marks a channel offline.
fn server_status(state: &State) -> String {
    let mut values: Vec<String> =
        state.cfg.servers.iter().map(|server| server.online.to_string()).collect();
    while values.len() < state.cfg.web.pad_status_to {
        values.push("-1".to_string());
    }
    values.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, DevAccount, DevCharacter, ServerEntry};
    use crate::http::Params;

    fn dev_character(name: &str, slot: usize) -> DevCharacter {
        DevCharacter {
            name: name.into(),
            slot,
            level: 1,
            class_index: 10,
            hair: 7700,
            nation: 2,
            gold: 0,
            exp: 0,
            x: None,
            y: None,
            speed_move: None,
        }
    }

    fn state() -> State {
        let cfg = Config {
            servers: vec![
                ServerEntry { name: "Teste1".into(), online: 7 },
                ServerEntry { name: "Teste2".into(), online: -1 },
            ],
            accounts: vec![DevAccount {
                username: "admin".into(),
                password: Some("admin".into()),
                password_hash: None,
                nation: 2,
                account_status: 0,
                ban_days: 0,
                characters: vec![dev_character("Athus", 0), dev_character("Pran", 1)],
            }],
            ..Default::default()
        };
        State::new(cfg).unwrap()
    }

    fn request(path: &str, query: &str) -> Request {
        Request {
            method: "POST".into(),
            path: path.into(),
            params: Params::parse(query),
            remote: "127.0.0.1".parse().unwrap(),
        }
    }

    #[test]
    fn issues_token_then_reports_characters() {
        let state = state();

        let token = route(&state, &request(ROUTE_GET_TOKEN, "id=admin&pw=admin")).body;
        assert_eq!(token.len(), 32, "expected a token, got {token:?}");

        let response = route(
            &state,
            &request(ROUTE_GET_CHRCNT, &format!("id=admin&pw={token}")),
        );
        assert_eq!(response.body, "CNT 2 0 0 0<br>2 0 0 0");
    }

    #[test]
    fn rejects_wrong_password_and_unknown_account() {
        let state = state();
        assert_eq!(route(&state, &request(ROUTE_GET_TOKEN, "id=admin&pw=x")).body, "-1");
        assert_eq!(route(&state, &request(ROUTE_GET_TOKEN, "id=zzz&pw=x")).body, "0");
    }

    #[test]
    fn char_count_requires_valid_token() {
        let state = state();
        assert_eq!(route(&state, &request(ROUTE_GET_CHRCNT, "id=admin&pw=nada")).body, "-1");
    }

    #[test]
    fn reports_server_population() {
        let state = state();
        let response = route(&state, &request("/servers/serv00.asp", ""));
        assert_eq!(response.body, "7 -1");
        // any channel index lands on the same route
        assert!(is_server_status_route("/servers/serv01.asp"));
        assert!(!is_server_status_route("/servers/servXX.asp"));
        assert!(!is_server_status_route("/member/aika_get_token.asp"));
    }

    #[test]
    fn reset_flag_renews_valid_token() {
        let state = state();
        let token = route(&state, &request(ROUTE_GET_TOKEN, "id=admin&pw=admin")).body;
        let renewed =
            route(&state, &request(ROUTE_RESET_FLAG, &format!("id=admin&pw={token}")));
        assert_eq!(renewed.body, token);
        assert_eq!(route(&state, &request(ROUTE_RESET_FLAG, "id=admin&pw=nao")).body, "-1");
    }

    #[test]
    fn unknown_route_is_not_fatal() {
        let state = state();
        assert_eq!(route(&state, &request("/qualquer.asp", "")).status, 404);
    }

    /// The launcher compares byte for byte with the `update.dat` it already
    /// has; any format difference makes it think a patch is pending.
    #[test]
    fn patch_info_matches_the_original_update_dat() {
        let state = state();
        let response = route(&state, &request(ROUTE_PATCH_INFO, ""));
        assert_eq!(response.body, "[AIKA] \r\n301 valhalla301.zip");
        assert_eq!(response.body.len(), 28, "the original file is 28 bytes");
    }

    #[test]
    fn launcher_page_is_served() {
        let state = state();
        let response = route(&state, &request(ROUTE_LAUNCHER, ""));
        assert_eq!(response.status, 200);
        assert!(response.body.contains("Teste1"), "shows the channel name");
    }
}
