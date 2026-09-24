use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: &str = "0.2.0-draft";
pub const TIMEBASE: u64 = 705_600_000;
pub const MAX_TICKS: u64 = 60_963_840_000_000;
pub const MAX_REVISION: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema_version: String,
    pub project_id: String,
    pub revision: u64,
    pub name: String,
    pub timebase: u64,
    pub canvas: Canvas,
    pub audio_sample_rate: u32,
    pub notes: Vec<String>,
    pub assets: Vec<Asset>,
    pub tracks: Vec<Track>,
    pub clips: Vec<Clip>,
    pub transitions: Vec<Transition>,
    pub links: Vec<Link>,
    pub subtitles: Vec<Subtitle>,
}

impl Project {
    pub fn empty(
        project_id: impl Into<String>,
        name: impl Into<String>,
        canvas: Canvas,
        audio_sample_rate: u32,
    ) -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_owned(),
            project_id: project_id.into(),
            revision: 0,
            name: name.into(),
            timebase: TIMEBASE,
            canvas,
            audio_sample_rate,
            notes: Vec::new(),
            assets: Vec::new(),
            tracks: Vec::new(),
            clips: Vec::new(),
            transitions: Vec::new(),
            links: Vec::new(),
            subtitles: Vec::new(),
        }
    }

    pub fn frame_ticks(&self) -> Option<u64> {
        let numerator = u128::from(TIMEBASE) * u128::from(self.canvas.fps.den);
        let denominator = u128::from(self.canvas.fps.num);
        if denominator == 0 || numerator % denominator != 0 {
            return None;
        }
        u64::try_from(numerator / denominator).ok()
    }

    pub fn duration_ticks(&self) -> u64 {
        self.clips
            .iter()
            .filter_map(Clip::end_tick)
            .chain(self.subtitles.iter().flat_map(|subtitle| {
                subtitle
                    .cues
                    .iter()
                    .filter_map(move |cue| subtitle.offset_tick.checked_add(cue.end_tick))
            }))
            .max()
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub background: String,
    pub color_mode: String,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: Rational { num: 30, den: 1 },
            background: "#000000".to_owned(),
            color_mode: "sdr_bt709".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rational {
    pub num: u32,
    pub den: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    pub path: String,
    pub probe_status: ProbeStatus,
    pub duration_ticks: Option<u64>,
    pub sha256: Option<String>,
    pub streams: Vec<Stream>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Video,
    Image,
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Unprobed,
    Probed,
    Offline,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub index: u64,
    pub kind: StreamKind,
    pub time_base: Rational,
    pub sample_rate: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Video,
    Audio,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: String,
    pub name: String,
    pub kind: TrackKind,
    pub locked: bool,
    pub enabled: bool,
    pub muted: bool,
    pub solo: bool,
    pub gain_db: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Video,
    Image,
    Audio,
    Subtitle,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Clip {
    Image(ImageClip),
    Video(VideoClip),
    Audio(AudioClip),
}

impl Clip {
    pub fn id(&self) -> &str {
        match self {
            Self::Image(clip) => &clip.id,
            Self::Video(clip) => &clip.id,
            Self::Audio(clip) => &clip.id,
        }
    }

    pub fn track_id(&self) -> &str {
        match self {
            Self::Image(clip) => &clip.track_id,
            Self::Video(clip) => &clip.track_id,
            Self::Audio(clip) => &clip.track_id,
        }
    }

    pub fn asset_id(&self) -> &str {
        match self {
            Self::Image(clip) => &clip.asset_id,
            Self::Video(clip) => &clip.asset_id,
            Self::Audio(clip) => &clip.asset_id,
        }
    }

    pub fn start_tick(&self) -> u64 {
        match self {
            Self::Image(clip) => clip.start_tick,
            Self::Video(clip) => clip.start_tick,
            Self::Audio(clip) => clip.start_tick,
        }
    }

    pub fn duration_ticks(&self) -> u64 {
        match self {
            Self::Image(clip) => clip.duration_ticks,
            Self::Video(clip) => clip.duration_ticks,
            Self::Audio(clip) => clip.duration_ticks,
        }
    }

    pub fn source_in_tick(&self) -> u64 {
        match self {
            Self::Image(clip) => clip.source_in_tick,
            Self::Video(clip) => clip.source_in_tick,
            Self::Audio(clip) => clip.source_in_tick,
        }
    }

    pub fn end_tick(&self) -> Option<u64> {
        self.start_tick().checked_add(self.duration_ticks())
    }

    pub fn is_visual(&self) -> bool {
        !matches!(self, Self::Audio(_))
    }

    pub fn motion(&self) -> Option<&Motion> {
        match self {
            Self::Image(clip) => Some(&clip.motion),
            Self::Video(clip) => Some(&clip.motion),
            Self::Audio(_) => None,
        }
    }

    pub fn motion_mut(&mut self) -> Option<&mut Motion> {
        match self {
            Self::Image(clip) => Some(&mut clip.motion),
            Self::Video(clip) => Some(&mut clip.motion),
            Self::Audio(_) => None,
        }
    }

    pub fn audio(&self) -> Option<&AudioSettings> {
        match self {
            Self::Audio(clip) => Some(&clip.audio),
            _ => None,
        }
    }

    pub fn audio_mut(&mut self) -> Option<&mut AudioSettings> {
        match self {
            Self::Audio(clip) => Some(&mut clip.audio),
            _ => None,
        }
    }

    pub fn set_start_tick(&mut self, value: u64) {
        match self {
            Self::Image(clip) => clip.start_tick = value,
            Self::Video(clip) => clip.start_tick = value,
            Self::Audio(clip) => clip.start_tick = value,
        }
    }

    pub fn set_duration_ticks(&mut self, value: u64) {
        match self {
            Self::Image(clip) => clip.duration_ticks = value,
            Self::Video(clip) => clip.duration_ticks = value,
            Self::Audio(clip) => clip.duration_ticks = value,
        }
    }

    pub fn set_source_in_tick(&mut self, value: u64) {
        match self {
            Self::Image(clip) => clip.source_in_tick = value,
            Self::Video(clip) => clip.source_in_tick = value,
            Self::Audio(clip) => clip.source_in_tick = value,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageClip {
    pub id: String,
    pub track_id: String,
    pub asset_id: String,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub source_in_tick: u64,
    pub motion: Motion,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoClip {
    pub id: String,
    pub track_id: String,
    pub asset_id: String,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub source_in_tick: u64,
    pub stream_index: u64,
    pub motion: Motion,
    pub audio_policy: VideoAudioPolicy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoAudioPolicy {
    Muted,
    SeparateLinked,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioClip {
    pub id: String,
    pub track_id: String,
    pub asset_id: String,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub source_in_tick: u64,
    pub stream_index: u64,
    pub audio: AudioSettings,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Motion {
    pub domain_duration_ticks: u64,
    pub sample_offset_tick: u64,
    pub fit: FitMode,
    pub avoid_exposed_edges: bool,
    pub anchor: Anchor,
    pub interpolation: Interpolation,
    pub keyframes: Vec<Keyframe>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Contain,
    Cover,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Linear,
    Smoothstep,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keyframe {
    pub tick: u64,
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    pub opacity: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AudioSettings {
    pub domain_duration_ticks: u64,
    pub sample_offset_tick: u64,
    pub gain_db: f64,
    pub pan: f64,
    pub muted: bool,
    pub fade_in_ticks: u64,
    pub fade_out_ticks: u64,
    pub fade_curve: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    pub track_id: String,
    pub from_clip_id: String,
    pub to_clip_id: String,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub kind: TransitionKind,
    pub curve: String,
    pub audio_policy: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    CrossDissolve,
    FadeBlack,
    FadeWhite,
    SlideLeft,
    SlideRight,
    WipeLeft,
    WipeRight,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub id: String,
    pub kind: String,
    pub clip_ids: [String; 2],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Subtitle {
    pub id: String,
    pub track_id: String,
    pub format: SubtitleFormat,
    pub offset_tick: u64,
    pub path: String,
    pub raw_document: String,
    pub cues: Vec<Cue>,
    pub preserve_unknown_syntax: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubtitleFormat {
    Srt,
    Ass,
    Ssa,
    Vtt,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cue {
    pub id: String,
    pub start_tick: u64,
    pub end_tick: u64,
    pub text: String,
    pub source_payload: String,
}

impl Track {
    pub fn new(id: impl Into<String>, name: impl Into<String>, kind: TrackKind) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            kind,
            locked: false,
            enabled: true,
            muted: false,
            solo: false,
            gain_db: 0.0,
        }
    }
}
