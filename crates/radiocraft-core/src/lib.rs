//! RadioCraft domain model: track types, SongData, cues, playlist, scheduler types.

pub mod booking;
pub mod events;
pub mod playlist;
pub mod rotation;
pub mod scheduler;
pub mod template;

pub use booking::{plan_booking, BookingOutcome, BookingRequest, HistoryEntry, LimitAction};
pub use events::{DueReason, Event, EventAction, EventTrigger};
pub use playlist::{PlaylistBuilder, PlaylistEntry, PlaylistSource};
pub use rotation::{
    CategoryType, DayMask, Rotation, RotationCriteria, SelectionMode,
    select_rotations, time_in_window,
};
pub use scheduler::{CandidatePool, Pick, Scheduler, SchedulerConfig, SchedulerDecision};
pub use template::{Context as TemplateContext, TemplateEngine};
