mod export;
mod parse;

use std::{io, path::Path};

use serde::{Deserialize, Serialize};
use storycut_core::SubtitleFormat;
use thiserror::Error;

pub use export::{assess_lossiness, export_subtitle};
pub use parse::{parse_subtitle, parse_subtitle_content};

/// Stable error categories consumed by CLI/MCP callers.
#[derive(Debug, Error)]
pub enum SubtitleError {
    #[error("could not read subtitle file: {0}")]
    Io(#[from] io::Error),
    #[error("unsupported subtitle format for path: {0}")]
    UnsupportedFormat(String),
    #[error(
        "unsupported subtitle encoding; save the file as UTF-8 or UTF-16 with a byte-order mark"
    )]
    UnsupportedEncoding,
    #[error("subtitle document is too large (maximum 65536 Unicode scalar values)")]
    DocumentTooLarge,
    #[error("invalid subtitle document at line {line}: {detail}")]
    Parse { line: usize, detail: String },
    #[error("invalid subtitle metadata: {0}")]
    InvalidMetadata(String),
    #[error("subtitle cue time exceeds the 24-hour project limit")]
    TimeOutOfRange,
    #[error("subtitle cue time cannot be represented exactly in the target format")]
    InvalidTimePrecision,
    #[error("UNSUPPORTED_FEATURE: {0}")]
    UnsupportedFeature(String),
}

impl SubtitleError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Io(_) => "IO_ERROR",
            Self::UnsupportedFormat(_) => "UNSUPPORTED_FORMAT",
            Self::UnsupportedEncoding => "UNSUPPORTED_ENCODING",
            Self::DocumentTooLarge => "DOCUMENT_TOO_LARGE",
            Self::Parse { .. } => "INVALID_SUBTITLE",
            Self::InvalidMetadata(_) => "INVALID_METADATA",
            Self::TimeOutOfRange => "TIME_OUT_OF_RANGE",
            Self::InvalidTimePrecision => "INVALID_TIME_PRECISION",
            Self::UnsupportedFeature(_) => "UNSUPPORTED_FEATURE",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossinessItem {
    /// Machine-readable, stable identifier such as `ass_styles` or `vtt_settings`.
    pub code: String,
    /// User-facing explanation of the information that cannot be represented.
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossinessReport {
    pub source_format: SubtitleFormat,
    pub target_format: SubtitleFormat,
    pub items: Vec<LossinessItem>,
}

impl LossinessReport {
    pub fn is_lossy(&self) -> bool {
        !self.items.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubtitleExport {
    pub content: String,
    pub format: SubtitleFormat,
    pub lossiness: LossinessReport,
    /// True when `content` is the unchanged stored `raw_document` string. This does not
    /// promise that the original file's byte encoding is reproduced by a later writer.
    pub exact_source: bool,
}

pub(crate) fn format_from_path(path: &Path) -> Result<SubtitleFormat, SubtitleError> {
    match path.extension().and_then(|value| value.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("srt") => Ok(SubtitleFormat::Srt),
        Some(ext) if ext.eq_ignore_ascii_case("ass") => Ok(SubtitleFormat::Ass),
        Some(ext) if ext.eq_ignore_ascii_case("ssa") => Ok(SubtitleFormat::Ssa),
        Some(ext) if ext.eq_ignore_ascii_case("vtt") => Ok(SubtitleFormat::Vtt),
        _ => Err(SubtitleError::UnsupportedFormat(
            path.extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_owned(),
        )),
    }
}

pub(crate) fn format_name(format: SubtitleFormat) -> &'static str {
    match format {
        SubtitleFormat::Srt => "SRT",
        SubtitleFormat::Ass => "ASS",
        SubtitleFormat::Ssa => "SSA",
        SubtitleFormat::Vtt => "WebVTT",
    }
}
