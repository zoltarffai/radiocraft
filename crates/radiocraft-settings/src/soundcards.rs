//! `settings_soundcards.xml` — audio-device / fade options (docs/02 §1, §2).



settings_struct! {
    /// Options persisted in `settings_soundcards.xml`.
    pub struct SoundcardSettings {
        "audio_mode" => audio_mode: i32 = 0,
        "InputFadeIn" => input_fade_in: i32 = 0,
        "InputFadeOut" => input_fade_out: i32 = 1500,
    }
}

impl SoundcardSettings {
    /// The file this setting set is normally stored in.
    pub const FILE: &'static str = "settings_soundcards.xml";
}
