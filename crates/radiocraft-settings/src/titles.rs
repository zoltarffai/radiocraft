//! `settings_titles.xml` — per-track-type display titles and colors
//! (docs/02 §1). Colors are .NET `ColorTranslator` ARGB integers.

settings_struct! {
    /// Options persisted in `settings_titles.xml`. Each track type has a
    /// `Color_<Type>` and `Title_<Type>` pair; the title is a template that
    /// goes through the `$var$` engine.
    pub struct TitlesSettings {
        "Color_Music" => color_music: i32 = (-8372160),
        "Title_Music" => title_music: String = "$artist$ - $title$",
        "Color_Jingle" => color_jingle: i32 = (-65536),
        "Title_Jingle" => title_jingle: String = "$station_name$ - $station_slogan$",
        "Color_Sweeper" => color_sweeper: i32 = (-16744448),
        "Title_Sweeper" => title_sweeper: String = "$station_name$ - $station_slogan$",
        "Color_Voiceover" => color_voiceover: i32 = (-32768),
        "Title_Voiceover" => title_voiceover: String = "$station_name$ - $station_slogan$",
        "Color_Commercial" => color_commercial: i32 = (-16777088),
        "Title_Commercial" => title_commercial: String = "$station_name$ - $station_slogan$",
        "Color_InternetStream" => color_internet_stream: i32 = (-12582784),
        "Title_InternetStream" => title_internet_stream: String = "$artist$ - $title$",
        "Color_Other" => color_other: i32 = (-10395295),
        "Title_Other" => title_other: String = "$artist$ - $title$",
        "Color_VDF" => color_vdf: i32 = (-10395295),
        "Title_VDF" => title_vdf: String = "$artist$ - $title$",
        "Color_Podcast" => color_podcast: i32 = (-10395295),
        "Title_Podcast" => title_podcast: String = "$artist$ - $title$",
        "Color_Request" => color_request: i32 = (-10395295),
        "Title_Request" => title_request: String = "$artist$ - $title$",
        "Color_News" => color_news: i32 = (-10395295),
        "Title_News" => title_news: String = "News $station_name$ - $station_slogan$",
        "EnableLineInTitle" => enable_line_in_title: bool = true,
        "LineInArtist" => line_in_artist: String = "-",
        "LineInTitle" => line_in_title: String = "LIVE!",
        "Color_PlaylistEvent" => color_playlist_event: i32 = (-10395295),
        "Title_PlaylistEvent" => title_playlist_event: String = "$station_name$ - $station_slogan$",
        "Color_TimedFile" => color_timed_file: i32 = (-10395295),
        "Title_TimedFile" => title_timed_file: String = "$artist$ - $title$",
        "Color_NewestFromFolder" => color_newest_from_folder: i32 = (-10395295),
        "Title_NewestFromFolder" => title_newest_from_folder: String = "$artist$ - $title$",
        "Color_FileByDate" => color_file_by_date: i32 = (-10395295),
        "Title_FileByDate" => title_file_by_date: String = "$artist$ - $title$",
        "Color_Teaser" => color_teaser: i32 = (-10395295),
        "Title_Teaser" => title_teaser: String = "$station_name$ - $station_slogan$",
    }
}

impl TitlesSettings {
    /// The file this setting set is normally stored in.
    pub const FILE: &'static str = "settings_titles.xml";

    /// Display title template for a numeric track type (docs/02 §7 type list).
    pub fn title_for_type(&self, track_type: i32) -> Option<&str> {
        let s = match track_type {
            0 => &self.title_music,
            1 => &self.title_jingle,
            2 => &self.title_sweeper,
            3 => &self.title_voiceover,
            4 => &self.title_commercial,
            5 => &self.title_internet_stream,
            6 => &self.title_other,
            7 => &self.title_vdf,
            8 => &self.title_podcast,
            9 => &self.title_request,
            10 => &self.title_news,
            11 => &self.title_playlist_event,
            12 => &self.title_file_by_date,
            13 => &self.title_newest_from_folder,
            14 => &self.title_teaser,
            _ => return None,
        };
        Some(s)
    }

    /// Display color for a numeric track type.
    pub fn color_for_type(&self, track_type: i32) -> Option<i32> {
        let c = match track_type {
            0 => self.color_music,
            1 => self.color_jingle,
            2 => self.color_sweeper,
            3 => self.color_voiceover,
            4 => self.color_commercial,
            5 => self.color_internet_stream,
            6 => self.color_other,
            7 => self.color_vdf,
            8 => self.color_podcast,
            9 => self.color_request,
            10 => self.color_news,
            11 => self.color_playlist_event,
            12 => self.color_file_by_date,
            13 => self.color_newest_from_folder,
            14 => self.color_teaser,
            _ => return None,
        };
        Some(c)
    }
}
