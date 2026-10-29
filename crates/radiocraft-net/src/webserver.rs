//! The embedded HTTP server (mirror of RadioDJ's internal web server + the
//! `WebAPI`/`NowPlayingTemplate` surfaces from docs/02 §9).
//!
//! RadioDJ, when the web server is enabled, serves:
//!
//! * `GET /nowplaying` — the rendered now-playing text (the configured now
//!   playing template is stored under a `templates` directory and rendered
//!   with `$var$` placeholders; `GET /nowplaying.tpl` serves a raw template
//!   file by name).
//! * `GET /api/nowplaying` — JSON snapshot of the current track.
//! * `POST /api/request` — song requests, forwarded to the host application
//!   through the [`RequestSink`] seam.
//!
//! The server runs on its own thread (tiny_http is blocking), listens on the
//! configurable port (`WebServerPort` lives in the general settings) and can
//! be stopped through the returned handle.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use radiocraft_core::template::{Context, TemplateEngine};

use crate::nowplaying::NowPlayingSnapshot;

/// Errors from the web server.
#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(String),
    #[error("no longer playing: nothing scheduled")]
    NoTrack,
}

/// A song request submitted over `POST /api/request`.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RequestSubmission {
    /// Requested file path or track name (form field `path`).
    pub path: String,
    /// Optional friendly title (form field `title`).
    pub title: String,
    /// Optional artist (form field `artist`).
    pub artist: String,
    /// Requesting user name (form field `user`).
    pub user: String,
}

/// Host application seam for incoming requests (the C# app pushes these onto
/// its request queue).
pub trait RequestSink: Send + Sync {
    fn submit(&self, request: RequestSubmission) -> Result<(), String>;
}

/// Supplies the current now-playing state to the server.
pub trait NowPlayingProvider: Send + Sync {
    /// Snapshot of the current track (for the JSON API).
    fn snapshot(&self) -> Option<NowPlayingSnapshot>;
    /// Fully populated template context for the current track.
    fn context(&self) -> Context;
}

/// Configuration for the embedded server.
#[derive(Clone, Debug)]
pub struct WebServerConfig {
    /// Address to bind (`127.0.0.1:8000` style).
    pub bind: String,
    /// Directory holding `.tpl` now-playing templates.
    pub templates_dir: PathBuf,
    /// Template used by `/nowplaying` when it exists (searched in
    /// `templates_dir` in this order).
    pub default_template: String,
}

impl WebServerConfig {
    /// RadioDJ defaults: port 8000, `Templates` under the app directory.
    pub fn new(bind: impl Into<String>, templates_dir: impl Into<PathBuf>) -> Self {
        WebServerConfig {
            bind: bind.into(),
            templates_dir: templates_dir.into(),
            default_template: "nowplaying.tpl".into(),
        }
    }
}

/// Handle for a running server.
pub struct WebServer {
    shutdown: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl WebServer {
    /// Start serving. `provider` supplies now-playing state, `sink` receives
    /// requests. Returns the handle plus the *actual* bound address (useful
    /// when port 0 was requested).
    pub fn start(
        config: WebServerConfig,
        provider: Arc<dyn NowPlayingProvider>,
        sink: Arc<dyn RequestSink>,
    ) -> Result<(Self, String), WebError> {
        let server =
            tiny_http::Server::http(&config.bind).map_err(|e| WebError::Http(e.to_string()))?;
        let actual = match server.server_addr().to_ip() {
            Some(addr) => addr.to_string(),
            None => config.bind.clone(),
        };
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = shutdown.clone();
        let engine = TemplateEngine::new();
        let handle = std::thread::spawn(move || {
            loop {
                if flag.load(Ordering::SeqCst) {
                    break;
                }
                // try_recv style polling: respond to queued requests then exit.
                match server.try_recv() {
                    Ok(Some(request)) => {
                        handle_request(request, &config, &engine, &provider, &sink);
                    }
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
            // Drain any last request before exiting.
        });
        Ok((
            WebServer {
                shutdown,
                handle: Some(handle),
            },
            actual,
        ))
    }

    /// Signal the worker thread to stop and join it.
    pub fn join(mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn handle_request(
    mut request: tiny_http::Request,
    config: &WebServerConfig,
    engine: &TemplateEngine,
    provider: &Arc<dyn NowPlayingProvider>,
    sink: &Arc<dyn RequestSink>,
) {
    let url = request.url().to_string();
    let (path, _query) = match url.split_once('?') {
        Some((p, q)) => (p, q),
        None => (url.as_str(), ""),
    };
    let method = request.method().as_str().to_string();
    let body = {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(request.as_reader(), &mut buf);
        String::from_utf8_lossy(&buf).into_owned()
    };

    let response = match (method.as_str(), path) {
        ("GET", "/nowplaying") => serve_now_playing(engine, provider, config),
        ("GET", p) if p.starts_with("/nowplaying/") || p == "/nowplaying.tpl" => {
            serve_template_file(p, config)
        }
        ("GET", "/api/nowplaying") => serve_api_now_playing(provider),
        ("POST", "/api/request") => serve_request_api(&body, sink),
        _ => text(tiny_http::StatusCode(404), "Not Found"),
    };
    let _ = request.respond(response);
}

fn response(
    status: tiny_http::StatusCode,
    content_type: &str,
    body: impl Into<String>,
) -> tiny_http::Response<Cursor<Vec<u8>>> {
    let body = body.into().into_bytes();
    let len = Some(body.len());
    tiny_http::Response::new(
        status,
        vec![tiny_http::Header::from_bytes("Content-Type", content_type).unwrap()],
        Cursor::new(body),
        len,
        None,
    )
}

fn text(
    status: tiny_http::StatusCode,
    body: impl Into<String>,
) -> tiny_http::Response<Cursor<Vec<u8>>> {
    response(status, "text/plain; charset=utf-8", body)
}

fn html(body: impl Into<String>) -> tiny_http::Response<Cursor<Vec<u8>>> {
    response(tiny_http::StatusCode(200), "text/html; charset=utf-8", body)
}

fn json(body: impl Into<String>) -> tiny_http::Response<Cursor<Vec<u8>>> {
    response(tiny_http::StatusCode(200), "application/json", body)
}

fn serve_now_playing(
    engine: &TemplateEngine,
    provider: &Arc<dyn NowPlayingProvider>,
    config: &WebServerConfig,
) -> tiny_http::Response<Cursor<Vec<u8>>> {
    let ctx = provider.context();
    let template_path = config.templates_dir.join(&config.default_template);
    let rendered = match std::fs::read_to_string(&template_path) {
        Ok(tpl) => engine.render(&tpl, &ctx),
        Err(_) => {
            // Fall back to a simple "artist - title" line, like the app does
            // when no template file exists.
            let artist = ctx.get("artist").unwrap_or("");
            let title = ctx.get("title").unwrap_or("");
            format!("{artist} - {title}")
        }
    };
    html(rendered)
}

fn serve_template_file(
    path: &str,
    config: &WebServerConfig,
) -> tiny_http::Response<Cursor<Vec<u8>>> {
    // /nowplaying/<name>.tpl — serve a named template from the templates dir.
    let name = path.rsplit('/').next().unwrap_or("");
    if name.is_empty() || name.contains("..") || !name.ends_with(".tpl") {
        return text(tiny_http::StatusCode(400), "Bad template name");
    }
    let full = config.templates_dir.join(name);
    match std::fs::read_to_string(&full) {
        Ok(contents) => text(tiny_http::StatusCode(200), contents),
        Err(_) => text(tiny_http::StatusCode(404), "Template not found"),
    }
}

fn serve_api_now_playing(
    provider: &Arc<dyn NowPlayingProvider>,
) -> tiny_http::Response<Cursor<Vec<u8>>> {
    match provider.snapshot() {
        Some(snap) => match serde_json::to_string(&snap) {
            Ok(body) => json(body),
            Err(e) => text(
                tiny_http::StatusCode(500),
                format!("serialization failed: {e}"),
            ),
        },
        None => text(tiny_http::StatusCode(204), ""),
    }
}

fn serve_request_api(
    body: &str,
    sink: &Arc<dyn RequestSink>,
) -> tiny_http::Response<Cursor<Vec<u8>>> {
    // Accept both `application/x-www-form-urlencoded` and JSON bodies.
    let submission = if body.trim_start().starts_with('{') {
        serde_json::from_str::<RequestSubmission>(body)
    } else {
        Ok(parse_form(body))
    };
    match submission {
        Ok(sub) => match sink.submit(sub) {
            Ok(()) => json(r#"{"status":"ok"}"#),
            Err(msg) => text(
                tiny_http::StatusCode(400),
                format!("request rejected: {msg}"),
            ),
        },
        Err(e) => text(tiny_http::StatusCode(400), format!("bad request: {e}")),
    }
}

fn parse_form(body: &str) -> RequestSubmission {
    let mut sub = RequestSubmission::default();
    for pair in body.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            let v = percent_decode(v);
            match k {
                "path" => sub.path = v,
                "title" => sub.title = v,
                "artist" => sub.artist = v,
                "user" => sub.user = v,
                _ => {}
            }
        }
    }
    sub
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
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

/// A trivial provider backed by an in-memory snapshot (used by hosts that
/// keep the state centrally, and by tests).
pub struct StaticProvider {
    snapshot: NowPlayingSnapshot,
    context: Context,
}

impl StaticProvider {
    pub fn new(snapshot: NowPlayingSnapshot) -> Self {
        let mut context = Context::new();
        context.set("station_name", snapshot.station.clone());
        context.set("artist", snapshot.artist.clone());
        context.set("title", snapshot.title.clone());
        context.set("album", snapshot.album.clone());
        context.set("duration", snapshot.duration_hms.clone());
        context.set("songtype", snapshot.track_type.to_string());
        StaticProvider { snapshot, context }
    }
}

impl NowPlayingProvider for StaticProvider {
    fn snapshot(&self) -> Option<NowPlayingSnapshot> {
        Some(self.snapshot.clone())
    }
    fn context(&self) -> Context {
        self.context.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    struct CollectingSink {
        got: Mutex<Vec<RequestSubmission>>,
    }

    impl RequestSink for CollectingSink {
        fn submit(&self, r: RequestSubmission) -> Result<(), String> {
            self.got.lock().unwrap().push(r);
            Ok(())
        }
    }

    fn sample() -> NowPlayingSnapshot {
        NowPlayingSnapshot {
            artist: "Daft Punk".into(),
            title: "Get Lucky".into(),
            station: "TEST".into(),
            duration_hms: "00:06:09".into(),
            ..Default::default()
        }
    }

    struct Harness {
        server: WebServer,
        base: String,
        sink: Arc<CollectingSink>,
    }

    fn start(dir: &Path) -> Harness {
        let mut cfg = WebServerConfig::new("127.0.0.1:0", dir);
        cfg.default_template = "np.tpl".into();
        std::fs::write(dir.join("np.tpl"), "$artist$ | $title$").unwrap();
        let provider = Arc::new(StaticProvider::new(sample()));
        let sink = Arc::new(CollectingSink {
            got: Mutex::new(Vec::new()),
        });
        let (server, addr) = WebServer::start(cfg, provider, sink.clone()).unwrap();
        Harness {
            server,
            base: format!("http://{addr}"),
            sink,
        }
    }

    #[test]
    fn now_playing_uses_template_and_json_api_serves_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let h = start(dir.path());

        let body = ureq::get(&format!("{}/nowplaying", h.base))
            .call()
            .unwrap()
            .into_string()
            .unwrap();
        assert_eq!(body, "Daft Punk | Get Lucky");

        let j: serde_json::Value = ureq::get(&format!("{}/api/nowplaying", h.base))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(j["artist"], "Daft Punk");
        assert_eq!(j["title"], "Get Lucky");

        let tpl = ureq::get(&format!("{}/nowplaying/np.tpl", h.base))
            .call()
            .unwrap()
            .into_string()
            .unwrap();
        assert_eq!(tpl, "$artist$ | $title$");

        h.server.join();
    }

    #[test]
    fn post_request_forwards_to_sink() {
        let dir = tempfile::tempdir().unwrap();
        let h = start(dir.path());

        let resp = ureq::post(&format!("{}/api/request", h.base))
            .send_form(&[("path", "C:\\music\\song.mp3"), ("user", "joe fan")]);
        assert!(resp.is_ok());
        {
            // Guard must be released before the next request: the server
            // thread locks the same mutex in submit().
            let got = h.sink.got.lock().unwrap();
            assert_eq!(got.len(), 1);
            assert_eq!(got[0].path, "C:\\music\\song.mp3");
            assert_eq!(got[0].user, "joe fan");
        }

        // JSON body also accepted.
        let resp = ureq::post(&format!("{}/api/request", h.base))
            .send_json(serde_json::json!({"path": "/x/y", "user": "mary"}));
        assert!(resp.is_ok());
        assert_eq!(h.sink.got.lock().unwrap().len(), 2);

        h.server.join();
    }

    #[test]
    fn unknown_routes_are_404() {
        let dir = tempfile::tempdir().unwrap();
        let h = start(dir.path());
        let err = ureq::get(&format!("{}/nope", h.base)).call().unwrap_err();
        assert!(matches!(err, ureq::Error::Status(404, _)));
        h.server.join();
    }
}
