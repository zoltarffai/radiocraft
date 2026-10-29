//! Zone mixer: four playout zones (main / aux / cart / instant) with
//! per-zone faders and mute, program-level ducking (`vol_onsweeper`,
//! `vol_overlay`, `vol_vt`) and a master output gain.

use serde::{Deserialize, Serialize};

use crate::util::{clamp_f64, db_to_lin, lin_to_db};

/// Playout zones as they appear in the RadioDJ player windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Zone {
    /// Main / linear player (the song queue).
    Main,
    /// Auxiliary player.
    Aux,
    /// Cart / buttons player.
    Cart,
    /// Instant-insert (news, station ID, on-demand files).
    Instant,
}

pub const ALL_ZONES: [Zone; 4] = [Zone::Main, Zone::Aux, Zone::Cart, Zone::Instant];

/// One zone channel: linear fader + mute.
#[derive(Debug, Clone, Copy)]
pub struct ChannelStrip {
    fader_lin: f64,
    mute: bool,
}

impl Default for ChannelStrip {
    fn default() -> Self {
        Self { fader_lin: 1.0, mute: false }
    }
}

impl ChannelStrip {
    /// 0 dB strip.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the fader in dB (clamped to about [-80, +12] dB).
    pub fn set_fader_db(&mut self, db: f64) {
        self.fader_lin = db_to_lin(db.clamp(-80.0, 12.0));
    }

    /// Set the fader from a linear value (clamped to [0, 4]).
    pub fn set_fader_lin(&mut self, lin: f64) {
        self.fader_lin = clamp_f64(lin, 0.0, 4.0);
    }

    pub fn fader_lin(&self) -> f64 {
        self.fader_lin
    }

    pub fn fader_db(&self) -> f64 {
        lin_to_db(self.fader_lin)
    }

    pub fn set_mute(&mut self, mute: bool) {
        self.mute = mute;
    }

    pub fn is_muted(&self) -> bool {
        self.mute
    }

    /// Effective gain for this strip (mute folds into a 0 gain).
    pub fn gain(&self) -> f64 {
        if self.mute {
            0.0
        } else {
            self.fader_lin
        }
    }
}

/// Mixer state for one render block.
///
/// Zones are summed by the engine; the mixer only holds gain state, so it
/// is `Send + Sync` and can sit behind one `parking_lot::Mutex`.
#[derive(Debug, Default, Clone)]
pub struct Mixer {
    strips: Vec<(Zone, ChannelStrip)>,
    master_lin: f64,
    /// Program duck applied to the main zone while a sweeper/VDF is
    /// playing (`vol_onsweeper`).
    duck_lin: f64,
    /// Optional second duck for voice-track mode (`vol_vt`).
    vt_duck_lin: f64,
}

impl Mixer {
    pub fn new() -> Self {
        Self {
            strips: ALL_ZONES.iter().map(|z| (*z, ChannelStrip::new())).collect(),
            master_lin: 1.0,
            duck_lin: 1.0,
            vt_duck_lin: 1.0,
        }
    }

    fn strip(&self, zone: Zone) -> ChannelStrip {
        self.strips
            .iter()
            .find(|(z, _)| *z == zone)
            .map(|(_, s)| *s)
            .unwrap_or_default()
    }

    fn strip_mut(&mut self, zone: Zone) -> &mut ChannelStrip {
        if !self.strips.iter().any(|(z, _)| *z == zone) {
            self.strips.push((zone, ChannelStrip::new()));
        }
        &mut self.strips.iter_mut().find(|(z, _)| *z == zone).unwrap().1
    }

    /// Zone fader in dB.
    pub fn zone_fader_db(&self, zone: Zone) -> f64 {
        self.strip(zone).fader_db()
    }

    pub fn set_zone_fader_db(&mut self, zone: Zone, db: f64) {
        self.strip_mut(zone).set_fader_db(db);
    }

    pub fn set_zone_muted(&mut self, zone: Zone, mute: bool) {
        self.strip_mut(zone).set_mute(mute);
    }

    pub fn is_zone_muted(&self, zone: Zone) -> bool {
        self.strip(zone).is_muted()
    }

    /// Master output gain (before the `vol_Out` trims inside the FX chain).
    pub fn master_db(&self) -> f64 {
        lin_to_db(self.master_lin)
    }

    pub fn set_master_db(&mut self, db: f64) {
        self.master_lin = db_to_lin(db.clamp(-80.0, 12.0));
    }

    pub fn master_lin(&self) -> f64 {
        self.master_lin
    }

    pub fn set_master_lin(&mut self, lin: f64) {
        self.master_lin = clamp_f64(lin, 0.0, 4.0);
    }

    /// Set the sweeper/VDF duck gain (`vol_onsweeper`, linear).
    pub fn set_duck_lin(&mut self, lin: f64) {
        self.duck_lin = clamp_f64(lin, 0.0, 1.0);
    }

    pub fn duck_lin(&self) -> f64 {
        self.duck_lin
    }

    /// Set the voice-track duck gain (`vol_vt`, linear).
    pub fn set_vt_duck_lin(&mut self, lin: f64) {
        self.vt_duck_lin = clamp_f64(lin, 0.0, 1.0);
    }

    pub fn vt_duck_lin(&self) -> f64 {
        self.vt_duck_lin
    }

    /// Effective gain for a zone including program ducks.
    ///
    /// The ducks for sweeper and voice-track only apply to the music zones
    /// (main + aux); carts and instant inserts play un-ducked.
    pub fn zone_gain(&self, zone: Zone) -> f64 {
        let strip = self.strip(zone);
        let duck = match zone {
            Zone::Main | Zone::Aux => self.duck_lin.min(self.vt_duck_lin),
            Zone::Cart | Zone::Instant => 1.0,
        };
        strip.gain() * duck
    }

    /// Multiply `buf` by the zone gain (interleaved stereo).
    pub fn apply_zone(&self, zone: Zone, buf: &mut [f32]) {
        let g = self.zone_gain(zone) as f32;
        if (g - 1.0).abs() < 1e-9 {
            return;
        }
        for s in buf.iter_mut() {
            *s *= g;
        }
    }

    /// Multiply the summed master buffer by the master gain.
    pub fn apply_master(&self, buf: &mut [f32]) {
        let g = self.master_lin as f32;
        if (g - 1.0).abs() < 1e-9 {
            return;
        }
        for s in buf.iter_mut() {
            *s *= g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::db_to_lin;

    #[test]
    fn strip_gain_and_mute() {
        let mut s = ChannelStrip::new();
        s.set_fader_db(-6.0);
        assert!((s.fader_lin() - db_to_lin(-6.0)).abs() < 1e-9);
        s.set_mute(true);
        assert_eq!(s.gain(), 0.0);
        s.set_mute(false);
        assert!(s.gain() > 0.0);
    }

    #[test]
    fn duck_applies_to_music_zones_only() {
        let mut m = Mixer::new();
        m.set_duck_lin(0.5);
        assert!((m.zone_gain(Zone::Main) - 0.5).abs() < 1e-9);
        assert!((m.zone_gain(Zone::Aux) - 0.5).abs() < 1e-9);
        assert!((m.zone_gain(Zone::Cart) - 1.0).abs() < 1e-9);
        assert!((m.zone_gain(Zone::Instant) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn zone_and_master_gain_stacking() {
        let mut m = Mixer::new();
        m.set_zone_fader_db(Zone::Main, -6.0206);
        m.set_master_db(-6.0206);
        let mut buf = vec![1.0f32; 4];
        m.apply_zone(Zone::Main, &mut buf);
        // -6 dB half-ish
        assert!((buf[0] - 0.5).abs() < 0.02);
        m.apply_master(&mut buf);
        assert!((buf[0] - 0.25).abs() < 0.02);
    }

    #[test]
    fn muted_zone_silences_buffer() {
        let mut m = Mixer::new();
        m.set_zone_muted(Zone::Cart, true);
        let mut buf = vec![1.0f32; 2];
        m.apply_zone(Zone::Cart, &mut buf);
        assert_eq!(buf[0], 0.0);
    }
}
