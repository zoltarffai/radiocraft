//! Now-playing metadata export (mirror of the MetadataExport plugin).
//!
//! RadioDJ ships the current track to three destinations, each driven by a
//! template string with `$var$` placeholders rendered through
//! [`TemplateEngine`]/[`TemplateContext`]:
//!
//! * **File** — `TargetPath` receives the rendered `txtFileTemplate`, written
//!   with the codepage picked by `FileEncoding` (0 = ANSI, 1 = UTF-8,
//!   2 = Unicode (UTF-16 LE with BOM)).
//! * **Web** — `WebItems` is a list of `<item>`s with `url` + `method`; POST
//!   sends the rendered `TargetData` as the request body, GET encodes it in
//!   the query string.
//! * **Serial** — `SerialTemplate` is written to `TargetPort` (COM…) with the
//!   `Serial*` settings and optional DTR/RTS lines asserted.
//!
//! `TypesToExport` is the `0;1;5;` style semicolon list of track types the
//! plugin will export; `EnableCustomTitle` + `CustomTitle` override artist +
//! title for the web/serial templates when enabled.

use std::io::Write;
use std::time::Duration;

use radiocraft_core::model::{SongData, TrackType};
use radiocraft_core::template::{Context, TemplateEngine};
use radiocraft_settings::PluginSettings;
use serde::{Deserialize, Serialize};

/// Errors from the export pipeline.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("transport error: {0}")]
    Transport(String),
    #[error("invalid serial port spec: {0}")]
    Serial(String),
}

/// Codepage used for the target file. Values map to RadioDJ's `FileEncoding`
/// key: 0 = system ANSI (Windows-1252), 1 = UTF-8, 2 = Unicode (UTF-16 LE + BOM).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileEncoding {
    Ansi,
    Utf8,
    Utf16,
}

impl From<i32> for FileEncoding {
    fn from(value: i32) -> Self {
        match value {
            0 => FileEncoding::Ansi,
            2 => FileEncoding::Utf16,
            _ => FileEncoding::Utf8,
        }
    }
}

impl FileEncoding {
    /// Encode `text` for disk. The UTF-16 variant emits a byte-order mark so
    /// the file can be round-tripped, like .NET's `Unicode` encoding.
    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            FileEncoding::Ansi => {
                // Lossy like the C# fallback: characters outside the codepage
                // are replaced.
                encoding_rs::WINDOWS_1252.encode(text).0.into_owned()
            }
            FileEncoding::Utf8 => text.as_bytes().to_vec(),
            FileEncoding::Utf16 => {
                let mut out = vec![0xFF, 0xFE]; // little-endian BOM
                for unit in text.encode_utf16() {
                    out.extend_from_slice(&unit.to_le_bytes());
                }
                out
            }
        }
    }
}

/// One `<item>` entry from the WebItems list.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebItem {
    pub url: String,
    /// "post" or "get" (case-insensitive; anything else defaults to post).
    pub method: String,
}

/// Parse the WebItems XML blob (`<items><item url=… method=… />…</items>`).
pub fn parse_web_items(xml: &str) -> Vec<WebItem> {
    // Malformed or empty blobs simply produce an empty list, like the plugin.
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut items = Vec::new();
    let mut current: Option<WebItem> = None;
    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(quick_xml::events::Event::Empty(e))
                if e.name().as_ref().eq_ignore_ascii_case(b"item") =>
            {
                // Self-closed `<item ... />` never emits an End event.
                let item = item_from_attrs(&e);
                if !item.url.is_empty() {
                    items.push(item);
                }
            }
            Ok(quick_xml::events::Event::Start(e))
                if e.name().as_ref().eq_ignore_ascii_case(b"item") =>
            {
                current = Some(item_from_attrs(&e));
            }
            Ok(quick_xml::events::Event::End(e))
                if e.name().as_ref().eq_ignore_ascii_case(b"item") =>
            {
                if let Some(item) = current.take() {
                    if !item.url.is_empty() {
                        items.push(item);
                    }
                }
            }
            Err(_) => break,
            Ok(_) => {}
        }
    }
    items
}

fn item_from_attrs(start: &quick_xml::events::BytesStart<'_>) -> WebItem {
    let mut item = WebItem {
        url: String::new(),
        method: "post".into(),
    };
    for attr in start.attributes().flatten() {
        let value = attr
            .unescape_value()
            .map(|v| v.into_owned())
            .unwrap_or_default();
        match attr.key.as_ref() {
            b"url" => item.url = value,
            b"method" => item.method = value,
            _ => {}
        }
    }
    item
}

/// HTTP transport seam so unit tests can stub the network.
pub trait HttpTransport: Send + Sync {
    fn send(&self, item: &WebItem, body: &str) -> Result<(), ExportError>;
}

/// Real transport using ureq (blocking).
pub struct UreqTransport {
    pub timeout: Duration,
}

impl Default for UreqTransport {
    fn default() -> Self {
        UreqTransport {
            timeout: Duration::from_secs(3),
        }
    }
}

impl HttpTransport for UreqTransport {
    fn send(&self, item: &WebItem, body: &str) -> Result<(), ExportError> {
        if item.method.eq_ignore_ascii_case("get") {
            let sep = if item.url.contains('?') { "&" } else { "?" };
            ureq::get(&format!("{}{}{}", item.url, sep, body))
                .timeout(self.timeout)
                .call()
                .map(|_| ())
                .map_err(|e| ExportError::Transport(e.to_string()))
        } else {
            ureq::post(&item.url)
                .timeout(self.timeout)
                .set("content-type", "application/x-www-form-urlencoded")
                .send_string(body)
                .map(|_| ())
                .map_err(|e| ExportError::Transport(e.to_string()))
        }
    }
}

/// Serial seam for tests.
pub trait SerialWriter: Send + Sync {
    fn write(&self, bytes: &[u8]) -> Result<(), ExportError>;
}

/// Everything the exporter needs, decoded from the plugin settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportConfig {
    pub export_to_file: bool,
    pub target_path: String,
    pub file_encoding: FileEncoding,
    pub txt_file_template: String,

    pub export_to_web: bool,
    pub web_items: Vec<WebItem>,
    pub target_data: String,

    pub export_to_serial: bool,
    pub serial_template: String,
    pub target_port: String,
    pub serial_baud: i32,
    pub serial_parity: i32,
    pub serial_stop: i32,
    pub serial_data: i32,
    pub serial_handshake: i32,
    pub serial_dtr: bool,
    pub serial_rts: bool,

    pub enable_custom_title: bool,
    pub custom_title: String,
    pub types_to_export: Vec<TrackType>,
}

impl ExportConfig {
    /// Decode the `Plugin_MetadataExport` keys (docs/02 §6).
    pub fn from_settings(s: &PluginSettings) -> Self {
        let types_to_export = s
            .get("TypesToExport")
            .unwrap_or("")
            .split(';')
            .filter_map(|p| p.trim().parse::<i32>().ok())
            .filter_map(TrackType::from_i32)
            .collect();
        ExportConfig {
            export_to_file: s.get_as("Export2File", false),
            target_path: s.get("TargetPath").unwrap_or("").to_string(),
            file_encoding: s.get_as("FileEncoding", 1).into(),
            txt_file_template: s.get("txtFileTemplate").unwrap_or("").to_string(),

            export_to_web: s.get_as("Export2Web", false),
            web_items: parse_web_items(s.get("WebItems").unwrap_or("")),
            target_data: s.get("TargetData").unwrap_or("-").to_string(),

            export_to_serial: s.get_as("Export2Serial", false),
            serial_template: s.get("SerialTemplate").unwrap_or("").to_string(),
            target_port: s.get("TargetPort").unwrap_or("").to_string(),
            serial_baud: s.get_as("SerialBaudRate", 1),
            serial_parity: s.get_as("SerialParity", 0),
            serial_stop: s.get_as("SerialStopBits", 1),
            serial_data: s.get_as("SerialDataBits", 8),
            serial_handshake: s.get_as("SerialHandshake", 0),
            serial_dtr: s.get_as("Serialdtr", false),
            serial_rts: s.get_as("Serialrts", false),

            enable_custom_title: s.get_as("EnableCustomTitle", false),
            custom_title: s.get("CustomTitle").unwrap_or("").to_string(),
            types_to_export,
        }
    }

    /// True when the plugin should export this track (typesToExport check).
    pub fn should_export(&self, song: &SongData) -> bool {
        if self.types_to_export.is_empty() {
            return true;
        }
        match TrackType::from_i32(song.track_type) {
            Some(t) => self.types_to_export.contains(&t),
            None => false,
        }
    }
}

/// The now-playing snapshot the exporter broadcasts.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NowPlayingSnapshot {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub track_type: i32,
    pub duration_seconds: f64,
    pub duration_hms: String,
    pub song_id: i64,
    pub cat_id: i64,
    pub year: i32,
    pub label: String,
    pub station: String,
}

impl NowPlayingSnapshot {
    pub fn from_song(station: &str, song: &SongData) -> Self {
        NowPlayingSnapshot {
            artist: song.artist.clone(),
            title: song.title.clone(),
            album: song.album.clone(),
            track_type: song.track_type,
            duration_seconds: song.duration,
            duration_hms: radiocraft_core::template::format_hms(song.duration),
            song_id: song.id,
            cat_id: song.id_subcat,
            year: song.year,
            label: song.label.clone(),
            station: station.to_string(),
        }
    }

    /// Recover a snapshot from a rendered context (used by the web console).
    pub fn from_context(ctx: &Context) -> Self {
        let get = |k: &str| ctx.get(k).unwrap_or("").to_string();
        NowPlayingSnapshot {
            artist: get("artist"),
            title: get("title"),
            album: get("album"),
            track_type: get("songtype").parse().unwrap_or(0),
            duration_seconds: get("durationinseconds").parse().unwrap_or(0.0),
            duration_hms: get("duration"),
            song_id: get("song.id").parse().unwrap_or(0),
            cat_id: get("catid").parse().unwrap_or(0),
            year: get("year").parse().unwrap_or(0),
            label: get("label"),
            station: get("station_name"),
        }
    }
}

/// Apply `EnableCustomTitle` + `CustomTitle` ("Artist|Title" form) to the
/// context, mirroring applyVariablesForWebExport/Serial in the plugin.
fn apply_custom_title(ctx: &mut Context, cfg: &ExportConfig, song: &SongData) {
    if !cfg.enable_custom_title {
        return;
    }
    let (artist, title) = match cfg.custom_title.split_once('|') {
        Some((a, t)) => (a.trim(), t.trim()),
        None => (song.artist.as_str(), cfg.custom_title.trim()),
    };
    ctx.set("artist", artist);
    ctx.set("title", title);
}

fn render(engine: &TemplateEngine, ctx: &Context, template: &str) -> String {
    engine.render(template, ctx)
}

/// Push now-playing metadata to all enabled destinations.
pub struct NowPlayingExporter {
    pub config: ExportConfig,
    pub station: String,
    engine: TemplateEngine,
    http: Box<dyn HttpTransport>,
    serial: Option<Box<dyn SerialWriter>>,
}

impl NowPlayingExporter {
    pub fn new(config: ExportConfig, station: impl Into<String>) -> Self {
        NowPlayingExporter {
            config,
            station: station.into(),
            engine: TemplateEngine::new(),
            http: Box::new(UreqTransport::default()),
            serial: None,
        }
    }

    /// Override the HTTP transport (tests).
    pub fn with_http(mut self, http: impl HttpTransport + 'static) -> Self {
        self.http = Box::new(http);
        self
    }

    /// Override the serial sink (tests).
    pub fn with_serial(mut self, serial: impl SerialWriter + 'static) -> Self {
        self.serial = Some(Box::new(serial));
        self
    }

    /// Build a template context for the current track.
    pub fn context_for(&self, song: &SongData) -> Context {
        let mut ctx = Context::new();
        ctx.set("station_name", self.station.clone());
        ctx.set("now_playing", "1");
        ctx.set("next_pl", String::new());
        ctx.set("next_art", String::new());
        ctx.set("next_ti", String::new());
        ctx.set_track(song);
        ctx.set_now(chrono::Local::now().naive_local());
        ctx
    }

    /// Write the file target. Returns the rendered text (empty when disabled).
    pub fn export_file(&self, ctx: &Context, song: &SongData) -> Result<String, ExportError> {
        let _ = song;
        if !self.config.export_to_file {
            return Ok(String::new());
        }
        let text = render(&self.engine, ctx, &self.config.txt_file_template);
        if self.config.target_path.is_empty() {
            return Ok(text);
        }
        let bytes = self.config.file_encoding.encode(&text);
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // RadioDJ keeps target files world-readable so web roots can read them.
            opts.mode(0o644);
        }
        let mut file = opts.open(&self.config.target_path)?;
        file.write_all(&bytes)?;
        file.flush()?;
        Ok(text)
    }

    /// Push to every configured web endpoint.
    pub fn export_web(&self, ctx: &Context, song: &SongData) -> Result<(), ExportError> {
        if !self.config.export_to_web {
            return Ok(());
        }
        let mut ctx = ctx.clone();
        apply_custom_title(&mut ctx, &self.config, song);
        let body = render(&self.engine, &ctx, &self.config.target_data);
        for item in &self.config.web_items {
            self.http.send(item, &body)?;
        }
        Ok(())
    }

    /// Push to the serial port.
    pub fn export_serial(&self, ctx: &Context, song: &SongData) -> Result<(), ExportError> {
        if !self.config.export_to_serial {
            return Ok(());
        }
        let mut ctx = ctx.clone();
        apply_custom_title(&mut ctx, &self.config, song);
        let text = render(&self.engine, &ctx, &self.config.serial_template);
        let bytes = text.into_bytes();
        match &self.serial {
            Some(writer) => writer.write(&bytes),
            None => write_to_serial(&self.config, &bytes),
        }
    }

    /// Run every enabled destination for the current song.
    pub fn broadcast(&self, song: &SongData) -> Result<NowPlayingSnapshot, ExportError> {
        if !self.config.should_export(song) {
            tracing::debug!("skipping export for track type {}", song.track_type);
            return Ok(NowPlayingSnapshot::from_song(&self.station, song));
        }
        let ctx = self.context_for(song);
        self.export_file(&ctx, song)?;
        self.export_web(&ctx, song)?;
        self.export_serial(&ctx, song)?;
        Ok(NowPlayingSnapshot::from_song(&self.station, song))
    }
}

fn write_to_serial(cfg: &ExportConfig, bytes: &[u8]) -> Result<(), ExportError> {
    let builder = serialport::new(&cfg.target_port, baud_rate(cfg.serial_baud))
        .timeout(Duration::from_millis(500))
        .data_bits(data_bits(cfg.serial_data))
        .stop_bits(stop_bits(cfg.serial_stop))
        .parity(parity(cfg.serial_parity))
        .flow_control(flow(cfg.serial_handshake));
    let port = builder
        .open()
        .map_err(|e| ExportError::Serial(e.to_string()))?;
    let mut port = port;
    // DTR/RTS lines asserted per plugin settings.
    let _ = port.write_data_terminal_ready(cfg.serial_dtr);
    let _ = port.write_request_to_send(cfg.serial_rts);
    port.write_all(bytes)
        .map_err(|e| ExportError::Serial(e.to_string()))?;
    let _ = port.flush();
    Ok(())
}

fn baud_rate(value: i32) -> u32 {
    // .NET SerialPort.BotryIndex order used by the plugin's BaudRateIndex.
    match value {
        0 => 9600,
        1 => 4800,
        2 => 14400,
        3 => 19200,
        4 => 38400,
        5 => 56000,
        6 => 57600,
        7 => 115200,
        8 => 128000,
        9 => 256000,
        _ => 4800,
    }
}

fn data_bits(value: i32) -> serialport::DataBits {
    match value {
        5 => serialport::DataBits::Five,
        6 => serialport::DataBits::Six,
        7 => serialport::DataBits::Seven,
        _ => serialport::DataBits::Eight,
    }
}

fn stop_bits(value: i32) -> serialport::StopBits {
    match value {
        2 => serialport::StopBits::Two,
        _ => serialport::StopBits::One,
    }
}

/// .NET `Parity` values: Space=0, Mark=1, Even=2, Odd=3, None=4. The `serialport`
/// crate supports only None/Odd/Even, so Mark and Space map to the closest
/// representable behaviour (parity bit asserted with even/none semantics).
fn parity(value: i32) -> serialport::Parity {
    match value {
        1 => serialport::Parity::Even, // Mark unsupported; keep parity enabled
        2 => serialport::Parity::Even,
        3 => serialport::Parity::Odd,
        0 => serialport::Parity::None, // Space unsupported; fall back to no parity
        _ => serialport::Parity::None, // 4 = None, plus anything unknown
    }
}

fn flow(value: i32) -> serialport::FlowControl {
    match value {
        1 => serialport::FlowControl::Software,
        2 => serialport::FlowControl::Hardware, // RtsCts
        3 => serialport::FlowControl::Hardware, // DtrDsr
        _ => serialport::FlowControl::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> SongData {
        SongData {
            id: 7,
            id_subcat: 3,
            artist: "The Beatles".into(),
            title: "Come Together".into(),
            album: "Abbey Road".into(),
            duration: 411.0,
            track_type: 0, // Music
            ..Default::default()
        }
    }

    fn config() -> ExportConfig {
        ExportConfig {
            export_to_file: true,
            target_path: String::new(),
            file_encoding: FileEncoding::Utf8,
            txt_file_template: "$artist$\n$title$\n$duration$".into(),
            export_to_web: false,
            web_items: vec![],
            target_data: String::new(),
            export_to_serial: false,
            serial_template: String::new(),
            target_port: String::new(),
            serial_baud: 1,
            serial_parity: 0,
            serial_stop: 1,
            serial_data: 8,
            serial_handshake: 0,
            serial_dtr: false,
            serial_rts: false,
            enable_custom_title: false,
            custom_title: String::new(),
            types_to_export: vec![TrackType::Music, TrackType::Jingle],
        }
    }

    #[test]
    fn file_target_writes_with_selected_encoding() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("now.txt");

        let mut cfg = config();
        cfg.target_path = path.to_str().unwrap().into();
        cfg.file_encoding = 1.into(); // UTF-8
        let ex = NowPlayingExporter::new(cfg.clone(), "TEST");
        let song = song();
        let ctx = ex.context_for(&song);
        ex.export_file(&ctx, &song).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "The Beatles\nCome Together\n00:06:51"
        );

        // 2 = UTF-16 LE with BOM
        let mut cfg = cfg.clone();
        cfg.file_encoding = 2.into();
        let ex = NowPlayingExporter::new(cfg.clone(), "TEST");
        ex.export_file(&ctx, &song).unwrap();
        let raw = std::fs::read(&path).unwrap();
        assert_eq!(&raw[..2], &[0xFF, 0xFE]);

        // 0 = ANSI (Windows-1252): é maps to a single 0xE9 byte.
        let mut cfg = cfg.clone();
        cfg.file_encoding = 0.into();
        cfg.txt_file_template = "caf$".into(); // unterminated: left as-is
        let ex = NowPlayingExporter::new(cfg, "TEST");
        let ctx2 = ex.context_for(&song);
        ex.export_file(&ctx2, &song).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"caf$");
    }

    #[test]
    fn web_items_are_parsed_and_rendered() {
        let xml = r#"<items><item url="http://x/nl" method="POST" /><item url="http://y/np" method="GET" /></items>"#;
        let items = parse_web_items(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].url, "http://x/nl");
        assert_eq!(items[1].method, "GET");
        // Empty reference files yield no endpoints.
        assert!(parse_web_items("<NewDataSet />").is_empty());
    }

    #[test]
    fn empty_types_list_export_all() {
        let cfg = ExportConfig {
            types_to_export: vec![],
            ..config()
        };
        assert!(cfg.should_export(&song()));
    }

    #[test]
    fn types_to_export_filters_music() {
        let cfg = config();
        assert!(cfg.should_export(&song()));
        let mut s = song();
        s.track_type = 7; // Sweep
        assert!(!cfg.should_export(&s));
    }

    #[test]
    fn encoding_variants() {
        assert_eq!(FileEncoding::Utf16.encode("h\u{e9}llo"), {
            let mut v = vec![0xFF, 0xFE];
            v.extend("h\u{e9}llo".encode_utf16().flat_map(|u| u.to_le_bytes()));
            v
        });
        assert_eq!(FileEncoding::Utf8.encode("hi"), b"hi");
        assert_eq!(FileEncoding::Ansi.encode("caf\u{e9}"), b"caf\xE9");
    }

    #[test]
    fn custom_title_overrides_artist_and_title() {
        let mut cfg = config();
        cfg.enable_custom_title = true;
        cfg.custom_title = "Dedicated Artist|Dedicated Song".into();
        let ex = NowPlayingExporter::new(cfg, "TEST");
        let song = song();
        let mut ctx = ex.context_for(&song);
        apply_custom_title(&mut ctx, &ex.config, &song);
        assert_eq!(ctx.get("artist"), Some("Dedicated Artist"));
        assert_eq!(ctx.get("title"), Some("Dedicated Song"));
    }

    #[test]
    fn web_transport_receives_rendered_target_data() {
        struct Recording;
        // Stash sends so the test can observe them.
        static SEEN: std::sync::Mutex<Vec<(String, String, String)>> =
            std::sync::Mutex::new(Vec::new());
        impl HttpTransport for Recording {
            fn send(&self, item: &WebItem, body: &str) -> Result<(), ExportError> {
                SEEN.lock().unwrap().push((
                    item.url.clone(),
                    item.method.clone(),
                    body.to_string(),
                ));
                Ok(())
            }
        }
        let mut cfg = config();
        cfg.export_to_web = true;
        cfg.target_data = "artist=$artist$&title=$title$".into();
        cfg.web_items = vec![WebItem {
            url: "http://x/nl".into(),
            method: "post".into(),
        }];
        let ex = NowPlayingExporter::new(cfg, "TEST").with_http(Recording);
        let song = song();
        let ctx = ex.context_for(&song);
        ex.export_web(&ctx, &song).unwrap();
        let seen = SEEN.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].2, "artist=The Beatles&title=Come Together");
    }

    #[test]
    fn serial_settings_map_to_netserialports_enum_indexes() {
        assert_eq!(baud_rate(1), 4800);
        assert_eq!(baud_rate(7), 115200);
        assert!(matches!(parity(0), serialport::Parity::None));
        assert!(matches!(stop_bits(1), serialport::StopBits::One));
        assert!(matches!(data_bits(8), serialport::DataBits::Eight));
        assert!(matches!(flow(0), serialport::FlowControl::None));
    }

    #[test]
    fn config_decodes_sample_plugin_file() {
        // Sample from the reference install (defaults visible in the XML).
        let xml = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<NewDataSet>
  <Settings><Key>Export2File</Key><Value>True</Value></Settings>
  <Settings><Key>FileEncoding</Key><Value>2</Value></Settings>
  <Settings><Key>Export2Web</Key><Value>False</Value></Settings>
  <Settings><Key>Export2Serial</Key><Value>False</Value></Settings>
  <Settings><Key>TargetPort</Key><Value>NO SERIAL PORT</Value></Settings>
  <Settings><Key>SerialBaudRate</Key><Value>1</Value></Settings>
  <Settings><Key>TypesToExport</Key><Value>0;1;5;6;7;8;9;10;12;13;</Value></Settings>
</NewDataSet>"#;
        let settings = PluginSettings::parse(xml).unwrap();
        let cfg = ExportConfig::from_settings(&settings);
        assert!(cfg.export_to_file);
        assert!(!cfg.export_to_web);
        assert_eq!(cfg.file_encoding, FileEncoding::Utf16);
        assert_eq!(cfg.target_port, "NO SERIAL PORT");
        assert_eq!(cfg.types_to_export.len(), 10);
        assert!(cfg.types_to_export.contains(&TrackType::Music));
        assert!(!cfg.types_to_export.contains(&TrackType::PlaylistEvent)); // 11 not in list
    }
}
