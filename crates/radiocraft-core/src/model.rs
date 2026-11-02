//! Core domain model types (mirrors rdjInterface.SongData and schema semantics).

use serde::{Deserialize, Serialize};

/// songs.song_type registry (docs/03 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(i32)]
pub enum TrackType {
    Music = 0,
    Jingle = 1,
    Sweeper = 2,
    Voiceover = 3,
    Commercial = 4,
    InternetStream = 5,
    Other = 6,
    VDF = 7,
    Podcast = 8,
    Request = 9,
    News = 10,
    PlaylistEvent = 11,
    FileByDate = 12,
    NewestFromFolder = 13,
    Teaser = 14,
}

impl TrackType {
    pub fn from_i32(v: i32) -> Option<Self> {
        match v {
            0 => Some(Self::Music),
            1 => Some(Self::Jingle),
            2 => Some(Self::Sweeper),
            3 => Some(Self::Voiceover),
            4 => Some(Self::Commercial),
            5 => Some(Self::InternetStream),
            6 => Some(Self::Other),
            7 => Some(Self::VDF),
            8 => Some(Self::Podcast),
            9 => Some(Self::Request),
            10 => Some(Self::News),
            11 => Some(Self::PlaylistEvent),
            12 => Some(Self::FileByDate),
            13 => Some(Self::NewestFromFolder),
            14 => Some(Self::Teaser),
            _ => None,
        }
    }
}

impl From<TrackType> for i32 {
    fn from(t: TrackType) -> Self {
        t as i32
    }
}

/// Track record, port of rdjInterface.SongData (analysis/songdata-members.txt).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SongData {
    pub id: i64,
    pub id_genre: i64,
    pub id_subcat: i64,
    pub track_no: i32,
    pub artist: String,
    pub original_artist: String,
    pub associated_artists: String,
    pub title: String,
    pub album: String,
    pub album_art: String,
    pub composer: String,
    pub label: String,
    pub publisher: String,
    pub copyright: String,
    pub isrc: String,
    pub year: i32,
    pub disc_no: i32,
    pub language: String,
    pub mix_type: String,
    pub mood: String,
    pub gender: String,
    pub rating: String,
    pub comments: String,
    pub alternate_title: String,
    pub buy_link: String,
    pub url1: String,
    pub url2: String,
    pub path: String,
    pub enabled: bool,
    pub duration: f64,
    pub original_duration: f64,
    pub elapsed: f64,
    pub remaining: f64,
    pub track_type: i32,
    pub cue_start: f64,
    pub cue_end: f64,
    pub cue_next: f64,
    pub cue_intro: f64,
    pub cue_outro: f64,
    pub cue_loop_in: f64,
    pub cue_loop_out: f64,
    pub cue_hook_in: f64,
    pub cue_hook_out: f64,
    pub cue_times: String,
    pub bpm: i32,
    pub loudness: f64,
    pub bs1770: f64,
    pub high_precision: bool,
    pub weight: f64,
    pub play_limit: i32,
    pub limit_action: i32,
    pub start_date: Option<chrono::NaiveDateTime>,
    pub end_date: Option<chrono::NaiveDateTime>,
    pub start_event: Option<chrono::NaiveDateTime>,
    pub end_event: Option<chrono::NaiveDateTime>,
    pub start_type: i32,
    pub end_type: i32,
    pub fade_in: i32,
    pub fade_out: i32,
    pub is_overlay: bool,
    pub use_stream_title: bool,
    pub vt_id: i64,
    pub player_id: i64,
    pub sw_id: i64,
    pub sweepers: String,
    pub sw_first: i32,
    pub sw_play: i32,
    pub sound_card_output: String,
    pub auto_free: bool,
    pub date_added: Option<chrono::NaiveDateTime>,
    pub date_modified: Option<chrono::NaiveDateTime>,
}
