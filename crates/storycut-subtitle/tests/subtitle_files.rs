use std::{fs, path::PathBuf};

use storycut_core::{SubtitleFormat, TIMEBASE};
use storycut_subtitle::{
    SubtitleError, assess_lossiness, export_subtitle, parse_subtitle, parse_subtitle_content,
};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name)
}

#[test]
fn parses_real_srt_file_and_returns_exact_source_when_unedited() {
    let path = fixture("captions.srt");
    let subtitle = parse_subtitle(&path, "subtitle_1", 0).unwrap();
    assert_eq!(subtitle.format, SubtitleFormat::Srt);
    assert_eq!(subtitle.cues.len(), 2);
    assert_eq!(subtitle.cues[0].start_tick, TIMEBASE);
    assert_eq!(subtitle.cues[0].end_tick, 4 * TIMEBASE);
    assert_eq!(subtitle.cues[0].text, "第一張圖片，緩緩向右移動。");
    assert!(subtitle.preserve_unknown_syntax);

    let exported = export_subtitle(&subtitle, SubtitleFormat::Srt).unwrap();
    assert!(exported.exact_source);
    assert_eq!(exported.content, fs::read_to_string(path).unwrap());
    assert!(!exported.lossiness.is_lossy());
}

#[test]
fn parses_real_ass_file_and_preserves_styles_and_comments_on_time_edit() {
    let subtitle = parse_subtitle(fixture("captions.ass"), "subtitle_1", 0).unwrap();
    assert_eq!(subtitle.format, SubtitleFormat::Ass);
    assert_eq!(subtitle.cues.len(), 2);
    assert_eq!(subtitle.cues[0].start_tick, TIMEBASE);

    let mut edited = subtitle.clone();
    edited.cues[0].start_tick += TIMEBASE;
    let exported = export_subtitle(&edited, SubtitleFormat::Ass).unwrap();
    assert!(!exported.exact_source);
    assert!(exported.content.contains("[V4+ Styles]"));
    assert!(exported.content.contains("Comment: 0,0:00:00.00"));
    assert!(
        exported
            .content
            .contains("Dialogue: 0,0:00:02.00,0:00:04.00")
    );

    let reparsed = parse_subtitle_content(
        "edited.ass",
        "subtitle_1",
        0,
        exported.content,
        SubtitleFormat::Ass,
    )
    .unwrap();
    assert_eq!(reparsed.cues[0].start_tick, 2 * TIMEBASE);
}

#[test]
fn parses_and_exports_a_real_ssa_file_without_flattening_its_source_syntax() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.ssa");
    let source = concat!(
        "[Script Info]\n",
        "Title: Legacy sample\n",
        "ScriptType: v4.00\n\n",
        "[V4 Styles]\n",
        "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, TertiaryColour, BackColour, Bold, Italic, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, AlphaLevel, Encoding\n",
        "Style: Default,Arial,40,&H00FFFFFF,&H0000FFFF,&H00000000,&H80000000,0,0,1,2,0,2,20,20,20,0,1\n\n",
        "[Events]\n",
        "Format: Marked, Start, End, Style, Name, MarginL, MarginR, Effect, Text\n",
        "Dialogue: Marked=0,0:00:01.00,0:00:02.50,Default,,0,0,,Legacy caption\n"
    );
    fs::write(&path, source).unwrap();
    let subtitle = parse_subtitle(&path, "subtitle_1", 0).unwrap();
    assert_eq!(subtitle.format, SubtitleFormat::Ssa);
    assert_eq!(subtitle.cues.len(), 1);
    assert_eq!(subtitle.cues[0].text, "Legacy caption");
    let exported = export_subtitle(&subtitle, SubtitleFormat::Ssa).unwrap();
    assert!(exported.exact_source);
    assert_eq!(exported.content, source);
}

#[test]
fn parses_real_vtt_file_and_preserves_notes_and_cue_settings() {
    let path = fixture("captions.vtt");
    let subtitle = parse_subtitle(&path, "subtitle_1", 0).unwrap();
    assert_eq!(subtitle.format, SubtitleFormat::Vtt);
    assert_eq!(subtitle.cues.len(), 2);
    assert_eq!(subtitle.cues[0].text, "第一張圖片，緩緩向右移動。");
    let exported = export_subtitle(&subtitle, SubtitleFormat::Vtt).unwrap();
    assert!(exported.exact_source);
    assert_eq!(exported.content, fs::read_to_string(path).unwrap());

    let mut edited = subtitle;
    edited.cues[0].end_tick += TIMEBASE;
    let changed = export_subtitle(&edited, SubtitleFormat::Vtt).unwrap();
    assert!(changed.content.contains("NOTE 此為字幕功能驗收樣本"));
    assert!(
        changed
            .content
            .contains("position:50% align:center size:90%")
    );
    assert!(changed.content.contains("00:00:05.000"));
}

#[test]
fn webvtt_cue_identifiers_that_begin_with_note_are_not_treated_as_note_blocks() {
    let subtitle = parse_subtitle_content(
        "note-id.vtt",
        "subtitle_1",
        0,
        "WEBVTT\n\nNOTE123\n00:00:01.000 --> 00:00:02.000\nCaption\n",
        SubtitleFormat::Vtt,
    )
    .unwrap();
    assert_eq!(subtitle.cues.len(), 1);
    assert_eq!(subtitle.cues[0].text, "Caption");
}

#[test]
fn document_offset_is_applied_only_when_exporting_the_standalone_file() {
    let subtitle = parse_subtitle(fixture("captions.srt"), "subtitle_1", 5 * TIMEBASE).unwrap();
    assert_eq!(subtitle.cues[0].start_tick, TIMEBASE);
    assert_eq!(subtitle.offset_tick, 5 * TIMEBASE);
    let exported = export_subtitle(&subtitle, SubtitleFormat::Srt).unwrap();
    assert!(!exported.exact_source);
    assert!(exported.content.contains("00:00:06,000 --> 00:00:09,000"));
}

#[test]
fn edits_plain_srt_text_and_timing_while_retaining_other_cues() {
    let mut subtitle = parse_subtitle(fixture("captions.srt"), "subtitle_1", 0).unwrap();
    subtitle.cues[0].start_tick += TIMEBASE;
    subtitle.cues[0].text = "已修改的字幕".into();
    let exported = export_subtitle(&subtitle, SubtitleFormat::Srt).unwrap();
    assert!(exported.content.contains("00:00:02,000 --> 00:00:04,000"));
    assert!(exported.content.contains("已修改的字幕"));
    assert!(exported.content.contains("第二張圖片，緩緩向上移動。"));

    let reparsed = parse_subtitle_content(
        "edited.srt",
        "subtitle_1",
        0,
        exported.content,
        SubtitleFormat::Srt,
    )
    .unwrap();
    assert_eq!(reparsed.cues[0].text, "已修改的字幕");
    assert_eq!(reparsed.cues[1].start_tick, 10 * TIMEBASE);
}

#[test]
fn cross_format_export_reports_lossiness_before_information_is_flattened() {
    let subtitle = parse_subtitle(fixture("captions.vtt"), "subtitle_1", 0).unwrap();
    let report = assess_lossiness(&subtitle, SubtitleFormat::Srt);
    assert!(report.is_lossy());
    assert!(
        report
            .items
            .iter()
            .any(|item| item.code == "vtt_document_blocks")
    );
    assert!(
        report
            .items
            .iter()
            .any(|item| item.code == "vtt_cue_settings")
    );

    let exported = export_subtitle(&subtitle, SubtitleFormat::Srt).unwrap();
    assert_eq!(exported.format, SubtitleFormat::Srt);
    assert!(exported.lossiness.is_lossy());
    assert!(exported.content.starts_with("1\n"));
}

#[test]
fn cross_format_conversion_cleans_source_specific_inline_markup() {
    let ass = parse_subtitle_content(
        "marked.ass",
        "subtitle_1",
        0,
        concat!(
            "[Script Info]\nScriptType: v4.00+\n\n",
            "[Events]\n",
            "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n",
            "Dialogue: 0,0:00:01.00,0:00:02.00,Default,,0,0,0,,{\\pos(960,920)}Hello\\NWorld\n"
        ),
        SubtitleFormat::Ass,
    )
    .unwrap();
    let converted = export_subtitle(&ass, SubtitleFormat::Srt).unwrap();
    assert!(converted.content.contains("Hello\nWorld"));
    assert!(!converted.content.contains("\\pos"));

    let vtt = parse_subtitle_content(
        "marked.vtt",
        "subtitle_1",
        0,
        "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n<b>Hello</b> <c.green>there</c>\n",
        SubtitleFormat::Vtt,
    )
    .unwrap();
    let converted = export_subtitle(&vtt, SubtitleFormat::Srt).unwrap();
    assert!(converted.content.contains("Hello there"));
    assert!(!converted.content.contains("<b>"));
}

#[test]
fn generated_ass_and_ssa_documents_are_parseable_and_keep_cue_times() {
    let subtitle = parse_subtitle(fixture("captions.srt"), "subtitle_1", 0).unwrap();
    for format in [SubtitleFormat::Ass, SubtitleFormat::Ssa] {
        let exported = export_subtitle(&subtitle, format).unwrap();
        let reparsed = parse_subtitle_content(
            "converted.subtitle",
            "subtitle_1",
            0,
            exported.content,
            format,
        )
        .unwrap();
        assert_eq!(reparsed.format, format);
        assert_eq!(reparsed.cues.len(), subtitle.cues.len());
        assert_eq!(reparsed.cues[0].start_tick, subtitle.cues[0].start_tick);
        assert_eq!(reparsed.cues[0].text, subtitle.cues[0].text);
    }
}

#[test]
fn same_format_text_edit_with_ass_override_tags_is_explicitly_unsupported() {
    let mut subtitle = parse_subtitle(fixture("captions.ass"), "subtitle_1", 0).unwrap();
    subtitle.cues[0].text = "替換文字".into();
    let error = export_subtitle(&subtitle, SubtitleFormat::Ass).unwrap_err();
    assert_eq!(error.code(), "UNSUPPORTED_FEATURE");
}

#[test]
fn utf16_bom_and_unicode_windows_style_path_are_read_from_a_real_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("字幕測試.srt");
    let text = "1\r\n00:00:00,000 --> 00:00:01,000\r\n繁體中文\r\n";
    let mut bytes = vec![0xff, 0xfe];
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    fs::write(&path, bytes).unwrap();

    let subtitle = parse_subtitle(&path, "subtitle_1", 0).unwrap();
    assert_eq!(subtitle.cues[0].text, "繁體中文");
    assert_eq!(subtitle.path, path.to_string_lossy());
    assert!(
        export_subtitle(&subtitle, SubtitleFormat::Srt)
            .unwrap()
            .exact_source
    );
}

#[test]
fn refuses_time_precision_that_the_target_format_cannot_represent() {
    let mut subtitle = parse_subtitle(fixture("captions.srt"), "subtitle_1", 0).unwrap();
    subtitle.cues[0].start_tick += 1;
    let error = export_subtitle(&subtitle, SubtitleFormat::Srt).unwrap_err();
    assert!(matches!(error, SubtitleError::InvalidTimePrecision));
}

#[test]
fn refuses_an_offset_that_would_push_export_past_the_project_limit() {
    let mut subtitle = parse_subtitle(
        fixture("captions.srt"),
        "subtitle_1",
        24 * 60 * 60 * TIMEBASE - 18 * TIMEBASE,
    )
    .unwrap();
    subtitle.offset_tick += TIMEBASE / 1_000;
    let error = export_subtitle(&subtitle, SubtitleFormat::Srt).unwrap_err();
    assert!(matches!(error, SubtitleError::TimeOutOfRange));
}

#[test]
fn malformed_caption_file_returns_a_parse_error_instead_of_dropping_its_block() {
    let error = parse_subtitle_content(
        "broken.srt",
        "subtitle_1",
        0,
        "1\nnot a timing line\nCaption\n",
        SubtitleFormat::Srt,
    )
    .unwrap_err();
    assert_eq!(error.code(), "INVALID_SUBTITLE");

    let error = parse_subtitle_content(
        "broken.vtt",
        "subtitle_1",
        0,
        "WEBVTT\n00:00:01.000 --> 00:00:02.000\nCaption\n",
        SubtitleFormat::Vtt,
    )
    .unwrap_err();
    assert_eq!(error.code(), "INVALID_SUBTITLE");
}
