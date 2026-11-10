//! Núcleo de Snipsy: modelo del proyecto y utilidades sin dependencias pesadas.

pub mod actions;
pub mod brand;
pub mod compose;
pub mod ducking;
pub mod effects;
pub mod follow;
pub mod history;
pub mod intent;
pub mod library;
pub mod project;
pub mod reframe;
pub mod silence;
pub mod timecode;
pub mod timeline;

pub use brand::{BrandKit, Corner, TextPreset};
pub use ducking::DuckSettings;
pub use silence::SilenceSettings;
pub use compose::{TextStyle, Transform, WordAnimation, WordMode, WordState};
pub use effects::{Animation, ColorGrade, Curve, Ease, Keyframe, Prop, Span};
pub use history::History;
pub use timeline::{Asset, AssetId, Clip, ClipId, Edge, Frame, Timeline, Track, TrackKind, TrackRef};
