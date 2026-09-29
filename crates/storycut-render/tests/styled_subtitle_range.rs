use std::{path::Path, process::Command};

use serde_json::{Value, json};
use storycut_core::{SubtitleFormat, TIMEBASE};

const FRAME: u64 = TIMEBASE / 8;

fn ffmpeg(args: &[&str]) {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-y"])
        .args(args)
        .output()
        .expect("launch ffmpeg");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn track(id: &str, kind: &str) -> Value {
    json!({
        "id":id,"name":id,"kind":kind,"locked":false,"enabled":true,
        "muted":false,"solo":false,"gain_db":0
    })
}

fn motion(duration: u64) -> Value {
    json!({
        "domain_duration_ticks":duration,"sample_offset_tick":0,"fit":"cover",
        "avoid_exposed_edges":true,"anchor":{"x":0.5,"y":0.5},"interpolation":"linear",
        "keyframes":[
            {"tick":0,"x":0,"y":0,"scale":1,"opacity":1},
            {"tick":duration-FRAME,"x":0,"y":0,"scale":1,"opacity":1}
        ]
    })
}

fn decode_rgb(path: &Path, at_seconds: Option<f64>) -> Vec<u8> {
    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-nostdin", "-v", "error", "-i"]);
    command.arg(path);
    if let Some(seconds) = at_seconds {
        command.args(["-ss", &format!("{seconds:.6}")]);
    }
    let output = command
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("decode video frame");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout.len(), 320 * 120 * 3);
    output.stdout
}

fn region_brightness(frame: &[u8], y_start: usize, y_end: usize) -> u64 {
    let mut total = 0;
    for y in y_start..y_end {
        for x in 0..320 {
            let at = (y * 320 + x) * 3;
            total += u64::from(frame[at]) + u64::from(frame[at + 1]) + u64::from(frame[at + 2]);
        }
    }
    total
}

fn bright_pixel_centroid_x(frame: &[u8], y_start: usize, y_end: usize) -> f64 {
    let mut weighted_x = 0_u64;
    let mut weight = 0_u64;
    for y in y_start..y_end {
        for x in 0..320 {
            let at = (y * 320 + x) * 3;
            let brightness =
                u32::from(frame[at]) + u32::from(frame[at + 1]) + u32::from(frame[at + 2]);
            if brightness > 90 {
                weighted_x += (x as u64) * u64::from(brightness);
                weight += u64::from(brightness);
            }
        }
    }
    assert!(
        weight > 0,
        "moving ASS cue is visible in the selected region"
    );
    weighted_x as f64 / weight as f64
}

#[test]
fn ass_animations_keep_absolute_phase_in_partial_render_and_preview() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=320x120:r=8:d=3",
        "-frames:v",
        "1",
        dir.join("black.png").to_str().unwrap(),
    ]);

    let raw = "[Script Info]\r\nScriptType: v4.00+\r\nPlayResX: 320\r\nPlayResY: 120\r\n\r\n[V4+ Styles]\r\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\r\nStyle: Default,Arial,28,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1\r\n\r\n[Events]\r\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\n註解: 保留未識別的繁體中文事件資料\r\nDialogue: 0,0:00:00.00,0:00:03.00,Default,,0,0,0,,{\\move(10,65,240,65,0,3000)}M\r\nDialogue: 0,0:00:00.00,0:00:03.00,Default,,0,0,0,,{\\pos(10,10)\\fad(1000,0)}F\r\n";
    let subtitle = storycut_subtitle::parse_subtitle_content(
        "motion.ass",
        "subs",
        0,
        raw,
        SubtitleFormat::Ass,
    )
    .expect("parse ASS");
    let export = storycut_subtitle::export_subtitle(&subtitle, SubtitleFormat::Ass)
        .expect("export ASS without flattening unknown event data");
    assert!(
        export
            .content
            .contains("註解: 保留未識別的繁體中文事件資料")
    );
    let duration = 3 * TIMEBASE;
    let project = json!({
        "schema_version":"0.2.0-draft","project_id":"ass-range","revision":1,"name":"ASS range",
        "timebase":TIMEBASE,
        "canvas":{"width":320,"height":120,"fps":{"num":8,"den":1},"background":"#000000","color_mode":"sdr_bt709"},
        "audio_sample_rate":48000,"notes":[],
        "assets":[{"id":"bg","kind":"image","path":"black.png","probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]}],
        "tracks":[track("v","video"),track("subs","subtitle")],
        "clips":[{"id":"bg","track_id":"v","asset_id":"bg","kind":"image","start_tick":0,"duration_ticks":duration,"source_in_tick":0,"motion":motion(duration)}],
        "transitions":[],"links":[],
        "subtitles":[serde_json::to_value(subtitle).unwrap()]
    });
    let project_path = dir.join("project.storycut.json");
    let full_path = dir.join("full.mp4");
    let partial_path = dir.join("partial.mp4");
    let preview_path = dir.join("preview.png");
    let partial_start = 4 * FRAME; // 0.5 s, within the ASS fade-in and move.
    let partial_end = 20 * FRAME; // 2.5 s.

    storycut_render::render(
        &project,
        &project_path,
        &full_path,
        &json!({"range_end_tick":duration,"subtitle_mode":"burn","encoder":"h264_cpu"}),
    )
    .expect("full ASS render");
    storycut_render::render(
        &project,
        &project_path,
        &partial_path,
        &json!({"range_start_tick":partial_start,"range_end_tick":partial_end,"subtitle_mode":"burn","encoder":"h264_cpu"}),
    )
    .expect("partial ASS render");
    storycut_render::render_frame(
        &project,
        &project_path,
        &preview_path,
        partial_start,
        320,
        120,
        true,
    )
    .expect("ASS preview frame");

    let full_at_start = decode_rgb(&full_path, Some(0.5));
    let partial_first = decode_rgb(&partial_path, Some(0.0));
    let preview = decode_rgb(&preview_path, None);

    // At absolute 0.5 s, the fade-in is half visible. The clipped preview and
    // the first partial-render frame must preserve that same animation phase.
    let full_fade = region_brightness(&full_at_start, 0, 45);
    let partial_fade = region_brightness(&partial_first, 0, 45);
    let preview_fade = region_brightness(&preview, 0, 45);
    assert!(
        full_fade > 10_000,
        "full render shows the mid-fade cue: {full_fade}"
    );
    assert!(
        partial_fade.abs_diff(full_fade) < full_fade / 3,
        "partial render keeps fade phase: full={full_fade}, partial={partial_fade}"
    );
    assert!(
        preview_fade.abs_diff(full_fade) < full_fade / 3,
        "preview keeps fade phase: full={full_fade}, preview={preview_fade}"
    );

    // At the same absolute time, the moving cue should occupy the same x
    // position. Rewriting Dialogue.Start without adjusting \move restarts it.
    let full_move = bright_pixel_centroid_x(&full_at_start, 45, 100);
    let partial_move = bright_pixel_centroid_x(&partial_first, 45, 100);
    let preview_move = bright_pixel_centroid_x(&preview, 45, 100);
    assert!(
        (partial_move - full_move).abs() < 4.0,
        "partial render keeps move phase: full={full_move}, partial={partial_move}"
    );
    assert!(
        (preview_move - full_move).abs() < 4.0,
        "preview keeps move phase: full={full_move}, preview={preview_move}"
    );
}
