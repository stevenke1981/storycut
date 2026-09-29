use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use storycut_render::{
    RenderReport, TIMEBASE, probe_media, render, render_frame, render_frame_with_root,
    render_with_root,
};

fn ffmpeg(args: &[&str]) -> Result<(), String> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-n"])
        .args(args)
        .output()
        .map_err(|e| format!("could not launch ffmpeg: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    Ok(())
}

fn unique_artifact_dir() -> (PathBuf, Option<tempfile::TempDir>) {
    if std::env::var_os("STORYCUT_KEEP_RENDER_ARTIFACTS").is_some() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("media-generated");
        fs::create_dir_all(&root).expect("create test media output directory");
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let run = root.join(format!("acceptance-{stamp}"));
        fs::create_dir(&run).expect("create unique acceptance output directory");
        (run, None)
    } else {
        let tmp = tempfile::tempdir().expect("create temporary fixture directory");
        (tmp.path().to_path_buf(), Some(tmp))
    }
}

fn generate_sources(dir: &Path) {
    let video = dir.join("blue_with_tone.mp4");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=blue:s=160x90:r=8:d=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-pix_fmt",
        "yuv420p",
        "-c:a",
        "aac",
        "-shortest",
        video.to_str().unwrap(),
    ])
    .expect("generate video and audio source");

    let logo = dir.join("half_alpha_red.png");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=red@0.5:s=32x16:d=1,format=rgba",
        "-frames:v",
        "1",
        logo.to_str().unwrap(),
    ])
    .expect("generate alpha PNG overlay");

    for (name, color) in [("crossfade_blue.png", "blue"), ("crossfade_red.png", "red")] {
        let image = dir.join(name);
        ffmpeg(&[
            "-f",
            "lavfi",
            "-i",
            &format!("color=c={color}:s=160x90:d=1,format=rgba"),
            "-frames:v",
            "1",
            image.to_str().unwrap(),
        ])
        .expect("generate cross-dissolve source image");
    }
}

fn project() -> Value {
    let second = TIMEBASE;
    json!({
        "schema_version":"0.2.0-draft", "project_id":"render-acceptance", "revision":7,
        "name":"Generated renderer acceptance", "timebase":TIMEBASE,
        "canvas":{"width":160,"height":90,"fps":{"num":8,"den":1},"background":"#000000","color_mode":"sdr_bt709"},
        "audio_sample_rate":48000,
        "notes":[],
        "assets":[
            {"id":"source","kind":"video","path":"blue_with_tone.mp4","probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]},
            {"id":"logo","kind":"image","path":"half_alpha_red.png","probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]}
        ],
        "tracks":[
            {"id":"base","name":"Base video","kind":"video","locked":false,"enabled":true,"muted":false,"solo":false,"gain_db":0},
            {"id":"overlay","name":"Alpha overlay","kind":"image","locked":false,"enabled":true,"muted":false,"solo":false,"gain_db":0},
            {"id":"voice","name":"Audio","kind":"audio","locked":false,"enabled":true,"muted":false,"solo":false,"gain_db":0},
            {"id":"captions","name":"Captions","kind":"subtitle","locked":false,"enabled":true,"muted":false,"solo":false,"gain_db":0}
        ],
        "clips":[
            {"id":"base-clip","track_id":"base","asset_id":"source","kind":"video","start_tick":0,"duration_ticks":2*second,"source_in_tick":0,"stream_index":0,"audio_policy":"muted",
             "motion":{"domain_duration_ticks":2*second,"sample_offset_tick":0,"fit":"cover","avoid_exposed_edges":true,"anchor":{"x":0.5,"y":0.5},"interpolation":"linear","keyframes":[{"tick":0,"x":0,"y":0,"scale":1,"opacity":1},{"tick":2*second-88200000,"x":0,"y":0,"scale":1,"opacity":1}]}},
            {"id":"logo-clip","track_id":"overlay","asset_id":"logo","kind":"image","start_tick":second/2,"duration_ticks":second,"source_in_tick":0,
             "motion":{"domain_duration_ticks":second,"sample_offset_tick":0,"fit":"contain","avoid_exposed_edges":false,"anchor":{"x":0.5,"y":0.5},"interpolation":"linear","keyframes":[{"tick":0,"x":0,"y":0,"scale":0.2,"opacity":1},{"tick":second-88200000,"x":0,"y":0,"scale":0.2,"opacity":1}]}},
            {"id":"tone","track_id":"voice","asset_id":"source","kind":"audio","start_tick":second/2,"duration_ticks":second,"source_in_tick":0,"stream_index":1,
             "audio":{"domain_duration_ticks":second,"sample_offset_tick":0,"gain_db":-6,"pan":0,"muted":false,"fade_in_ticks":second/10,"fade_out_ticks":0,"fade_curve":"linear_amplitude"}}
        ],
        "transitions":[], "links":[],
        "subtitles":[
            {"id":"sub","track_id":"captions","format":"srt","offset_tick":0,"path":"caption.srt",
             "raw_document":"1\n00:00:01,000 --> 00:00:01,750\nSTORYCUT TEST\n","preserve_unknown_syntax":true,
             "cues":[{"id":"cue","start_tick":second,"end_tick":second+second*3/4,"text":"STORYCUT TEST","source_payload":""}]}
        ]
    })
}

fn render_one(project: &Value, dir: &Path, name: &str, subtitle_mode: &str) -> RenderReport {
    let output = dir.join(name);
    render(
        project,
        &dir.join("project.storycut.json"),
        &output,
        &json!({
            "range_start_tick":0,
            "range_end_tick":2*TIMEBASE,
            "subtitle_mode":subtitle_mode,
            "encoder":"h264_cpu",
            "overwrite":false
        }),
    )
    .unwrap_or_else(|error| panic!("render failed [{}]: {error}", error.code()))
}

fn crossfade_project() -> Value {
    let second = TIMEBASE;
    let motion = |duration: u64| {
        json!({
            "domain_duration_ticks":duration,"sample_offset_tick":0,"fit":"cover",
            "avoid_exposed_edges":true,"anchor":{"x":0.5,"y":0.5},
            "interpolation":"linear","keyframes":[
                {"tick":0,"x":0,"y":0,"scale":1,"opacity":1},
                {"tick":duration-88_200_000,"x":0,"y":0,"scale":1,"opacity":1}
            ]
        })
    };
    json!({
        "schema_version":"0.2.0-draft","project_id":"crossfade-check","revision":1,"name":"Cross dissolve test","timebase":TIMEBASE,
        "canvas":{"width":160,"height":90,"fps":{"num":8,"den":1},"background":"#000000","color_mode":"sdr_bt709"},
        "audio_sample_rate":48000,"notes":[],
        "assets":[
            {"id":"blue","kind":"image","path":"crossfade_blue.png","probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]},
            {"id":"red","kind":"image","path":"crossfade_red.png","probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]}
        ],
        "tracks":[{"id":"v","name":"Video","kind":"video","locked":false,"enabled":true,"muted":false,"solo":false,"gain_db":0}],
        "clips":[
            {"id":"a","track_id":"v","asset_id":"blue","kind":"image","start_tick":0,"duration_ticks":second,"source_in_tick":0,"motion":motion(second)},
            {"id":"b","track_id":"v","asset_id":"red","kind":"image","start_tick":second/2,"duration_ticks":second,"source_in_tick":0,"motion":motion(second)}
        ],
        "transitions":[{"id":"cross","track_id":"v","from_clip_id":"a","to_clip_id":"b","start_tick":second/2,"duration_ticks":second/2,"kind":"cross_dissolve","curve":"linear","audio_policy":"independent"}],
        "links":[],"subtitles":[]
    })
}

fn decode_frame(path: &Path, time: &str) -> Vec<u8> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-ss", time, "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("decode video frame");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn decode_audio(path: &Path) -> Vec<f32> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args([
            "-map", "0:a:0", "-f", "f32le", "-ac", "2", "-ar", "48000", "-",
        ])
        .output()
        .expect("decode audio samples");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
        .stdout
        .chunks_exact(4)
        .map(|s| f32::from_le_bytes(s.try_into().unwrap()))
        .collect()
}

fn rms(samples: &[f32], start_frame: usize, end_frame: usize) -> f64 {
    let mut sum = 0.0_f64;
    let mut count = 0_u64;
    for sample in &samples[start_frame * 2..end_frame * 2] {
        sum += f64::from(*sample) * f64::from(*sample);
        count += 1;
    }
    (sum / count as f64).sqrt()
}

fn audio_fade_project(duration: u64, fade_in: u64, fade_out: u64) -> Value {
    json!({
        "schema_version":"0.2.0-draft","project_id":"audio-range-fade-check","revision":1,
        "name":"Audio range fade acceptance","timebase":TIMEBASE,
        "canvas":{"width":160,"height":90,"fps":{"num":8,"den":1},"background":"#000000","color_mode":"sdr_bt709"},
        "audio_sample_rate":48000,"notes":[],
        "assets":[{"id":"tone","kind":"audio","path":"fade-tone.wav","probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]}],
        "tracks":[{"id":"a","name":"Audio","kind":"audio","locked":false,"enabled":true,"muted":false,"solo":false,"gain_db":0}],
        "clips":[{"id":"fade","track_id":"a","asset_id":"tone","kind":"audio","start_tick":0,"duration_ticks":duration,"source_in_tick":0,"stream_index":0,
            "audio":{"domain_duration_ticks":duration,"sample_offset_tick":0,"gain_db":0,"pan":0,"muted":false,"fade_in_ticks":fade_in,"fade_out_ticks":fade_out,"fade_curve":"linear_amplitude"}}],
        "transitions":[],"links":[],"subtitles":[]
    })
}

fn generate_motion_source(dir: &Path) -> PathBuf {
    let image = dir.join("motion-grid.png");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=160x90:r=8:d=1",
        "-vf",
        "drawbox=x=56:y=29:w=48:h=32:color=white:t=fill",
        "-frames:v",
        "1",
        image.to_str().unwrap(),
    ])
    .expect("generate motion measurement image");
    image
}

fn motion_keyframes(
    duration: u64,
    sample_offset: u64,
    from: [f64; 4],
    to: [f64; 4],
    interpolation: &str,
) -> Value {
    let frame = TIMEBASE / 8;
    json!({
        "domain_duration_ticks":duration,
        "sample_offset_tick":sample_offset,
        "fit":"cover",
        "avoid_exposed_edges":false,
        "anchor":{"x":0.5,"y":0.5},
        "interpolation":interpolation,
        "keyframes":[
            {"tick":0,"x":from[0],"y":from[1],"scale":from[2],"opacity":from[3]},
            {"tick":duration-frame,"x":to[0],"y":to[1],"scale":to[2],"opacity":to[3]}
        ]
    })
}

fn motion_project(clips: Vec<Value>) -> Value {
    json!({
        "schema_version":"0.2.0-draft","project_id":"motion-render-check","revision":1,
        "name":"Motion render acceptance","timebase":TIMEBASE,
        "canvas":{"width":160,"height":90,"fps":{"num":8,"den":1},"background":"#000000","color_mode":"sdr_bt709"},
        "audio_sample_rate":48000,"notes":[],
        "assets":[{"id":"grid","kind":"image","path":"motion-grid.png","probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]}],
        "tracks":[{"id":"v","name":"Video","kind":"video","locked":false,"enabled":true,"muted":false,"solo":false,"gain_db":0}],
        "clips":clips,"transitions":[],"links":[],"subtitles":[]
    })
}

fn render_from_zero(project: &Value, dir: &Path, output_name: &str, end_tick: u64) -> PathBuf {
    let output = dir.join(output_name);
    render(
        project,
        &dir.join("project.storycut.json"),
        &output,
        &json!({
            "range_start_tick":0,
            "range_end_tick":end_tick,
            "subtitle_mode":"none",
            "encoder":"h264_cpu",
            "overwrite":false
        }),
    )
    .unwrap_or_else(|error| panic!("motion render failed [{}]: {error}", error.code()));
    output
}

fn decode_all_frames(path: &Path, width: usize, height: usize) -> Vec<Vec<u8>> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args(["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("decode all rendered motion frames");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let frame_bytes = width * height * 3;
    output
        .stdout
        .chunks_exact(frame_bytes)
        .map(|frame| frame.to_vec())
        .collect()
}

fn bright_bounds(frame: &[u8], width: usize, threshold: u8) -> (f64, f64, usize, usize) {
    let mut min_x = width;
    let mut min_y = frame.len() / (width * 3);
    let mut max_x = 0;
    let mut max_y = 0;
    for (index, pixel) in frame.chunks_exact(3).enumerate() {
        if pixel.iter().all(|channel| *channel >= threshold) {
            let x = index % width;
            let y = index / width;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
        }
    }
    assert!(
        min_x <= max_x && min_y <= max_y,
        "no bright marker detected"
    );
    (
        (min_x + max_x) as f64 / 2.0,
        (min_y + max_y) as f64 / 2.0,
        max_x - min_x + 1,
        max_y - min_y + 1,
    )
}

#[test]
fn real_ffmpeg_motion_moves_in_all_four_directions_and_scales_110_to_118_percent() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!("SKIP: ffmpeg and ffprobe are runtime requirements for motion acceptance");
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    generate_motion_source(&dir);
    let second = TIMEBASE;
    let directions = [
        ("right", [-0.12, 0.0, 1.0, 1.0], [0.12, 0.0, 1.0, 1.0]),
        ("left", [0.12, 0.0, 1.0, 1.0], [-0.12, 0.0, 1.0, 1.0]),
        ("up", [0.0, 0.12, 1.0, 1.0], [0.0, -0.12, 1.0, 1.0]),
        ("down", [0.0, -0.12, 1.0, 1.0], [0.0, 0.12, 1.0, 1.0]),
    ];
    let clips = directions
        .iter()
        .enumerate()
        .map(|(index, (_, from, to))| {
            json!({
                "id":format!("direction-{index}"),"track_id":"v","asset_id":"grid","kind":"image",
                "start_tick":index as u64*second,"duration_ticks":second,"source_in_tick":0,
                "motion":motion_keyframes(second,0,*from,*to,"linear")
            })
        })
        .collect();
    let pan_output = render_from_zero(
        &motion_project(clips),
        &dir,
        "four-directions.mp4",
        4 * second,
    );
    let pan_frames = decode_all_frames(&pan_output, 160, 90);
    assert_eq!(pan_frames.len(), 32);
    for (index, (name, from, to)) in directions.iter().enumerate() {
        let start = bright_bounds(&pan_frames[index * 8], 160, 210);
        let end = bright_bounds(&pan_frames[index * 8 + 7], 160, 210);
        let delta_x = end.0 - start.0;
        let delta_y = end.1 - start.1;
        match *name {
            "right" => assert!(
                delta_x > 25.0 && delta_y.abs() < 2.0,
                "right delta {delta_x},{delta_y}"
            ),
            "left" => assert!(
                delta_x < -25.0 && delta_y.abs() < 2.0,
                "left delta {delta_x},{delta_y}"
            ),
            "up" => assert!(
                delta_y < -17.0 && delta_x.abs() < 2.0,
                "up delta {delta_x},{delta_y}"
            ),
            "down" => assert!(
                delta_y > 17.0 && delta_x.abs() < 2.0,
                "down delta {delta_x},{delta_y}"
            ),
            _ => unreachable!(),
        }
        assert!((start.0 - (80.0 + from[0] * 160.0)).abs() < 3.0);
        assert!((end.0 - (80.0 + to[0] * 160.0)).abs() < 3.0);
        assert!((start.1 - (45.0 + from[1] * 90.0)).abs() < 3.0);
        assert!((end.1 - (45.0 + to[1] * 90.0)).abs() < 3.0);
        let middle = bright_bounds(&pan_frames[index * 8 + 4], 160, 210);
        let expected_middle_x = start.0 + (end.0 - start.0) * (4.0 / 7.0);
        let expected_middle_y = start.1 + (end.1 - start.1) * (4.0 / 7.0);
        assert!(
            (middle.0 - expected_middle_x).abs() < 2.0
                && (middle.1 - expected_middle_y).abs() < 2.0,
            "{name} midpoint {middle:?} did not match expected {expected_middle_x},{expected_middle_y} between {start:?} and {end:?}"
        );
    }

    let zoom_clip = json!({
        "id":"zoom","track_id":"v","asset_id":"grid","kind":"image",
        "start_tick":0,"duration_ticks":second,"source_in_tick":0,
        "motion":motion_keyframes(second,0,[0.0,0.0,1.10,1.0],[0.0,0.0,1.18,1.0],"linear")
    });
    let zoom_output = render_from_zero(
        &motion_project(vec![zoom_clip]),
        &dir,
        "zoom-110-to-118.mp4",
        second,
    );
    let zoom_frames = decode_all_frames(&zoom_output, 160, 90);
    let start = bright_bounds(&zoom_frames[0], 160, 210);
    let middle = bright_bounds(&zoom_frames[4], 160, 210);
    let end = bright_bounds(&zoom_frames[7], 160, 210);
    assert!(
        (start.2 as f64 / 48.0 - 1.10).abs() < 0.10,
        "start width {}",
        start.2
    );
    assert!(
        (middle.2 as f64 / 48.0 - 1.14).abs() < 0.10,
        "middle width {}",
        middle.2
    );
    assert!(
        (end.2 as f64 / 48.0 - 1.18).abs() < 0.10,
        "end width {}",
        end.2
    );
    assert!(
        end.2 >= start.2 + 3,
        "zoom marker width did not grow: {} -> {}",
        start.2,
        end.2
    );
    eprintln!("REAL_MOTION_OUTPUT_DIR={}", dir.display());
}

#[test]
fn render_rejects_oversized_static_motion_layer_before_publishing_output() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!("SKIP: ffmpeg and ffprobe are runtime requirements for layer budget acceptance");
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    generate_motion_source(&dir);
    let duration = TIMEBASE;
    let clip = json!({
        "id":"oversized","track_id":"v","asset_id":"grid","kind":"image",
        "start_tick":0,"duration_ticks":duration,"source_in_tick":0,
        "motion":motion_keyframes(duration,0,[0.0,0.0,16.0,1.0],[0.0,0.0,16.0,1.0],"linear")
    });
    let mut project = motion_project(vec![clip]);
    project["canvas"]["width"] = json!(1920);
    project["canvas"]["height"] = json!(1080);
    let output = dir.join("must-not-render.mp4");
    let error = render(
        &project,
        &dir.join("project.storycut.json"),
        &output,
        &json!({
            "range_start_tick":0,
            "range_end_tick":duration,
            "subtitle_mode":"none",
            "encoder":"h264_cpu",
            "overwrite":false
        }),
    )
    .expect_err("oversized motion layer must be rejected before invoking the renderer");
    assert_eq!(error.code(), "UNSUPPORTED_FEATURE");
    assert!(error.to_string().contains("per-layer render limit"));
    assert!(
        !output.exists(),
        "renderer must not publish output after rejecting the layer"
    );
}

#[test]
fn real_ffmpeg_split_clip_samples_the_same_motion_domain_without_a_jump() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!("SKIP: ffmpeg and ffprobe are runtime requirements for split motion acceptance");
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    generate_motion_source(&dir);
    let second = TIMEBASE;
    let frame = TIMEBASE / 8;
    let split = 4 * frame;
    let from = [-0.04, 0.03, 1.10, 0.70];
    let to = [0.04, -0.03, 1.18, 1.0];
    let full = json!({
        "id":"full","track_id":"v","asset_id":"grid","kind":"image",
        "start_tick":0,"duration_ticks":second,"source_in_tick":0,
        "motion":motion_keyframes(second,0,from,to,"smoothstep")
    });
    let full_project = motion_project(vec![full]);
    let full_output = render_from_zero(&full_project, &dir, "full-motion.mp4", second);

    let left = json!({
        "id":"left","track_id":"v","asset_id":"grid","kind":"image",
        "start_tick":0,"duration_ticks":split,"source_in_tick":0,
        "motion":motion_keyframes(second,0,from,to,"smoothstep")
    });
    let right = json!({
        "id":"right","track_id":"v","asset_id":"grid","kind":"image",
        "start_tick":split,"duration_ticks":second-split,"source_in_tick":0,
        "motion":motion_keyframes(second,split,from,to,"smoothstep")
    });
    let split_project = motion_project(vec![left, right]);
    let split_output = render_from_zero(&split_project, &dir, "split-motion.mp4", second);

    let full_frames = decode_all_frames(&full_output, 160, 90);
    let split_frames = decode_all_frames(&split_output, 160, 90);
    assert_eq!(full_frames.len(), 8);
    assert_eq!(split_frames.len(), full_frames.len());
    for (index, (full, split)) in full_frames.iter().zip(&split_frames).enumerate() {
        let max_delta = full
            .iter()
            .zip(split)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        assert!(
            max_delta <= 1,
            "split changed frame {index}; max RGB delta {max_delta}"
        );
    }
    let (start, _, start_width, _) = bright_bounds(&split_frames[0], 160, 165);
    let (end, _, end_width, _) = bright_bounds(&split_frames[7], 160, 220);
    assert!(
        end > start,
        "split motion must continue its x position through the cut"
    );
    assert!(
        end_width >= start_width + 2,
        "opacity/zoom should continue through split"
    );
    eprintln!("REAL_SPLIT_MOTION_OUTPUT_DIR={}", dir.display());
}

#[test]
fn ffmpeg_renders_real_layers_audio_offset_and_srt_then_verifies_output() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!("SKIP: ffmpeg and ffprobe are runtime requirements for real_render");
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    generate_sources(&dir);
    let project = project();
    let source_probe =
        probe_media(&dir.join("blue_with_tone.mp4")).expect("probe generated source");
    assert_eq!(
        serde_json::to_value(&source_probe).unwrap()["kind"],
        "video"
    );
    assert_eq!(source_probe.streams.len(), 2);
    assert_eq!(source_probe.streams[0].kind, "video");
    assert_eq!(source_probe.streams[1].kind, "audio");
    let logo_probe =
        probe_media(&dir.join("half_alpha_red.png")).expect("probe transparent image source");
    assert_eq!(serde_json::to_value(&logo_probe).unwrap()["kind"], "image");
    assert_eq!(
        logo_probe.duration_ticks, None,
        "image demuxer nominal duration is not clip duration"
    );

    let clean = render_one(&project, &dir, "clean.mp4", "none");
    let burned = render_one(&project, &dir, "burned.mp4", "burn");
    assert_eq!(clean.frame_count, 16);
    assert_eq!(burned.frame_count, 16);
    assert_eq!(clean.width, 160);
    assert_eq!(clean.height, 90);
    assert_eq!(clean.planned_duration_ticks, 2 * TIMEBASE);
    assert_eq!(burned.subtitle_mode, "burn");
    assert!(
        burned
            .verified_streams
            .iter()
            .any(|s| s.starts_with("video:"))
    );
    assert!(
        burned
            .verified_streams
            .iter()
            .any(|s| s.starts_with("audio:"))
    );
    assert!(clean.duration_ticks.abs_diff(2 * TIMEBASE) <= TIMEBASE / 8);
    assert!(clean.audio_sample_count.abs_diff(96_000) <= 1_024);

    let preview = render_frame(
        &project,
        &dir.join("project.storycut.json"),
        &dir.join("preview.png"),
        10 * (TIMEBASE / 8),
        80,
        46,
        true,
    )
    .expect("render one composed preview frame");
    assert_eq!(preview.decoded_frame_count, 1);
    assert_eq!((preview.width, preview.height), (80, 46));
    let preview_probe = probe_media(&preview.output).expect("probe PNG preview");
    assert_eq!(preview_probe.width, Some(80));
    assert_eq!(preview_probe.height, Some(46));

    let crossfade = render(
        &crossfade_project(),
        &dir.join("project.storycut.json"),
        &dir.join("crossfade.mp4"),
        &json!({
            "range_start_tick":0,
            "range_end_tick":3*TIMEBASE/2,
            "subtitle_mode":"none",
            "encoder":"h264_cpu",
            "overwrite":false
        }),
    )
    .expect("render declared cross dissolve");
    assert_eq!(crossfade.frame_count, 12);
    let transition_frame = decode_frame(&crossfade.output, "0.75");
    let center = (45 * 160 + 80) * 3;
    assert!(
        transition_frame[center] > 65 && transition_frame[center + 2] > 65,
        "cross dissolve frame should contain both blue and red, rgb={:?}",
        &transition_frame[center..center + 3]
    );

    let clean_frame = decode_frame(&clean.output, "1.25");
    let burned_frame = decode_frame(&burned.output, "1.25");
    assert_eq!(clean_frame.len(), burned_frame.len());
    let changed_pixels = clean_frame
        .chunks_exact(3)
        .zip(burned_frame.chunks_exact(3))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 18))
        .count();
    assert!(
        changed_pixels > 100,
        "subtitle burn changed only {changed_pixels} pixels"
    );

    let pcm = decode_audio(&clean.output);
    assert!(
        rms(&pcm, 0, 18_000) < 0.015,
        "audio begins before its 0.5s timeline offset"
    );
    assert!(
        rms(&pcm, 27_000, 33_000) > 0.02,
        "delayed audio is missing at 0.6s: rms={}",
        rms(&pcm, 27_000, 33_000)
    );

    let duplicate = render(
        &project, &dir.join("project.storycut.json"), &clean.output,
        &json!({"range_start_tick":0,"range_end_tick":2*TIMEBASE,"subtitle_mode":"none","encoder":"h264_cpu"}),
    ).unwrap_err();
    assert_eq!(duplicate.code(), "OUTPUT_EXISTS");

    let media_report = json!({
        "source_probe":source_probe,
        "clean":clean,
        "burned":burned,
        "preview":preview,
        "crossfade":crossfade,
        "decoded_subtitle_pixel_delta_count":changed_pixels,
        "audio_rms_before_0_5_seconds":rms(&pcm,0,18_000),
        "audio_rms_at_0_6_seconds":rms(&pcm,27_000,33_000),
        "ffmpeg_source_command":"blue color video + 440 Hz sine; translucent red PNG logo and separate blue/red cross-dissolve stills"
    });
    fs::write(
        dir.join("verification.json"),
        serde_json::to_vec_pretty(&media_report).unwrap(),
    )
    .expect("write renderer acceptance evidence");
    eprintln!("REAL_RENDER_OUTPUT_DIR={}", dir.display());
    assert!(dir.join("clean.mp4").is_file());
    assert!(dir.join("burned.mp4").is_file());
}

#[test]
fn range_preview_ignores_missing_media_outside_effective_inputs() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!("SKIP: ffmpeg and ffprobe are runtime requirements for range preview acceptance");
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    generate_sources(&dir);
    let mut value = project();
    let add_missing_asset = |value: &mut Value, id: &str, kind: &str| {
        value["assets"].as_array_mut().unwrap().push(json!({
            "id":id,"kind":kind,"path":format!("missing-{id}.{}", if kind == "image" {"png"} else {"wav"}),
            "probe_status":"unprobed","duration_ticks":null,"sha256":null,"streams":[]
        }));
    };
    add_missing_asset(&mut value, "unreferenced", "image");
    add_missing_asset(&mut value, "future", "image");
    add_missing_asset(&mut value, "disabled", "image");
    add_missing_asset(&mut value, "transparent", "image");
    add_missing_asset(&mut value, "muted", "audio");
    add_missing_asset(&mut value, "not-solo", "audio");

    let second = TIMEBASE;
    value["tracks"].as_array_mut().unwrap().extend([
        json!({"id":"disabled-visual","kind":"image","enabled":false}),
        json!({"id":"transparent-visual","kind":"image","enabled":true}),
        json!({"id":"muted-audio","kind":"audio","enabled":true,"muted":true}),
        json!({"id":"not-solo-audio","kind":"audio","enabled":true,"muted":false,"solo":false}),
    ]);
    value["tracks"][2]["solo"] = Value::Bool(true);

    let motion = json!({
        "domain_duration_ticks":second,"sample_offset_tick":0,"fit":"cover",
        "avoid_exposed_edges":false,"anchor":{"x":0.5,"y":0.5},"interpolation":"linear",
        "keyframes":[
            {"tick":0,"x":0,"y":0,"scale":1,"opacity":0},
            {"tick":second-TIMEBASE/8,"x":0,"y":0,"scale":1,"opacity":0}
        ]
    });
    value["clips"].as_array_mut().unwrap().extend([
        json!({"id":"future-clip","track_id":"base","asset_id":"future","kind":"image",
            "start_tick":2*second,"duration_ticks":second,"source_in_tick":0}),
        json!({"id":"disabled-clip","track_id":"disabled-visual","asset_id":"disabled","kind":"image",
            "start_tick":0,"duration_ticks":second,"source_in_tick":0}),
        json!({"id":"transparent-clip","track_id":"transparent-visual","asset_id":"transparent","kind":"image",
            "start_tick":0,"duration_ticks":second,"source_in_tick":0,"motion":motion}),
        json!({"id":"muted-clip","track_id":"muted-audio","asset_id":"muted","kind":"audio",
            "start_tick":0,"duration_ticks":second,"source_in_tick":0,"stream_index":0,
            "audio":{"domain_duration_ticks":second,"sample_offset_tick":0,"gain_db":0,"pan":0,
                "muted":false,"fade_in_ticks":0,"fade_out_ticks":0}}),
        json!({"id":"not-solo-clip","track_id":"not-solo-audio","asset_id":"not-solo","kind":"audio",
            "start_tick":0,"duration_ticks":second,"source_in_tick":0,"stream_index":0,
            "audio":{"domain_duration_ticks":second,"sample_offset_tick":0,"gain_db":0,"pan":0,
                "muted":false,"fade_in_ticks":0,"fade_out_ticks":0}}),
    ]);

    let preview = render_frame(
        &value,
        &dir.join("project.storycut.json"),
        &dir.join("effective-inputs.png"),
        TIMEBASE / 8,
        80,
        46,
        false,
    )
    .expect("missing media that cannot contribute to this frame must not block preview");
    assert_eq!(preview.decoded_frame_count, 1);
}

#[test]
fn real_ffmpeg_renders_a_large_multitrack_filter_graph_and_cleans_its_script() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!("SKIP: ffmpeg and ffprobe are runtime requirements for large graph acceptance");
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    let image_dir = dir.join("media");
    fs::create_dir_all(&image_dir).expect("create image fixture directory");
    let base_image = image_dir.join("source.png");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "color=c=blue:s=32x18:d=1",
        "-frames:v",
        "1",
        base_image.to_str().unwrap(),
    ])
    .expect("generate large graph source image");
    let music = dir.join("music.wav");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=1",
        "-c:a",
        "pcm_s16le",
        music.to_str().unwrap(),
    ])
    .expect("generate repeated music source");

    let frame_ticks = TIMEBASE / 240;
    let image_assets: Vec<_> = (0..72)
        .map(|index| {
            let name = format!("shot-{index:02}.png");
            fs::copy(&base_image, image_dir.join(&name)).expect("copy image fixture");
            json!({
                "id":format!("image-{index}"),"kind":"image",
                "path":format!("media/{name}"),"probe_status":"unprobed",
                "duration_ticks":null,"sha256":null,"streams":[]
            })
        })
        .collect();
    let visual_clips: Vec<_> = (0..72)
        .map(|index| json!({
            "id":format!("visual-{index}"),"track_id":"visual","asset_id":format!("image-{index}"),
            "kind":"image","start_tick":index*frame_ticks,"duration_ticks":frame_ticks,"source_in_tick":0
        }))
        .collect();
    let audio_clips: Vec<_> = (0..104)
        .map(|index| {
            json!({
                "id":format!("music-{index}"),"track_id":"music","asset_id":"music",
                "kind":"audio","start_tick":index*frame_ticks,"duration_ticks":frame_ticks,
                "source_in_tick":0,"stream_index":0,
                "audio":{"domain_duration_ticks":frame_ticks,"sample_offset_tick":0,
                    "gain_db":-12,"pan":0,"muted":false,"fade_in_ticks":0,"fade_out_ticks":0}
            })
        })
        .collect();
    let mut clips = visual_clips;
    clips.extend(audio_clips);
    let mut assets = image_assets;
    assets.push(json!({
        "id":"music","kind":"audio","path":"music.wav","probe_status":"unprobed",
        "duration_ticks":null,"sha256":null,"streams":[]
    }));
    let value = json!({
        "schema_version":"0.2.0-draft","project_id":"large-filter-graph","revision":1,
        "name":"Large filter graph acceptance","timebase":TIMEBASE,
        "canvas":{"width":160,"height":90,"fps":{"num":240,"den":1},
            "background":"#000000","color_mode":"sdr_bt709"},
        "audio_sample_rate":48000,"notes":[],"assets":assets,
        "tracks":[
            {"id":"visual","name":"Visual","kind":"image","enabled":true},
            {"id":"music","name":"Music","kind":"audio","enabled":true}
        ],
        "clips":clips,"transitions":[],"links":[],"subtitles":[]
    });
    let output = dir.join("large-filter-graph.mp4");
    let report = render(
        &value,
        &dir.join("project.storycut.json"),
        &output,
        &json!({
            "range_start_tick":0,"range_end_tick":104*frame_ticks,
            "subtitle_mode":"none","encoder":"h264_cpu","overwrite":false
        }),
    )
    .expect("FFmpeg should render 72 visual and 104 repeated audio clip inputs");
    assert_eq!(report.frame_count, 104);
    assert_eq!(report.planned_audio_sample_count, 20_800);
    assert!(output.is_file());
    assert!(
        fs::read_dir(&dir)
            .expect("read fixture output directory")
            .all(|entry| !entry
                .expect("read fixture directory entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".filtergraph")),
        "temporary filter graph must be removed after render"
    );
}

#[test]
fn audio_fade_curve_uses_clip_local_offset_for_partial_render_ranges() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!(
            "SKIP: ffmpeg and ffprobe are runtime requirements for audio range fade acceptance"
        );
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    let tone = dir.join("fade-tone.wav");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=5",
        "-c:a",
        "pcm_s16le",
        tone.to_str().unwrap(),
    ])
    .expect("generate five-second tone source");

    let duration = 5 * TIMEBASE;
    let project = audio_fade_project(duration, 2 * TIMEBASE, TIMEBASE);
    let full_output = render_from_zero(&project, &dir, "audio-full.mp4", duration);
    let range_output = dir.join("audio-range-1-to-3.mp4");
    render(
        &project,
        &dir.join("project.storycut.json"),
        &range_output,
        &json!({
            "range_start_tick":TIMEBASE,
            "range_end_tick":3*TIMEBASE,
            "subtitle_mode":"none",
            "encoder":"h264_cpu",
            "overwrite":false
        }),
    )
    .expect("render clipped range from inside the audio clip");

    let full_pcm = decode_audio(&full_output);
    let range_pcm = decode_audio(&range_output);
    assert!(full_pcm.len() >= 5 * 48_000 * 2);
    assert!(range_pcm.len() >= 2 * 48_000 * 2);
    for range_start in [7_200, 19_200, 31_200, 43_200, 55_200, 67_200] {
        let full_start = range_start + 48_000;
        let ranged = rms(&range_pcm, range_start, range_start + 9_600);
        let full = rms(&full_pcm, full_start, full_start + 9_600);
        assert!(
            (ranged - full).abs() < 0.025,
            "partial range RMS {ranged:.4} differs from full render overlap RMS {full:.4} at local sample {range_start}"
        );
    }
    eprintln!("REAL_AUDIO_RANGE_FADE_OUTPUT_DIR={}", dir.display());
}

#[test]
fn full_domain_audio_fade_out_remains_audible_until_clip_end() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!("SKIP: ffmpeg and ffprobe are runtime requirements for audio fade acceptance");
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    let tone = dir.join("fade-tone.wav");
    ffmpeg(&[
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=1",
        "-c:a",
        "pcm_s16le",
        tone.to_str().unwrap(),
    ])
    .expect("generate one-second fade source tone");

    let project = audio_fade_project(TIMEBASE, 0, TIMEBASE);
    let output = render_from_zero(&project, &dir, "audio-full-fade-out.mp4", TIMEBASE);
    let pcm = decode_audio(&output);
    assert!(
        pcm.len() >= 48_000 * 2,
        "decoded audio must cover the one-second clip"
    );

    let middle = rms(&pcm, 19_200, 28_800);
    let tail = rms(&pcm, 40_800, 45_600);
    assert!(
        middle > 0.02,
        "full-domain fade-out must retain audible signal mid-clip; RMS was {middle:.5}"
    );
    assert!(
        tail > 0.002 && tail < middle * 0.35,
        "fade-out must remain audible before reaching silence at the end; middle RMS {middle:.5}, tail RMS {tail:.5}"
    );
}

#[test]
fn render_rejects_media_outside_the_project_parent() {
    let (dir, _temp) = unique_artifact_dir();
    let project_dir = dir.join("workspace/projects/nested");
    // It need not be valid media: an escaping canonical path must be rejected
    // before ffprobe is invoked on it. The project JSON is intentionally absent
    // because the renderer receives an in-memory value; its parent must exist
    // so root canonicalization can reach the path authorization check.
    fs::create_dir_all(&project_dir).unwrap();
    fs::write(dir.join("outside.png"), b"outside project root").unwrap();
    let mut value = crossfade_project();
    value["assets"].as_array_mut().unwrap().truncate(1);
    value["clips"].as_array_mut().unwrap().truncate(1);
    value["transitions"] = json!([]);
    value["assets"][0]["path"] = Value::String("../../../outside.png".into());

    let error = render(
        &value,
        &project_dir.join("project.storycut.json"),
        &dir.join("must-not-exist.mp4"),
        &json!({
            "range_start_tick":0,
            "range_end_tick":3*TIMEBASE/2,
            "subtitle_mode":"none",
            "encoder":"h264_cpu",
            "overwrite":false
        }),
    )
    .expect_err("direct renderer call must not read outside the project parent");

    assert_eq!(error.code(), "INVALID_PROJECT", "{error}");
    assert!(
        error
            .to_string()
            .contains("outside the authorized media root")
    );
    assert!(!dir.join("must-not-exist.mp4").exists());
}

#[test]
fn explicit_workspace_root_allows_nested_project_media_for_render_and_preview() {
    if Command::new("ffmpeg").arg("-version").output().is_err()
        || Command::new("ffprobe").arg("-version").output().is_err()
    {
        eprintln!("SKIP: ffmpeg and ffprobe are runtime requirements for root authorization test");
        return;
    }
    let (dir, _temp) = unique_artifact_dir();
    let workspace = dir.join("workspace");
    let project_dir = workspace.join("projects/nested");
    fs::create_dir_all(&project_dir).expect("create nested project directory");
    generate_sources(&workspace);
    let mut value = project();
    value["assets"][0]["path"] = Value::String("../../blue_with_tone.mp4".into());
    value["assets"][1]["path"] = Value::String("../../half_alpha_red.png".into());
    let project_path = project_dir.join("project.storycut.json");

    let default_error = render(
        &value,
        &project_path,
        &project_dir.join("default-denied.mp4"),
        &json!({
            "range_start_tick":0,
            "range_end_tick":2*TIMEBASE,
            "subtitle_mode":"none",
            "encoder":"h264_cpu"
        }),
    )
    .expect_err("conservative API must limit sources to project parent");
    assert_eq!(default_error.code(), "INVALID_PROJECT");

    let rendered = render_with_root(
        &value,
        &project_path,
        &workspace,
        &project_dir.join("authorized.mp4"),
        &json!({
            "range_start_tick":0,
            "range_end_tick":2*TIMEBASE,
            "subtitle_mode":"none",
            "encoder":"h264_cpu"
        }),
    )
    .expect("authorized workspace root should allow sibling media");
    assert_eq!(rendered.frame_count, 16);

    let preview = render_frame_with_root(
        &value,
        &project_path,
        &workspace,
        &project_dir.join("authorized-preview.png"),
        TIMEBASE,
        80,
        46,
        false,
    )
    .expect("preview should use the same authorized workspace root");
    assert_eq!(preview.decoded_frame_count, 1);
}
