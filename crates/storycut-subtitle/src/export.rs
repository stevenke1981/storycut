use storycut_core::MAX_TICKS;
use storycut_core::{Cue, Subtitle, SubtitleFormat, TIMEBASE};

use crate::{
    LossinessItem, LossinessReport, SubtitleError, SubtitleExport, format_name,
    parse::{ParsedCue, field_index, parse_cues},
};

const MILLISECOND_TICKS: u64 = TIMEBASE / 1_000;
const CENTISECOND_TICKS: u64 = TIMEBASE / 100;

/// Describe source information that cannot be represented in the requested format.
/// The report is deliberately explicit: callers can show it before writing an export.
pub fn assess_lossiness(subtitle: &Subtitle, target: SubtitleFormat) -> LossinessReport {
    let mut items = Vec::new();
    if subtitle.format != target {
        match subtitle.format {
            SubtitleFormat::Srt => add_loss(
                &mut items,
                "srt_cue_numbers",
                "SRT cue sequence numbers are regenerated for the target format.",
            ),
            SubtitleFormat::Ass | SubtitleFormat::Ssa => {
                add_loss(
                    &mut items,
                    "ass_script_and_styles",
                    "ASS/SSA script properties and style definitions are not represented by the target format.",
                );
                add_loss(
                    &mut items,
                    "ass_effects_and_override_tags",
                    "ASS/SSA positioning, animation, karaoke, and inline override tags may be flattened.",
                );
                if subtitle.raw_document.contains("Comment:") {
                    add_loss(
                        &mut items,
                        "ass_comment_events",
                        "ASS/SSA comment events are omitted from the converted subtitle file.",
                    );
                }
            }
            SubtitleFormat::Vtt => {
                add_loss(
                    &mut items,
                    "vtt_document_blocks",
                    "WebVTT NOTE, STYLE, REGION, and other document-level blocks are omitted.",
                );
                let lower_document = subtitle.raw_document.to_ascii_lowercase();
                if lower_document.contains("position:")
                    || lower_document.contains("line:")
                    || lower_document.contains("vertical:")
                {
                    add_loss(
                        &mut items,
                        "vtt_cue_settings",
                        "WebVTT cue positioning settings are not represented by the target format.",
                    );
                }
                if subtitle.raw_document.contains('<') {
                    add_loss(
                        &mut items,
                        "vtt_text_markup",
                        "WebVTT text markup and CSS classes may be removed or flattened.",
                    );
                }
                if target != SubtitleFormat::Vtt && contains_vtt_cue_id(subtitle) {
                    add_loss(
                        &mut items,
                        "vtt_cue_ids",
                        "WebVTT cue identifiers are not represented by the target format.",
                    );
                }
            }
        }
        add_loss(
            &mut items,
            "source_document_syntax",
            "Unknown source sections and syntax remain in the project document but are not copied to the converted file.",
        );
        if matches!(target, SubtitleFormat::Ass | SubtitleFormat::Ssa)
            && !matches!(subtitle.format, SubtitleFormat::Ass | SubtitleFormat::Ssa)
        {
            add_loss(
                &mut items,
                "generated_default_style",
                "The target ASS/SSA document uses a generated default style; source display styling is not recreated.",
            );
        }
    }
    LossinessReport {
        source_format: subtitle.format,
        target_format: target,
        items,
    }
}

/// Export a subtitle document. Same-format, unedited documents retain their original
/// syntax exactly. Cross-format exports include a lossiness report with the output.
pub fn export_subtitle(
    subtitle: &Subtitle,
    target: SubtitleFormat,
) -> Result<SubtitleExport, SubtitleError> {
    if !subtitle.preserve_unknown_syntax {
        return Err(SubtitleError::UnsupportedFeature(
            "subtitle document does not guarantee preservation of unknown source syntax".into(),
        ));
    }
    let lossiness = assess_lossiness(subtitle, target);
    if target == subtitle.format {
        let original = parse_cues(&subtitle.raw_document, subtitle.format)?;
        if original.len() != subtitle.cues.len() {
            return Err(SubtitleError::UnsupportedFeature(
                "same-format export cannot safely add or remove cues while preserving unknown syntax".into(),
            ));
        }
        let unchanged = subtitle.offset_tick == 0
            && original
                .iter()
                .zip(&subtitle.cues)
                .all(|(source, cue)| cue_matches(cue, source));
        if unchanged {
            return Ok(SubtitleExport {
                content: subtitle.raw_document.clone(),
                format: target,
                lossiness,
                exact_source: true,
            });
        }
        let content = update_same_format(subtitle, &original)?;
        return Ok(SubtitleExport {
            content,
            format: target,
            lossiness,
            exact_source: false,
        });
    }

    let mut shifted = Vec::with_capacity(subtitle.cues.len());
    for cue in &subtitle.cues {
        let start_tick = cue
            .start_tick
            .checked_add(subtitle.offset_tick)
            .ok_or(SubtitleError::TimeOutOfRange)?;
        let end_tick = cue
            .end_tick
            .checked_add(subtitle.offset_tick)
            .ok_or(SubtitleError::TimeOutOfRange)?;
        shifted.push((cue, start_tick, end_tick));
    }
    for (_, start_tick, end_tick) in &shifted {
        validate_export_range(*start_tick, *end_tick)?;
    }
    let content = match target {
        SubtitleFormat::Srt => export_srt(&shifted, subtitle.format)?,
        SubtitleFormat::Vtt => export_vtt(&shifted, subtitle.format)?,
        SubtitleFormat::Ass => export_ass(&shifted, false, subtitle.format)?,
        SubtitleFormat::Ssa => export_ass(&shifted, true, subtitle.format)?,
    };
    Ok(SubtitleExport {
        content,
        format: target,
        lossiness,
        exact_source: false,
    })
}

fn update_same_format(
    subtitle: &Subtitle,
    original: &[ParsedCue],
) -> Result<String, SubtitleError> {
    let mut replacements = Vec::with_capacity(subtitle.cues.len());
    for (cue, source) in subtitle.cues.iter().zip(original) {
        if cue.source_payload != source.source_payload {
            return Err(SubtitleError::UnsupportedFeature(format!(
                "cue {} was reordered or its raw source payload was changed; unknown syntax cannot be associated safely",
                cue.id
            )));
        }
        let shifted_start = cue
            .start_tick
            .checked_add(subtitle.offset_tick)
            .ok_or(SubtitleError::TimeOutOfRange)?;
        let shifted_end = cue
            .end_tick
            .checked_add(subtitle.offset_tick)
            .ok_or(SubtitleError::TimeOutOfRange)?;
        validate_export_range(shifted_start, shifted_end)?;
        if cue_matches_shifted(cue, source, shifted_start, shifted_end) {
            replacements.push((source.source_payload.clone(), source.source_payload.clone()));
            continue;
        }
        let replacement = match subtitle.format {
            SubtitleFormat::Srt => update_srt_payload(source, cue, shifted_start, shifted_end)?,
            SubtitleFormat::Vtt => update_vtt_payload(source, cue, shifted_start, shifted_end)?,
            SubtitleFormat::Ass | SubtitleFormat::Ssa => {
                update_ass_payload(subtitle, source, cue, shifted_start, shifted_end)?
            }
        };
        replacements.push((source.source_payload.clone(), replacement));
    }
    replace_payloads(&subtitle.raw_document, &replacements)
}

fn cue_matches(cue: &Cue, source: &ParsedCue) -> bool {
    cue.start_tick == source.start_tick
        && cue.end_tick == source.end_tick
        && cue.text == source.text
        && cue.source_payload == source.source_payload
}

fn cue_matches_shifted(cue: &Cue, source: &ParsedCue, start: u64, end: u64) -> bool {
    start == source.start_tick
        && end == source.end_tick
        && cue.text == source.text
        && cue.source_payload == source.source_payload
}

fn update_srt_payload(
    source: &ParsedCue,
    cue: &Cue,
    start_tick: u64,
    end_tick: u64,
) -> Result<String, SubtitleError> {
    let mut lines = source
        .source_payload
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let timing_index = lines
        .iter()
        .position(|line| line.contains("-->"))
        .ok_or_else(|| unsupported("SRT cue timing line was not preserved"))?;
    lines[timing_index] = replace_arrow_times(
        &lines[timing_index],
        format_srt_time(start_tick)?,
        format_srt_time(end_tick)?,
    )?;
    if cue.text != source.text {
        reject_markup_edit(&source.text, SubtitleFormat::Srt)?;
        let replacement = split_caption_text(&cue.text)?;
        lines.truncate(timing_index + 1);
        lines.extend(replacement);
    }
    Ok(rebuild_lines(
        &lines,
        line_ending(&source.source_payload),
        has_line_ending(&source.source_payload),
    ))
}

fn update_vtt_payload(
    source: &ParsedCue,
    cue: &Cue,
    start_tick: u64,
    end_tick: u64,
) -> Result<String, SubtitleError> {
    let mut lines = source
        .source_payload
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let timing_index = lines
        .iter()
        .position(|line| line.contains("-->"))
        .ok_or_else(|| unsupported("WebVTT cue timing line was not preserved"))?;
    lines[timing_index] = replace_arrow_times(
        &lines[timing_index],
        format_vtt_time(start_tick)?,
        format_vtt_time(end_tick)?,
    )?;
    if cue.text != source.text {
        reject_markup_edit(&source.text, SubtitleFormat::Vtt)?;
        let replacement = split_caption_text(&cue.text)?;
        lines.truncate(timing_index + 1);
        lines.extend(replacement);
    }
    Ok(rebuild_lines(
        &lines,
        line_ending(&source.source_payload),
        has_line_ending(&source.source_payload),
    ))
}

fn update_ass_payload(
    subtitle: &Subtitle,
    source: &ParsedCue,
    cue: &Cue,
    start_tick: u64,
    end_tick: u64,
) -> Result<String, SubtitleError> {
    let fields = ass_event_fields(&subtitle.raw_document)?;
    let start_index = field_index(&fields, "Start")
        .ok_or_else(|| unsupported("ASS/SSA event format has no Start field"))?;
    let end_index = field_index(&fields, "End")
        .ok_or_else(|| unsupported("ASS/SSA event format has no End field"))?;
    let text_index = field_index(&fields, "Text")
        .ok_or_else(|| unsupported("ASS/SSA event format has no Text field"))?;
    let line = source
        .source_payload
        .strip_suffix('\n')
        .unwrap_or(&source.source_payload)
        .strip_suffix('\r')
        .unwrap_or_else(|| {
            source
                .source_payload
                .strip_suffix('\n')
                .unwrap_or(&source.source_payload)
        });
    let (prefix, value) = line
        .split_once(':')
        .ok_or_else(|| unsupported("ASS/SSA Dialogue source line is malformed"))?;
    let mut values = split_event_values(value, fields.len())
        .ok_or_else(|| unsupported("ASS/SSA Dialogue source fields no longer match its Format"))?;
    values[start_index] = replace_field_time(&values[start_index], format_ass_time(start_tick)?)?;
    values[end_index] = replace_field_time(&values[end_index], format_ass_time(end_tick)?)?;
    if cue.text != source.text {
        reject_markup_edit(&source.text, subtitle.format)?;
        values[text_index] = cue.text.clone();
    }
    let body = values.join(",");
    let trailing_newline = source.source_payload.ends_with('\n');
    let crlf = source.source_payload.ends_with("\r\n");
    let ending = if trailing_newline {
        if crlf { "\r\n" } else { "\n" }
    } else {
        ""
    };
    Ok(format!("{prefix}:{body}{ending}"))
}

fn replace_payloads(
    source: &str,
    replacements: &[(String, String)],
) -> Result<String, SubtitleError> {
    let mut spans = Vec::with_capacity(replacements.len());
    let mut search_from = 0;
    for (old, new) in replacements {
        if old == new {
            continue;
        }
        let Some(relative) = source[search_from..].find(old) else {
            return Err(unsupported(
                "cue raw syntax no longer appears in the source document; refusing to discard unknown content",
            ));
        };
        let start = search_from + relative;
        let end = start + old.len();
        spans.push((start, end, new.as_str()));
        search_from = end;
    }
    let mut output = source.to_owned();
    for (start, end, replacement) in spans.into_iter().rev() {
        output.replace_range(start..end, replacement);
    }
    Ok(output)
}

fn replace_arrow_times(line: &str, start: String, end: String) -> Result<String, SubtitleError> {
    let arrow = line
        .find("-->")
        .ok_or_else(|| unsupported("subtitle timing line has no --> separator"))?;
    let left = &line[..arrow];
    let left_start = left
        .find(|ch: char| !ch.is_whitespace())
        .ok_or_else(|| unsupported("subtitle timing line has no start timestamp"))?;
    let left_end = left.trim_end().len();
    let right_start = arrow + 3 + line[arrow + 3..].len() - line[arrow + 3..].trim_start().len();
    let right_end = line[right_start..]
        .find(char::is_whitespace)
        .map(|relative| right_start + relative)
        .unwrap_or(line.len());
    if left_end <= left_start || right_end <= right_start {
        return Err(unsupported("subtitle timing line has an invalid timestamp"));
    }
    Ok(format!(
        "{}{}{}{}{}{}{}",
        &line[..left_start],
        start,
        &line[left_end..arrow],
        "-->",
        &line[arrow + 3..right_start],
        end,
        &line[right_end..]
    ))
}

fn replace_field_time(field: &str, value: String) -> Result<String, SubtitleError> {
    let start = field
        .find(|ch: char| !ch.is_whitespace())
        .ok_or_else(|| unsupported("ASS/SSA time field is empty"))?;
    let end = field.trim_end().len();
    Ok(format!("{}{}{}", &field[..start], value, &field[end..]))
}

fn split_event_values(value: &str, count: usize) -> Option<Vec<String>> {
    if count == 0 {
        return None;
    }
    let mut fields = Vec::with_capacity(count);
    let mut remaining = value;
    for _ in 1..count {
        let (field, rest) = remaining.split_once(',')?;
        fields.push(field.to_owned());
        remaining = rest;
    }
    fields.push(remaining.to_owned());
    Some(fields)
}

fn ass_event_fields(document: &str) -> Result<Vec<String>, SubtitleError> {
    let mut in_events = false;
    for line in document.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_events = trimmed[1..trimmed.len() - 1].eq_ignore_ascii_case("Events");
            continue;
        }
        if in_events {
            if let Some((key, value)) = line.split_once(':') {
                if key.trim().eq_ignore_ascii_case("Format") {
                    return Ok(value
                        .split(',')
                        .map(|field| field.trim().to_owned())
                        .collect());
                }
            }
        }
    }
    Err(unsupported(
        "ASS/SSA document has no Events Format declaration",
    ))
}

fn split_caption_text(text: &str) -> Result<Vec<String>, SubtitleError> {
    if text.contains("\n\n") {
        return Err(unsupported(
            "blank lines inside a cue cannot be safely represented by SRT/WebVTT block syntax",
        ));
    }
    if text.is_empty() {
        Ok(Vec::new())
    } else {
        Ok(text.split('\n').map(str::to_owned).collect())
    }
}

fn reject_markup_edit(text: &str, format: SubtitleFormat) -> Result<(), SubtitleError> {
    let unsafe_markup = match format {
        SubtitleFormat::Srt | SubtitleFormat::Vtt => text.contains('<') && text.contains('>'),
        SubtitleFormat::Ass | SubtitleFormat::Ssa => {
            text.contains("{\\")
                || text.contains("\\N")
                || text.contains("\\n")
                || text.contains("\\h")
        }
    };
    if unsafe_markup {
        return Err(unsupported(format!(
            "editing text with {} inline markup is unsupported until its tag positions can be preserved",
            format_name(format)
        )));
    }
    Ok(())
}

fn export_srt(
    cues: &[(&Cue, u64, u64)],
    source_format: SubtitleFormat,
) -> Result<String, SubtitleError> {
    let mut output = String::new();
    for (index, (cue, start, end)) in cues.iter().enumerate() {
        output.push_str(&(index + 1).to_string());
        output.push('\n');
        output.push_str(&format_srt_time(*start)?);
        output.push_str(" --> ");
        output.push_str(&format_srt_time(*end)?);
        output.push('\n');
        output.push_str(&to_plain_text(cue, source_format));
        output.push_str("\n\n");
    }
    Ok(output)
}

fn export_vtt(
    cues: &[(&Cue, u64, u64)],
    source_format: SubtitleFormat,
) -> Result<String, SubtitleError> {
    let mut output = String::from("WEBVTT\n\n");
    for (cue, start, end) in cues {
        output.push_str(&cue.id);
        output.push('\n');
        output.push_str(&format_vtt_time(*start)?);
        output.push_str(" --> ");
        output.push_str(&format_vtt_time(*end)?);
        output.push('\n');
        output.push_str(&to_plain_text(cue, source_format));
        output.push_str("\n\n");
    }
    Ok(output)
}

fn export_ass(
    cues: &[(&Cue, u64, u64)],
    ssa: bool,
    source_format: SubtitleFormat,
) -> Result<String, SubtitleError> {
    let mut output = if ssa {
        concat!(
            "[Script Info]\nScriptType: v4.00\nWrapStyle: 0\n\n",
            "[V4 Styles]\n",
            "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, TertiaryColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, AlphaLevel, Encoding\n",
            "Style: Default,Arial,48,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,0,0,1,2,1,2,20,20,24,0,1\n\n",
            "[Events]\n",
            "Format: Marked, Start, End, Style, Name, MarginL, MarginR, Effect, Text\n"
        )
        .to_owned()
    } else {
        concat!(
            "[Script Info]\nScriptType: v4.00+\nWrapStyle: 0\nScaledBorderAndShadow: yes\n\n",
            "[V4+ Styles]\n",
            "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
            "Style: Default,Arial,48,&H00FFFFFF,&H000000FF,&H00000000,&H80000000,0,0,0,0,100,100,0,0,1,2,1,2,20,20,24,1\n\n",
            "[Events]\n",
            "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n"
        )
        .to_owned()
    };
    for (cue, start, end) in cues {
        if ssa {
            output.push_str(&format!(
                "Dialogue: Marked=0,{},{},Default,,0,0,,{}\n",
                format_ass_time(*start)?,
                format_ass_time(*end)?,
                to_ass_text(cue, source_format)
            ));
        } else {
            output.push_str(&format!(
                "Dialogue: 0,{},{},Default,,0,0,0,,{}\n",
                format_ass_time(*start)?,
                format_ass_time(*end)?,
                to_ass_text(cue, source_format)
            ));
        }
    }
    Ok(output)
}

fn to_plain_text(cue: &Cue, source: SubtitleFormat) -> String {
    match source {
        SubtitleFormat::Ass | SubtitleFormat::Ssa => {
            let mut plain = String::new();
            let mut chars = cue.text.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '{' && chars.peek() == Some(&'\\') {
                    chars.next();
                    for tag in chars.by_ref() {
                        if tag == '}' {
                            break;
                        }
                    }
                    continue;
                }
                if ch == '\\' {
                    match chars.peek() {
                        Some('N' | 'n') => {
                            chars.next();
                            plain.push('\n');
                            continue;
                        }
                        Some('h') => {
                            chars.next();
                            plain.push('\u{00a0}');
                            continue;
                        }
                        _ => {}
                    }
                }
                plain.push(ch);
            }
            plain
        }
        SubtitleFormat::Vtt => strip_vtt_tags(&cue.text),
        SubtitleFormat::Srt => cue.text.clone(),
    }
}

fn to_ass_text(cue: &Cue, source: SubtitleFormat) -> String {
    to_plain_text(cue, source).replace('\n', "\\N")
}

fn strip_vtt_tags(text: &str) -> String {
    let mut output = String::new();
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => output.push(ch),
            _ => {}
        }
    }
    output
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn format_srt_time(ticks: u64) -> Result<String, SubtitleError> {
    let millis = exact_units(ticks, MILLISECOND_TICKS)?;
    let hours = millis / 3_600_000;
    let minutes = millis / 60_000 % 60;
    let seconds = millis / 1_000 % 60;
    let sub = millis % 1_000;
    Ok(format!("{hours:02}:{minutes:02}:{seconds:02},{sub:03}"))
}

fn format_vtt_time(ticks: u64) -> Result<String, SubtitleError> {
    let millis = exact_units(ticks, MILLISECOND_TICKS)?;
    let hours = millis / 3_600_000;
    let minutes = millis / 60_000 % 60;
    let seconds = millis / 1_000 % 60;
    let sub = millis % 1_000;
    Ok(format!("{hours:02}:{minutes:02}:{seconds:02}.{sub:03}"))
}

fn format_ass_time(ticks: u64) -> Result<String, SubtitleError> {
    let centiseconds = exact_units(ticks, CENTISECOND_TICKS)?;
    let hours = centiseconds / (60 * 60 * 100);
    let minutes = centiseconds / (60 * 100) % 60;
    let seconds = centiseconds / 100 % 60;
    let sub = centiseconds % 100;
    Ok(format!("{hours}:{minutes:02}:{seconds:02}.{sub:02}"))
}

fn exact_units(ticks: u64, unit_ticks: u64) -> Result<u64, SubtitleError> {
    if ticks > MAX_TICKS {
        return Err(SubtitleError::TimeOutOfRange);
    }
    if ticks % unit_ticks != 0 {
        return Err(SubtitleError::InvalidTimePrecision);
    }
    Ok(ticks / unit_ticks)
}

fn validate_export_range(start_tick: u64, end_tick: u64) -> Result<(), SubtitleError> {
    if start_tick >= end_tick || end_tick > MAX_TICKS {
        return Err(SubtitleError::TimeOutOfRange);
    }
    Ok(())
}

fn contains_vtt_cue_id(subtitle: &Subtitle) -> bool {
    subtitle.cues.iter().any(|cue| {
        cue.source_payload
            .lines()
            .next()
            .is_some_and(|first| !first.contains("-->"))
    })
}

fn line_ending(source: &str) -> &str {
    if source.contains("\r\n") {
        "\r\n"
    } else if source.contains('\r') {
        "\r"
    } else {
        "\n"
    }
}

fn has_line_ending(source: &str) -> bool {
    source.ends_with('\n') || source.ends_with('\r')
}

fn rebuild_lines(lines: &[String], ending: &str, trailing_ending: bool) -> String {
    let mut result = lines.join(ending);
    if trailing_ending {
        result.push_str(ending);
    }
    result
}

fn add_loss(items: &mut Vec<LossinessItem>, code: &str, message: &str) {
    if !items.iter().any(|item| item.code == code) {
        items.push(LossinessItem {
            code: code.into(),
            message: message.into(),
        });
    }
}

fn unsupported(message: impl Into<String>) -> SubtitleError {
    SubtitleError::UnsupportedFeature(message.into())
}
