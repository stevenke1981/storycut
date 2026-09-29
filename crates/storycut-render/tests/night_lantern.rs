//! Real-FFmpeg acceptance for the Night Lantern profile: frozen video holds
//! under centered dissolves, sidechain ducking, master loudness and ASS burn.

use serde_json::{Value, json};
use std::path::Path;
use std::process::Command;
use storycut_render::{TIMEBASE, render};

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

fn still(dir: &Path, name: &str, color: &str) {
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        &format!("color=c={color}:s=160x90:d=1,format=rgba"),
        "-frames:v",
        "1",
        dir.join(name).to_str().unwrap(),
    ]);
}

fn motion(duration: u64) -> Value {
    json!({
        "domain_duration_ticks":duration,"sample_offset_tick":0,"fit":"cover",
        "avoid_exposed_edges":true,"anchor":{"x":0.5,"y":0.5},"interpolation":"linear",
        "keyframes":[{"tick":0,"x":0,"y":0,"scale":1,"opacity":1},
                     {"tick":duration-FRAME,"x":0,"y":0,"scale":1,"opacity":1}]
    })
}

fn track(id: &str, kind: &str) -> Value {
    json!({"id":id,"name":id,"kind":kind,"locked":false,"enabled":true,"muted":false,"solo":false,"gain_db":0})
}

fn base_project(
    assets: Value,
    tracks: Value,
    clips: Value,
    transitions: Value,
    subtitles: Value,
) -> Value {
    json!({
        "schema_version":"0.2.0-draft","project_id":"night-lantern","revision":1,"name":"Night Lantern",
        "timebase":TIMEBASE,
        "canvas":{"width":160,"height":90,"fps":{"num":8,"den":1},"background":"#000000","color_mode":"sdr_bt709"},
        "audio_sample_rate":48000,"notes":[],
        "assets":assets,"tracks":tracks,"clips":clips,"transitions":transitions,"links":[],"subtitles":subtitles
    })
}

fn asset(id: &str, kind: &str, path: &str) -> Value {
    json!({"id":id,"kind":kind,"path":path,"probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]})
}

fn render_to(
    project: &Value,
    dir: &Path,
    name: &str,
    end: u64,
    options: Value,
) -> storycut_render::RenderReport {
    let mut opts = json!({"range_start_tick":0,"range_end_tick":end,"subtitle_mode":"none","encoder":"h264_cpu"});
    if let (Some(target), Some(extra)) = (opts.as_object_mut(), options.as_object()) {
        for (key, value) in extra {
            target.insert(key.clone(), value.clone());
        }
    }
    render(
        project,
        &dir.join("project.storycut.json"),
        &dir.join(name),
        &opts,
    )
    .unwrap_or_else(|error| panic!("render failed [{}]: {error}", error.code()))
}

/// Average RGB of the frame nearest to `seconds`.
fn frame_rgb(path: &Path, seconds: f64) -> (f64, f64, f64) {
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-ss",
            &format!("{seconds:.4}"),
            "-i",
        ])
        .arg(path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("decode frame");
    assert!(output.status.success());
    let pixels = output.stdout.len() / 3;
    let mut sum = (0.0, 0.0, 0.0);
    for px in output.stdout.chunks_exact(3) {
        sum.0 += f64::from(px[0]);
        sum.1 += f64::from(px[1]);
        sum.2 += f64::from(px[2]);
    }
    (
        sum.0 / pixels as f64,
        sum.1 / pixels as f64,
        sum.2 / pixels as f64,
    )
}

fn decode_audio(path: &Path) -> Vec<f32> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args([
            "-map", "0:a:0", "-f", "f32le", "-ac", "1", "-ar", "48000", "-",
        ])
        .output()
        .expect("decode audio");
    assert!(output.status.success());
    output
        .stdout
        .chunks_exact(4)
        .map(|s| f32::from_le_bytes(s.try_into().unwrap()))
        .collect()
}

/// Amplitude of one frequency in a mono window (Goertzel).
fn tone_level(samples: &[f32], start_s: f64, end_s: f64, hz: f64) -> f64 {
    let a = (start_s * 48_000.0) as usize;
    let b = (end_s * 48_000.0) as usize;
    let window = &samples[a..b];
    let k = 2.0 * (2.0 * std::f64::consts::PI * hz / 48_000.0).cos();
    let (mut s1, mut s2) = (0.0_f64, 0.0_f64);
    for x in window {
        let s0 = f64::from(*x) + k * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - k * s1 * s2;
    2.0 * power.max(0.0).sqrt() / window.len() as f64
}

#[test]
fn held_native_video_keeps_its_core_opaque_under_centered_dissolves() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    still(dir, "red.png", "red");
    still(dir, "blue.png", "blue");
    // 1 s green then 1 s yellow: distinct first and last source frames.
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=0x00ff00:s=160x90:r=8:d=1",
        "-f",
        "lavfi",
        "-i",
        "color=c=0xffff00:s=160x90:r=8:d=1",
        "-filter_complex",
        "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p[v]",
        "-map",
        "[v]",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        dir.join("native.mp4").to_str().unwrap(),
    ]);
    let s = TIMEBASE;
    let window = 4 * FRAME; // 0.5 s dissolve window
    let video_duration = window + 2 * s + window;
    let project = base_project(
        json!([
            asset("red", "image", "red.png"),
            asset("blue", "image", "blue.png"),
            asset("native", "video", "native.mp4")
        ]),
        json!([track("v", "video")]),
        json!([
            {"id":"red","track_id":"v","asset_id":"red","kind":"image","start_tick":0,"duration_ticks":s + 2*FRAME,"source_in_tick":0,"motion":motion(s + 2*FRAME)},
            {"id":"native","track_id":"v","asset_id":"native","kind":"video","start_tick":s - 2*FRAME,"duration_ticks":video_duration,
             "source_in_tick":0,"stream_index":0,"audio_policy":"muted","hold_head_ticks":window,"hold_tail_ticks":window,"motion":motion(video_duration)},
            {"id":"blue","track_id":"v","asset_id":"blue","kind":"image","start_tick":s - 2*FRAME + video_duration - window,"duration_ticks":s,"source_in_tick":0,"motion":motion(s)}
        ]),
        json!([
            {"id":"t1","track_id":"v","from_clip_id":"red","to_clip_id":"native","start_tick":s - 2*FRAME,"duration_ticks":window,"kind":"cross_dissolve","curve":"linear","audio_policy":"independent"},
            {"id":"t2","track_id":"v","from_clip_id":"native","to_clip_id":"blue","start_tick":s - 2*FRAME + video_duration - window,"duration_ticks":window,"kind":"cross_dissolve","curve":"linear","audio_policy":"independent"}
        ]),
        json!([]),
    );
    let end = s - 2 * FRAME + video_duration - window + s;
    let report = render_to(&project, dir, "held.mp4", end, json!({}));
    assert_eq!(report.frame_count, end / FRAME);
    let out = dir.join("held.mp4");
    // Core: 1.25 s .. 3.25 s is the untouched source (green, then yellow).
    for t in [1.25, 1.5, 2.0, 2.125] {
        let (r, g, b) = frame_rgb(&out, t);
        assert!(
            r < 30.0 && g > 220.0 && b < 30.0,
            "core green at {t}: {r},{g},{b}"
        );
    }
    for t in [2.25, 2.75, 3.125] {
        let (r, g, b) = frame_rgb(&out, t);
        assert!(
            r > 220.0 && g > 220.0 && b < 40.0,
            "core yellow at {t}: {r},{g},{b}"
        );
    }
    // Inside the first window the head hold (first frame, green) blends over red.
    let (r, g, _) = frame_rgb(&out, 1.0);
    assert!(r > 60.0 && g > 60.0, "red→green dissolve at 1.0: {r},{g}");
    // Inside the second window the tail hold (last frame, yellow) blends into blue.
    let (r, g, b) = frame_rgb(&out, 3.5);
    assert!(
        r > 60.0 && g > 60.0 && b > 60.0,
        "yellow→blue dissolve at 3.5: {r},{g},{b}"
    );
}

#[test]
fn held_video_partial_range_inside_a_hold_clones_the_boundary_frame() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=0x00ff00:s=160x90:r=8:d=1",
        "-f",
        "lavfi",
        "-i",
        "color=c=0xffff00:s=160x90:r=8:d=1",
        "-filter_complex",
        "[0:v][1:v]concat=n=2:v=1:a=0,format=yuv420p[v]",
        "-map",
        "[v]",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        dir.join("native.mp4").to_str().unwrap(),
    ]);
    let s = TIMEBASE;
    let duration = s + 2 * s + s;
    let project = base_project(
        json!([asset("native", "video", "native.mp4")]),
        json!([track("v", "video")]),
        json!([{"id":"native","track_id":"v","asset_id":"native","kind":"video","start_tick":0,"duration_ticks":duration,
                "source_in_tick":0,"stream_index":0,"audio_policy":"muted","hold_head_ticks":s,"hold_tail_ticks":s,"motion":motion(duration)}]),
        json!([]),
        json!([]),
    );
    // Range entirely inside the tail hold: every frame is the last (yellow) frame.
    let options = json!({"range_start_tick":3*s + 2*FRAME,"range_end_tick":4*s});
    let output = dir.join("tail.mp4");
    let mut opts = json!({"subtitle_mode":"none","encoder":"h264_cpu"});
    for (k, v) in options.as_object().unwrap() {
        opts[k] = v.clone();
    }
    let report = render(&project, &dir.join("project.storycut.json"), &output, &opts).unwrap();
    assert_eq!(report.frame_count, 6);
    let (r, g, b) = frame_rgb(&output, 0.3);
    assert!(
        r > 220.0 && g > 220.0 && b < 40.0,
        "tail hold is yellow: {r},{g},{b}"
    );
}

#[test]
fn sidechain_ducking_lowers_music_only_while_narration_plays() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=220:sample_rate=48000:duration=3",
        "-af",
        "volume=0.3",
        dir.join("music.wav").to_str().unwrap(),
    ]);
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=1000:sample_rate=48000:duration=1",
        "-af",
        "volume=0.5",
        dir.join("voice.wav").to_str().unwrap(),
    ]);
    still(dir, "black.png", "black");
    let s = TIMEBASE;
    let audio = |duration: u64| json!({"domain_duration_ticks":duration,"sample_offset_tick":0,"gain_db":0,"pan":0,"muted":false,"fade_in_ticks":0,"fade_out_ticks":0,"fade_curve":"linear_amplitude"});
    let make = |ducked: bool| {
        let mut music_track = track("music", "audio");
        if ducked {
            music_track["ducking"] = json!({"source_track_id":"narration","threshold":0.015,"ratio":8,"attack_ms":20,"release_ms":200});
        }
        base_project(
            json!([
                asset("bg", "image", "black.png"),
                asset("music", "audio", "music.wav"),
                asset("voice", "audio", "voice.wav")
            ]),
            json!([
                track("v", "video"),
                track("narration", "audio"),
                music_track
            ]),
            json!([
                {"id":"bg","track_id":"v","asset_id":"bg","kind":"image","start_tick":0,"duration_ticks":3*s,"source_in_tick":0,"motion":motion(3*s)},
                {"id":"voice","track_id":"narration","asset_id":"voice","kind":"audio","start_tick":s,"duration_ticks":s,"source_in_tick":0,"stream_index":0,"audio":audio(s)},
                {"id":"music","track_id":"music","asset_id":"music","kind":"audio","start_tick":0,"duration_ticks":3*s,"source_in_tick":0,"stream_index":0,"audio":audio(3*s)}
            ]),
            json!([]),
            json!([]),
        )
    };
    render_to(&make(false), dir, "plain.mp4", 3 * s, json!({}));
    render_to(&make(true), dir, "ducked.mp4", 3 * s, json!({}));
    let plain = decode_audio(&dir.join("plain.mp4"));
    let ducked = decode_audio(&dir.join("ducked.mp4"));
    let before = tone_level(&ducked, 0.2, 0.8, 220.0) / tone_level(&plain, 0.2, 0.8, 220.0);
    let during = tone_level(&ducked, 1.3, 1.8, 220.0) / tone_level(&plain, 1.3, 1.8, 220.0);
    let after = tone_level(&ducked, 2.4, 2.9, 220.0) / tone_level(&plain, 2.4, 2.9, 220.0);
    assert!(
        (before - 1.0).abs() < 0.1,
        "music untouched before narration: {before}"
    );
    assert!(during < 0.5, "music ducked under narration: {during}");
    assert!(
        (after - 1.0).abs() < 0.15,
        "music recovers after narration: {after}"
    );
    let voice_plain = tone_level(&plain, 1.3, 1.8, 1000.0);
    let voice_ducked = tone_level(&ducked, 1.3, 1.8, 1000.0);
    assert!(
        (voice_ducked / voice_plain - 1.0).abs() < 0.05,
        "narration itself is not compressed"
    );
}

fn integrated_lufs(path: &Path) -> f64 {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-nostats", "-i"])
        .arg(path)
        .args(["-map", "0:a:0", "-af", "ebur128", "-f", "null", "-"])
        .output()
        .expect("measure loudness");
    let text = String::from_utf8_lossy(&output.stderr);
    let summary = &text[text.rfind("Summary:").expect("ebur128 summary")..];
    let line = summary
        .lines()
        .find(|l| l.trim_start().starts_with("I:"))
        .unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[test]
fn master_loudness_two_pass_reaches_the_target() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=6",
        "-af",
        "volume=0.03",
        dir.join("quiet.wav").to_str().unwrap(),
    ]);
    still(dir, "black.png", "black");
    let s = TIMEBASE;
    let project = base_project(
        json!([
            asset("bg", "image", "black.png"),
            asset("tone", "audio", "quiet.wav")
        ]),
        json!([track("v", "video"), track("a", "audio")]),
        json!([
            {"id":"bg","track_id":"v","asset_id":"bg","kind":"image","start_tick":0,"duration_ticks":6*s,"source_in_tick":0,"motion":motion(6*s)},
            {"id":"tone","track_id":"a","asset_id":"tone","kind":"audio","start_tick":0,"duration_ticks":6*s,"source_in_tick":0,"stream_index":0,
             "audio":{"domain_duration_ticks":6*s,"sample_offset_tick":0,"gain_db":0,"pan":0,"muted":false,"fade_in_ticks":0,"fade_out_ticks":0,"fade_curve":"linear_amplitude"}}
        ]),
        json!([]),
        json!([]),
    );
    let plain = render_to(&project, dir, "plain.mp4", 6 * s, json!({}));
    assert!(plain.master_loudness.is_none());
    let report = render_to(
        &project,
        dir,
        "loud.mp4",
        6 * s,
        json!({"master_loudness":{"integrated_lufs":-16,"true_peak_db":-1.5}}),
    );
    let measured = report.master_loudness.expect("loudness report");
    assert!(
        measured["measured_before"]["integrated_lufs"]
            .as_f64()
            .unwrap()
            < -25.0
    );
    let before = integrated_lufs(&dir.join("plain.mp4"));
    let after = integrated_lufs(&dir.join("loud.mp4"));
    assert!(before < -25.0, "source is quiet: {before}");
    assert!(
        (after + 16.0).abs() <= 1.0,
        "normalized to -16 LUFS: {after}"
    );
    assert_eq!(report.frame_count, 48);
}

#[test]
fn ass_burn_keeps_styles_and_follows_offset_and_range() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    still(dir, "black.png", "black");
    let s = TIMEBASE;
    // A full-width white box via a huge border: easy to detect by brightness.
    let raw = "[Script Info]\r\nScriptType: v4.00+\r\nPlayResX: 160\r\nPlayResY: 90\r\n\r\n[V4+ Styles]\r\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\r\nStyle: Box,Arial,40,&H00FFFFFF,&H00FFFFFF,&H00FFFFFF,&H00FFFFFF,0,0,0,0,100,100,0,0,3,30,0,5,0,0,0,1\r\n\r\n[Events]\r\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\r\nDialogue: 0,0:00:01.00,0:00:02.00,Box,,0,0,0,,WWWW\r\n";
    let project = base_project(
        json!([asset("bg", "image", "black.png")]),
        json!([track("v", "video"), track("subs", "subtitle")]),
        json!([{"id":"bg","track_id":"v","asset_id":"bg","kind":"image","start_tick":0,"duration_ticks":6*s,"source_in_tick":0,"motion":motion(6*s)}]),
        json!([]),
        json!([serde_json::to_value(
            storycut_subtitle::parse_subtitle_content(
                "subs.ass",
                "subs",
                2 * s,
                raw,
                storycut_core::SubtitleFormat::Ass
            )
            .expect("parse ASS")
        )
        .unwrap()]),
    );
    // Offset 2 s moves the cue to 3..4 s; the range starts at 2 s, so it shows at 1..2 s.
    let output = dir.join("ass.mp4");
    let report = render(
        &project,
        &dir.join("project.storycut.json"),
        &output,
        &json!({"range_start_tick":2*s,"range_end_tick":5*s,"subtitle_mode":"burn","encoder":"h264_cpu"}),
    )
    .unwrap_or_else(|error| panic!("ass burn failed [{}]: {error}", error.code()));
    assert_eq!(report.frame_count, 24);
    let lit = frame_rgb(&output, 1.5).0;
    let dark_before = frame_rgb(&output, 0.5).0;
    let dark_after = frame_rgb(&output, 2.5).0;
    assert!(lit > 60.0, "styled box is burned during the cue: {lit}");
    assert!(
        dark_before < 5.0 && dark_after < 5.0,
        "no cue outside: {dark_before} {dark_after}"
    );
}
