use std::{fs, io::Read, path::Path};

use storycut_core::{Cue, MAX_TICKS, Subtitle, SubtitleFormat, TIMEBASE};
use uuid::Uuid;

use crate::{SubtitleError, format_from_path};

const MAX_DOCUMENT_CHARS: usize = 65_536;
const MAX_DOCUMENT_BYTES: usize = 262_148;
const MILLISECOND_TICKS: u64 = TIMEBASE / 1_000;
const CENTISECOND_TICKS: u64 = TIMEBASE / 100;

#[derive(Clone, Debug)]
pub(crate) struct ParsedCue {
    pub start_tick: u64,
    pub end_tick: u64,
    pub text: String,
    pub source_payload: String,
    pub line: usize,
}

/// Read and parse an SRT, ASS, SSA, or WebVTT file from disk.
///
/// Cue ticks stay relative to the subtitle document. `offset_tick` is the
/// document's placement on the project timeline, as required by storycut-core.
pub fn parse_subtitle(
    path: impl AsRef<Path>,
    track_id: impl Into<String>,
    offset_tick: u64,
) -> Result<Subtitle, SubtitleError> {
    let path = path.as_ref();
    let format = format_from_path(path)?;
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_DOCUMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err(SubtitleError::DocumentTooLarge);
    }
    let raw_document = decode_document(&bytes)?;
    parse_subtitle_content(path, track_id, offset_tick, raw_document, format)
}

/// Parse already-read subtitle text. This is also useful for import adapters
/// that have performed their own workspace authorization before opening files.
pub fn parse_subtitle_content(
    path: impl AsRef<Path>,
    track_id: impl Into<String>,
    offset_tick: u64,
    raw_document: impl Into<String>,
    format: SubtitleFormat,
) -> Result<Subtitle, SubtitleError> {
    let path = path.as_ref();
    let track_id = track_id.into();
    let raw_document = raw_document.into();
    if !valid_id(&track_id) {
        return Err(SubtitleError::InvalidMetadata(
            "track_id must match the StoryCut identifier pattern".into(),
        ));
    }
    if path.as_os_str().to_string_lossy().chars().count() > 32_767 {
        return Err(SubtitleError::InvalidMetadata(
            "subtitle path exceeds 32767 characters".into(),
        ));
    }
    if offset_tick > MAX_TICKS {
        return Err(SubtitleError::TimeOutOfRange);
    }
    if raw_document.chars().count() > MAX_DOCUMENT_CHARS {
        return Err(SubtitleError::DocumentTooLarge);
    }

    let parsed = parse_cues(&raw_document, format)?;
    if parsed.len() > 100_000 {
        return Err(SubtitleError::InvalidMetadata(
            "subtitle contains more than 100000 cues".into(),
        ));
    }

    let mut cues = Vec::with_capacity(parsed.len());
    for cue in parsed {
        if cue.start_tick >= cue.end_tick {
            return Err(SubtitleError::Parse {
                line: cue.line,
                detail: "cue end must be later than cue start".into(),
            });
        }
        if offset_tick
            .checked_add(cue.end_tick)
            .is_none_or(|end| end > MAX_TICKS)
        {
            return Err(SubtitleError::TimeOutOfRange);
        }
        cues.push(Cue {
            id: fresh_id("cue"),
            start_tick: cue.start_tick,
            end_tick: cue.end_tick,
            text: cue.text,
            source_payload: cue.source_payload,
        });
    }

    Ok(Subtitle {
        id: fresh_id("subtitle"),
        track_id,
        format,
        offset_tick,
        path: path.to_string_lossy().into_owned(),
        raw_document,
        cues,
        preserve_unknown_syntax: true,
    })
}

pub(crate) fn parse_cues(
    raw_document: &str,
    format: SubtitleFormat,
) -> Result<Vec<ParsedCue>, SubtitleError> {
    match format {
        SubtitleFormat::Srt => parse_srt(raw_document),
        SubtitleFormat::Vtt => parse_vtt(raw_document),
        SubtitleFormat::Ass | SubtitleFormat::Ssa => parse_ass(raw_document),
    }
}

fn parse_srt(source: &str) -> Result<Vec<ParsedCue>, SubtitleError> {
    let mut cues = Vec::new();
    for (payload, line) in split_blocks(source) {
        let lines = content_lines(&payload);
        let timing_index = lines.iter().position(|value| value.contains("-->"));
        let Some(timing_index) = timing_index else {
            return Err(parse_error(line, "SRT block has no timing line"));
        };
        if timing_index > 1 {
            return Err(parse_error(
                line,
                "SRT timing line must follow the optional cue number",
            ));
        }
        if timing_index == 1
            && !trim_bom(lines[0])
                .trim()
                .chars()
                .all(|ch| ch.is_ascii_digit())
        {
            return Err(parse_error(line, "invalid SRT cue number"));
        }
        let (start_tick, end_tick) = parse_arrow_timing(
            lines[timing_index],
            parse_srt_timestamp,
            line + timing_index,
        )?;
        let text = lines[timing_index + 1..].join("\n");
        cues.push(ParsedCue {
            start_tick,
            end_tick,
            text,
            source_payload: payload,
            line,
        });
    }
    Ok(cues)
}

fn parse_vtt(source: &str) -> Result<Vec<ParsedCue>, SubtitleError> {
    let blocks = split_blocks(source);
    if blocks.is_empty() {
        return Err(parse_error(1, "WebVTT document is empty"));
    }
    let header_lines = content_lines(&blocks[0].0);
    if !header_lines.first().is_some_and(|line| {
        let header = trim_bom(line).trim();
        header == "WEBVTT" || header.starts_with("WEBVTT ") || header.starts_with("WEBVTT\t")
    }) {
        return Err(parse_error(
            blocks[0].1,
            "WebVTT header must begin with WEBVTT",
        ));
    }
    if header_lines.iter().any(|line| line.contains("-->")) {
        return Err(parse_error(
            blocks[0].1,
            "WebVTT header must be separated from cues by a blank line",
        ));
    }

    let mut cues = Vec::new();
    for (payload, line) in blocks.into_iter().skip(1) {
        let lines = content_lines(&payload);
        let first = lines
            .first()
            .map(|value| trim_bom(value).trim())
            .unwrap_or_default();
        if is_vtt_special_block(first) {
            continue;
        }
        let Some(timing_index) = lines.iter().position(|value| value.contains("-->")) else {
            // Preserve otherwise unknown WebVTT blocks in raw_document. They are not cues.
            continue;
        };
        if timing_index > 1 {
            return Err(parse_error(
                line,
                "WebVTT cue timing must follow the optional cue identifier",
            ));
        }
        let (start_tick, end_tick) = parse_arrow_timing(
            lines[timing_index],
            parse_vtt_timestamp,
            line + timing_index,
        )?;
        let text = lines[timing_index + 1..].join("\n");
        cues.push(ParsedCue {
            start_tick,
            end_tick,
            text,
            source_payload: payload,
            line,
        });
    }
    Ok(cues)
}

fn is_vtt_special_block(first_line: &str) -> bool {
    ["NOTE", "STYLE", "REGION"].into_iter().any(|keyword| {
        first_line.eq_ignore_ascii_case(keyword)
            || first_line
                .get(..keyword.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(keyword))
                && first_line
                    .get(keyword.len()..)
                    .and_then(|suffix| suffix.chars().next())
                    .is_some_and(|ch| matches!(ch, ' ' | '\t'))
    })
}

fn parse_ass(source: &str) -> Result<Vec<ParsedCue>, SubtitleError> {
    let mut cues = Vec::new();
    let mut section = String::new();
    let mut event_fields: Option<Vec<String>> = None;
    for (index, raw_line) in source.split_inclusive('\n').enumerate() {
        let line_number = index + 1;
        let line = line_without_ending(raw_line);
        let logical_line = if line_number == 1 {
            trim_bom(line)
        } else {
            line
        };
        let trimmed = logical_line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].to_ascii_lowercase();
            if section == "events" {
                event_fields = None;
            }
            continue;
        }
        if section != "events" {
            continue;
        }
        let Some((key, value)) = logical_line.split_once(':') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("format") {
            event_fields = Some(
                value
                    .split(',')
                    .map(|field| field.trim().to_owned())
                    .collect(),
            );
            continue;
        }
        if !key.trim().eq_ignore_ascii_case("dialogue") {
            // ASS comments and future event records remain untouched in raw_document.
            continue;
        }
        let Some(fields) = event_fields.as_ref() else {
            return Err(parse_error(
                line_number,
                "ASS/SSA Dialogue requires an Events Format declaration",
            ));
        };
        let start_index = field_index(fields, "Start");
        let end_index = field_index(fields, "End");
        let text_index = field_index(fields, "Text");
        let (Some(start_index), Some(end_index), Some(text_index)) =
            (start_index, end_index, text_index)
        else {
            return Err(SubtitleError::UnsupportedFeature(
                "ASS/SSA event format does not expose Start, End, and Text fields".into(),
            ));
        };
        let values = split_ass_fields(value, fields.len()).ok_or_else(|| {
            parse_error(
                line_number,
                "ASS/SSA Dialogue has fewer fields than declared",
            )
        })?;
        let start_tick = parse_ass_timestamp(values[start_index].trim())
            .ok_or_else(|| parse_error(line_number, "invalid ASS/SSA start time"))?;
        let end_tick = parse_ass_timestamp(values[end_index].trim())
            .ok_or_else(|| parse_error(line_number, "invalid ASS/SSA end time"))?;
        cues.push(ParsedCue {
            start_tick,
            end_tick,
            text: values[text_index].to_owned(),
            source_payload: raw_line.to_owned(),
            line: line_number,
        });
    }
    Ok(cues)
}

pub(crate) fn parse_srt_timestamp(value: &str) -> Option<u64> {
    parse_clock(value, true, true)
}

pub(crate) fn parse_vtt_timestamp(value: &str) -> Option<u64> {
    parse_clock(value, false, true)
}

pub(crate) fn parse_ass_timestamp(value: &str) -> Option<u64> {
    let mut parts = value.split(':');
    let hours = parts.next()?.parse::<u64>().ok()?;
    let minutes = parts.next()?.parse::<u64>().ok()?;
    let seconds = parts.next()?;
    if parts.next().is_some() || minutes >= 60 {
        return None;
    }
    let (whole, fraction) = seconds.split_once('.')?;
    let seconds = whole.parse::<u64>().ok()?;
    if seconds >= 60
        || fraction.is_empty()
        || fraction.len() > 2
        || !fraction.chars().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    let centiseconds = if fraction.len() == 1 {
        fraction.parse::<u64>().ok()? * 10
    } else {
        fraction.parse::<u64>().ok()?
    };
    let total_cs = hours
        .checked_mul(3_600 * 100)?
        .checked_add(minutes.checked_mul(60 * 100)?)?
        .checked_add(seconds.checked_mul(100)?)?
        .checked_add(centiseconds)?;
    let ticks = total_cs.checked_mul(CENTISECOND_TICKS)?;
    (ticks <= MAX_TICKS).then_some(ticks)
}

fn parse_clock(value: &str, require_hours: bool, allow_comma: bool) -> Option<u64> {
    let (clock, fraction) = value
        .split_once('.')
        .or_else(|| allow_comma.then(|| value.split_once(',')).flatten())?;
    if fraction.is_empty() || fraction.len() > 3 || !fraction.chars().all(|ch| ch.is_ascii_digit())
    {
        return None;
    }
    let components: Vec<_> = clock.split(':').collect();
    let (hours, minutes, seconds) = match components.as_slice() {
        [hours, minutes, seconds] => (
            hours.parse::<u64>().ok()?,
            minutes.parse::<u64>().ok()?,
            seconds.parse::<u64>().ok()?,
        ),
        [minutes, seconds] if !require_hours => (
            0,
            minutes.parse::<u64>().ok()?,
            seconds.parse::<u64>().ok()?,
        ),
        _ => return None,
    };
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    let mut millis = fraction.parse::<u64>().ok()?;
    for _ in fraction.len()..3 {
        millis = millis.checked_mul(10)?;
    }
    let total_millis = hours
        .checked_mul(3_600_000)?
        .checked_add(minutes.checked_mul(60_000)?)?
        .checked_add(seconds.checked_mul(1_000)?)?
        .checked_add(millis)?;
    let ticks = total_millis.checked_mul(MILLISECOND_TICKS)?;
    (ticks <= MAX_TICKS).then_some(ticks)
}

fn parse_arrow_timing(
    line: &str,
    parse_timestamp: fn(&str) -> Option<u64>,
    line_number: usize,
) -> Result<(u64, u64), SubtitleError> {
    let Some((left, right)) = line.split_once("-->") else {
        return Err(parse_error(line_number, "timing line has no --> separator"));
    };
    let start = parse_timestamp(left.trim())
        .ok_or_else(|| parse_error(line_number, "invalid cue start time"))?;
    let end_token = right.split_whitespace().next().unwrap_or_default();
    let end = parse_timestamp(end_token)
        .ok_or_else(|| parse_error(line_number, "invalid cue end time"))?;
    Ok((start, end))
}

pub(crate) fn split_blocks(source: &str) -> Vec<(String, usize)> {
    let mut blocks = Vec::new();
    let mut current = String::new();
    let mut current_line = 1;
    let mut line_number = 1;
    for raw_line in source.split_inclusive('\n') {
        let content = line_without_ending(raw_line);
        if content.trim().is_empty() {
            if !current.is_empty() {
                blocks.push((std::mem::take(&mut current), current_line));
            }
        } else {
            if current.is_empty() {
                current_line = line_number;
            }
            current.push_str(raw_line);
        }
        line_number += 1;
    }
    if !current.is_empty() {
        blocks.push((current, current_line));
    }
    blocks
}

pub(crate) fn content_lines(payload: &str) -> Vec<&str> {
    payload.lines().map(trim_bom).collect()
}

pub(crate) fn line_without_ending(line: &str) -> &str {
    line.strip_suffix('\n')
        .unwrap_or(line)
        .strip_suffix('\r')
        .unwrap_or_else(|| line.strip_suffix('\n').unwrap_or(line))
}

pub(crate) fn trim_bom(value: &str) -> &str {
    value.strip_prefix('\u{feff}').unwrap_or(value)
}

fn split_ass_fields(value: &str, count: usize) -> Option<Vec<&str>> {
    if count == 0 {
        return None;
    }
    let mut fields = Vec::with_capacity(count);
    let mut remaining = value;
    for _ in 1..count {
        let (field, rest) = remaining.split_once(',')?;
        fields.push(field);
        remaining = rest;
    }
    fields.push(remaining);
    Some(fields)
}

pub(crate) fn field_index(fields: &[String], target: &str) -> Option<usize> {
    fields
        .iter()
        .position(|value| value.trim().eq_ignore_ascii_case(target))
}

fn decode_document(bytes: &[u8]) -> Result<String, SubtitleError> {
    if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
        return std::str::from_utf8(bytes)
            .map(str::to_owned)
            .map_err(|_| SubtitleError::UnsupportedEncoding);
    }
    if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        if bytes.len() % 2 != 0 {
            return Err(SubtitleError::UnsupportedEncoding);
        }
        let little_endian = bytes[0] == 0xff;
        let units = bytes[2..]
            .chunks_exact(2)
            .map(|pair| {
                if little_endian {
                    u16::from_le_bytes([pair[0], pair[1]])
                } else {
                    u16::from_be_bytes([pair[0], pair[1]])
                }
            })
            .collect::<Vec<_>>();
        let mut decoded =
            String::from_utf16(&units).map_err(|_| SubtitleError::UnsupportedEncoding)?;
        decoded.insert(0, '\u{feff}');
        return Ok(decoded);
    }
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| SubtitleError::UnsupportedEncoding)
}

fn fresh_id(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::new_v4().simple())
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'))
}

fn parse_error(line: usize, detail: impl Into<String>) -> SubtitleError {
    SubtitleError::Parse {
        line,
        detail: detail.into(),
    }
}
