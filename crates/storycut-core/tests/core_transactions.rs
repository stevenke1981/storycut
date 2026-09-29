use std::path::PathBuf;

use serde_json::{Value, json};
use storycut_core::{
    Asset, AssetKind, AudioClip, AudioSettings, Canvas, Cue, FitMode, Interpolation, Keyframe,
    Link, Motion, Operation, ProbeStatus, ProjectStore, Rational, Stream, StreamKind, Subtitle,
    SubtitleFormat, TIMEBASE, Track, TrackChanges, TrackKind, Transition, TransitionKind,
    VideoAudioPolicy, VideoClip,
};
use tempfile::TempDir;

fn frame_ticks() -> u64 {
    TIMEBASE / 30
}

fn image_asset(id: &str, duration_ticks: u64) -> Asset {
    Asset {
        id: id.to_owned(),
        kind: AssetKind::Image,
        path: "fixtures/still.png".to_owned(),
        probe_status: ProbeStatus::Probed,
        duration_ticks: Some(duration_ticks),
        sha256: None,
        streams: Vec::new(),
    }
}

fn motion(duration_ticks: u64) -> Motion {
    motion_for(duration_ticks, frame_ticks())
}

fn motion_for(duration_ticks: u64, frame: u64) -> Motion {
    let last_tick = duration_ticks - frame;
    let first = Keyframe {
        tick: 0,
        x: 0.0,
        y: 0.0,
        scale: 1.0,
        opacity: 1.0,
    };
    let last = Keyframe {
        tick: last_tick,
        x: 0.2,
        y: -0.1,
        scale: 1.2,
        opacity: 1.0,
    };
    Motion {
        domain_duration_ticks: duration_ticks,
        sample_offset_tick: 0,
        fit: FitMode::Contain,
        avoid_exposed_edges: false,
        anchor: storycut_core::Anchor { x: 0.5, y: 0.5 },
        interpolation: Interpolation::Smoothstep,
        keyframes: if last_tick == 0 {
            vec![first]
        } else {
            vec![first, last]
        },
    }
}

fn new_store(temp: &TempDir) -> (ProjectStore, PathBuf) {
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let path = workspace.join("draft.storycut.json");
    let store = ProjectStore::create(
        &workspace,
        &path,
        "test",
        Canvas::default(),
        48_000,
        "create-key",
    )
    .unwrap();
    (store, path)
}

fn ntsc_store(temp: &TempDir) -> (ProjectStore, PathBuf) {
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let path = workspace.join("draft.storycut.json");
    let canvas = Canvas {
        fps: Rational {
            num: 30_000,
            den: 1_001,
        },
        ..Canvas::default()
    };
    let store =
        ProjectStore::create(&workspace, &path, "test", canvas, 48_000, "create-key").unwrap();
    (store, path)
}

fn clip_value(duration_ticks: u64) -> Value {
    json!({
        "kind":"image", "id":"still-1", "track_id":"video-1", "asset_id":"still-asset",
        "start_tick":0, "duration_ticks":duration_ticks, "source_in_tick":0,
        "motion":motion(duration_ticks)
    })
}

#[test]
fn project_create_import_timeline_split_undo_redo_and_reopen_are_durable() {
    let temp = TempDir::new().unwrap();
    let (store, project_path) = new_store(&temp);

    // A still-image probe duration is not a playback limit for an image clip.
    let imported = store
        .import_assets(
            0,
            "media-1",
            false,
            vec![image_asset("still-asset", frame_ticks())],
        )
        .unwrap();
    assert_eq!(imported.revision, 1);

    let clip_duration = frame_ticks() * 300;
    let operations = vec![
        serde_json::from_value::<Operation>(json!({
            "op":"track.add", "track":Track::new("video-1", "V1", TrackKind::Video)
        }))
        .unwrap(),
        serde_json::from_value::<Operation>(
            json!({"op":"clip.add", "clip":clip_value(clip_duration)}),
        )
        .unwrap(),
    ];
    let dry = store
        .apply(1, "timeline-1", true, operations.clone())
        .unwrap();
    assert!(!dry.applied);
    assert_eq!(dry.revision, 1);
    assert_eq!(store.snapshot().unwrap().clips.len(), 0);

    let applied = store
        .apply(1, "timeline-1", false, operations.clone())
        .unwrap();
    assert!(applied.applied);
    assert_eq!(applied.revision, 2);
    assert_eq!(applied.duration_ticks, clip_duration);
    assert_eq!(
        store.apply(0, "timeline-1", false, operations).unwrap(),
        applied
    );

    let split_tick = frame_ticks() * 150;
    let split = store
        .apply(
            2,
            "split-1",
            false,
            vec![Operation::ClipSplit {
                clip_id: "still-1".to_owned(),
                at_tick: split_tick,
                respect_links: true,
            }],
        )
        .unwrap();
    assert_eq!(split.revision, 3);
    let project = store.snapshot().unwrap();
    assert_eq!(project.clips.len(), 2);
    let right = project
        .clips
        .iter()
        .find(|clip| clip.start_tick() == split_tick)
        .unwrap();
    assert_eq!(right.motion().unwrap().sample_offset_tick, split_tick);
    assert_eq!(right.motion().unwrap().domain_duration_ticks, clip_duration);

    assert_eq!(store.undo(3, "undo-1", false).unwrap().revision, 4);
    assert_eq!(store.snapshot().unwrap().clips.len(), 1);
    assert_eq!(store.redo(4, "redo-1", false).unwrap().revision, 5);
    assert_eq!(store.snapshot().unwrap().clips.len(), 2);

    let project_id = store.snapshot().unwrap().project_id;
    assert_eq!(store.save(5, "save-1").unwrap().revision, 5);
    assert_eq!(store.save(0, "save-1").unwrap().revision, 5);
    let loaded = ProjectStore::load(project_path.parent().unwrap(), &project_id).unwrap();
    assert_eq!(loaded.snapshot().unwrap().revision, 5);
    let opened = ProjectStore::open(&project_path).unwrap();
    assert_eq!(opened.snapshot().unwrap().project_id, project_id);
    let exported: storycut_core::Project =
        serde_json::from_slice(&std::fs::read(&project_path).unwrap()).unwrap();
    assert_eq!(exported.revision, 5);

    // Same create key and payload returns the existing project after restart.
    let retried = ProjectStore::create(
        project_path.parent().unwrap(),
        &project_path,
        "test",
        Canvas::default(),
        48_000,
        "create-key",
    )
    .unwrap();
    assert_eq!(retried.snapshot().unwrap().project_id, project_id);
}

#[test]
fn committed_state_survives_project_json_export_failure_and_retry_repairs_projection() {
    let temp = TempDir::new().unwrap();
    let (store, project_path) = new_store(&temp);
    std::fs::remove_file(&project_path).unwrap();
    // An empty directory at the exchange-file path deterministically prevents
    // atomic rename while leaving the canonical sidecar writable.
    std::fs::create_dir(&project_path).unwrap();

    let operation = Operation::TrackAdd {
        track: Track::new("video-1", "V1", TrackKind::Video),
    };
    let committed = store
        .apply(0, "projection-failure", false, vec![operation.clone()])
        .unwrap();
    assert!(committed.applied);
    assert_eq!(committed.revision, 1);
    assert!(
        committed
            .projection_warning
            .as_deref()
            .is_some_and(|warning| warning.contains("revision 1 committed"))
    );

    let persisted: Value =
        serde_json::from_slice(&std::fs::read(store.state_path()).unwrap()).unwrap();
    assert_eq!(persisted["project"]["revision"], 1);
    assert_eq!(persisted["project"]["tracks"][0]["id"], "video-1");

    let retry_while_blocked = store
        .apply(0, "projection-failure", false, vec![operation.clone()])
        .unwrap();
    assert_eq!(retry_while_blocked.revision, 1);
    assert!(retry_while_blocked.projection_warning.is_some());

    std::fs::remove_dir(&project_path).unwrap();
    let retry_after_recovery = store
        .apply(0, "projection-failure", false, vec![operation])
        .unwrap();
    assert_eq!(retry_after_recovery.revision, 1);
    assert_eq!(retry_after_recovery.projection_warning, None);

    let exported: storycut_core::Project =
        serde_json::from_slice(&std::fs::read(&project_path).unwrap()).unwrap();
    assert_eq!(exported.revision, 1);
    assert_eq!(exported.tracks.len(), 1);
}

#[test]
fn older_state_without_undo_stacks_loads_without_rewriting_sidecar() {
    let temp = TempDir::new().unwrap();
    let (store, project_path) = new_store(&temp);
    let project_id = store.snapshot().unwrap().project_id;
    let state_path = store.state_path().to_path_buf();
    let mut state: Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    state.as_object_mut().unwrap().remove("undo_stack");
    state.as_object_mut().unwrap().remove("redo_stack");
    let old_bytes = serde_json::to_vec_pretty(&state).unwrap();
    std::fs::write(&state_path, &old_bytes).unwrap();

    let loaded = ProjectStore::load(project_path.parent().unwrap(), &project_id).unwrap();
    assert_eq!(loaded.snapshot().unwrap().project_id, project_id);
    assert_eq!(std::fs::read(&state_path).unwrap(), old_bytes);
}

#[test]
fn failed_batch_stays_atomic_and_stale_revisions_are_rejected_after_idempotency() {
    let temp = TempDir::new().unwrap();
    let (store, _) = new_store(&temp);
    store
        .import_assets(
            0,
            "media-1",
            false,
            vec![image_asset("still-asset", frame_ticks())],
        )
        .unwrap();
    let before = store.snapshot().unwrap();
    let invalid = vec![
        Operation::TrackAdd { track: Track::new("video-1", "V1", TrackKind::Video) },
        serde_json::from_value::<Operation>(json!({"op":"clip.add", "clip":{
            "kind":"image", "id":"broken", "track_id":"video-1", "asset_id":"missing",
            "start_tick":0, "duration_ticks":frame_ticks(), "source_in_tick":0, "motion":motion(frame_ticks())
        }})).unwrap(),
    ];
    assert_eq!(
        store
            .apply(1, "invalid-batch", false, invalid)
            .unwrap_err()
            .code(),
        "NOT_FOUND"
    );
    assert_eq!(store.snapshot().unwrap(), before);

    let conflict = store
        .apply(
            0,
            "new-key",
            false,
            vec![Operation::TrackAdd {
                track: Track::new("video-1", "V1", TrackKind::Video),
            }],
        )
        .unwrap_err();
    assert_eq!(conflict.code(), "REVISION_CONFLICT");
}

#[test]
fn initially_locked_tracks_cannot_be_unlocked_and_used_inside_one_batch() {
    let temp = TempDir::new().unwrap();
    let (store, _) = new_store(&temp);
    store
        .import_assets(
            0,
            "media-1",
            false,
            vec![image_asset("still-asset", frame_ticks())],
        )
        .unwrap();
    store
        .apply(
            1,
            "create-locked-track",
            false,
            vec![Operation::TrackAdd {
                track: Track {
                    locked: true,
                    ..Track::new("video-1", "V1", TrackKind::Video)
                },
            }],
        )
        .unwrap();
    let operations = vec![
        Operation::TrackUpdate {
            track_id: "video-1".into(),
            changes: TrackChanges {
                locked: Some(false),
                ..Default::default()
            },
        },
        Operation::ClipAdd {
            clip: serde_json::from_value(clip_value(frame_ticks())).unwrap(),
        },
    ];
    assert_eq!(
        store
            .apply(2, "unlock-and-edit", false, operations)
            .unwrap_err()
            .code(),
        "LOCKED_TRACK"
    );
    let project = store.snapshot().unwrap();
    assert_eq!(project.revision, 2);
    assert!(project.tracks[0].locked);
    assert!(project.clips.is_empty());
}

#[test]
fn image_trim_preserves_the_original_motion_domain_and_clip_remove_is_lift() {
    let temp = TempDir::new().unwrap();
    let (store, _) = new_store(&temp);
    store
        .import_assets(
            0,
            "media-1",
            false,
            vec![image_asset("still-asset", frame_ticks())],
        )
        .unwrap();
    let duration = frame_ticks() * 10;
    let operations = vec![
        Operation::TrackAdd {
            track: Track::new("video-1", "V1", TrackKind::Video),
        },
        Operation::ClipAdd {
            clip: serde_json::from_value(clip_value(duration)).unwrap(),
        },
    ];
    store.apply(1, "add-image", false, operations).unwrap();
    let trimmed_start = frame_ticks() * 5;
    store
        .apply(
            2,
            "trim-image",
            false,
            vec![Operation::ClipTrim {
                clip_id: "still-1".into(),
                start_tick: trimmed_start,
                source_in_tick: 0,
                duration_ticks: duration - trimmed_start,
                keyframe_policy: storycut_core::KeyframePolicy::ResampleVisible,
                respect_links: true,
            }],
        )
        .unwrap();
    let project = store.snapshot().unwrap();
    assert_eq!(project.clips[0].start_tick(), trimmed_start);
    assert_eq!(
        project.clips[0].motion().unwrap().domain_duration_ticks,
        duration
    );
    assert_eq!(
        project.clips[0].motion().unwrap().sample_offset_tick,
        trimmed_start
    );

    let mut replacement = project.clips[0].motion().unwrap().clone();
    replacement.keyframes[0].x = -0.25;
    store
        .apply(
            3,
            "set-motion",
            false,
            vec![Operation::MotionSet {
                clip_id: "still-1".into(),
                motion: replacement,
            }],
        )
        .unwrap();
    assert_eq!(
        store.snapshot().unwrap().clips[0]
            .motion()
            .unwrap()
            .keyframes[0]
            .x,
        -0.25
    );
    store
        .apply(
            4,
            "lift-remove",
            false,
            vec![Operation::ClipRemove {
                clip_id: "still-1".into(),
                respect_links: true,
                mode: storycut_core::RemoveMode::Lift,
                ripple_track_ids: vec![],
            }],
        )
        .unwrap();
    assert_eq!(store.snapshot().unwrap().duration_ticks(), 0);
}

#[test]
fn operation_parser_accepts_all_contract_tags_and_rejects_unknown_fields() {
    let f = frame_ticks();
    let motion = motion(f);
    let cue = json!({"id":"cue-1","start_tick":0,"end_tick":f,"text":"x","source_payload":"x"});
    let audio = json!({"domain_duration_ticks":f,"sample_offset_tick":0,"gain_db":0.0,"pan":0.0,"muted":false,"fade_in_ticks":0,"fade_out_ticks":0,"fade_curve":"linear_amplitude"});
    let track = serde_json::to_value(Track::new("track-1", "T", TrackKind::Video)).unwrap();
    let clip = clip_value(f);
    let transition = json!({"id":"tr-1","track_id":"track-1","from_clip_id":"a","to_clip_id":"b","start_tick":0,"duration_ticks":f,"kind":"cross_dissolve","curve":"linear","audio_policy":"independent"});
    let link = json!({"id":"link-1","kind":"av_sync","clip_ids":["v","a"]});
    let operations = vec![
        json!({"op":"track.add","track":track}),
        json!({"op":"track.update","track_id":"track-1","changes":{"enabled":false}}),
        json!({"op":"track.reorder","track_ids":["track-1"]}),
        json!({"op":"track.remove","track_id":"track-1","require_empty":true}),
        json!({"op":"clip.add","clip":clip}),
        json!({"op":"clip.move","clip_id":"clip-1","track_id":"track-1","start_tick":0,"respect_links":true}),
        json!({"op":"clip.trim","clip_id":"clip-1","start_tick":0,"source_in_tick":0,"duration_ticks":f,"keyframe_policy":"retime_full","respect_links":true}),
        json!({"op":"clip.split","clip_id":"clip-1","at_tick":f,"respect_links":true}),
        json!({"op":"clip.remove","clip_id":"clip-1","respect_links":true,"mode":"lift","ripple_track_ids":[]}),
        json!({"op":"motion.set","clip_id":"clip-1","motion":motion}),
        json!({"op":"audio.set","clip_id":"audio-1","audio":audio}),
        json!({"op":"transition.set","transition":transition}),
        json!({"op":"transition.remove","transition_id":"tr-1","overlap_resolution":"reject_if_overlap","ripple_track_ids":[]}),
        json!({"op":"link.create","link":link}),
        json!({"op":"link.remove","link_id":"link-1"}),
        json!({"op":"subtitle.cue.update","document_id":"sub-1","cue":cue,"edit_mode":"text_and_time_preserve_syntax","allow_lossy":false}),
        json!({"op":"subtitle.cue.insert","document_id":"sub-1","cue":cue,"edit_mode":"raw_payload","allow_lossy":false}),
        json!({"op":"subtitle.cue.remove","document_id":"sub-1","cue_id":"cue-1","allow_lossy":false}),
        json!({"op":"subtitle.shift","document_id":"sub-1","offset_tick":0}),
    ];
    assert_eq!(operations.len(), 19);
    for value in operations {
        serde_json::from_value::<Operation>(value).unwrap();
    }

    let unknown = json!({
        "op":"track.add",
        "track":track,
        "unexpected":true
    });
    assert!(serde_json::from_value::<Operation>(unknown).is_err());
    assert!(
        serde_json::from_value::<Operation>(json!({
            "op":"track.update","track_id":"track-1","changes":{"enabled":null}
        }))
        .is_err()
    );
}

#[test]
fn linked_video_audio_move_split_envelope_and_unlink_share_one_transaction_core() {
    let temp = TempDir::new().unwrap();
    let (store, _) = ntsc_store(&temp);
    let frame = store.snapshot().unwrap().frame_ticks().unwrap();
    let duration = frame * 300;
    let video_asset = Asset {
        id: "movie-asset".into(),
        kind: AssetKind::Video,
        path: "fixtures/movie.mp4".into(),
        probe_status: ProbeStatus::Probed,
        duration_ticks: Some(duration),
        sha256: None,
        streams: vec![
            Stream {
                index: 0,
                kind: StreamKind::Video,
                time_base: Rational {
                    num: 1,
                    den: 30_000,
                },
                sample_rate: None,
            },
            Stream {
                index: 1,
                kind: StreamKind::Audio,
                time_base: Rational {
                    num: 1,
                    den: 48_000,
                },
                sample_rate: Some(48_000),
            },
        ],
    };
    store
        .import_assets(0, "media-video", false, vec![video_asset])
        .unwrap();
    let audio = AudioSettings {
        domain_duration_ticks: duration,
        sample_offset_tick: 0,
        gain_db: 0.0,
        pan: 0.0,
        muted: false,
        fade_in_ticks: frame,
        fade_out_ticks: frame,
        fade_curve: "linear_amplitude".into(),
    };
    let mut operations = vec![
        Operation::TrackAdd {
            track: Track::new("v1", "V1", TrackKind::Video),
        },
        Operation::TrackAdd {
            track: Track::new("a1", "A1", TrackKind::Audio),
        },
        Operation::ClipAdd {
            clip: storycut_core::Clip::Video(VideoClip {
                id: "video-1".into(),
                track_id: "v1".into(),
                asset_id: "movie-asset".into(),
                start_tick: 0,
                duration_ticks: duration,
                source_in_tick: 0,
                stream_index: 0,
                motion: motion_for(duration, frame),
                audio_policy: VideoAudioPolicy::Muted,
                hold_head_ticks: 0,
                hold_tail_ticks: 0,
            }),
        },
        Operation::ClipAdd {
            clip: storycut_core::Clip::Audio(AudioClip {
                id: "audio-1".into(),
                track_id: "a1".into(),
                asset_id: "movie-asset".into(),
                start_tick: 0,
                duration_ticks: duration,
                source_in_tick: 0,
                stream_index: 1,
                audio: audio.clone(),
            }),
        },
        Operation::LinkCreate {
            link: Link {
                id: "av-1".into(),
                kind: "av_sync".into(),
                clip_ids: ["video-1".into(), "audio-1".into()],
            },
        },
    ];
    let link = operations.pop().unwrap();
    operations.insert(2, link);
    assert_eq!(
        store
            .apply(1, "linked-create", false, operations)
            .unwrap()
            .revision,
        2
    );
    assert_eq!(
        store
            .apply(
                2,
                "linked-move",
                false,
                vec![Operation::ClipMove {
                    clip_id: "video-1".into(),
                    track_id: "v1".into(),
                    start_tick: frame,
                    respect_links: true,
                }]
            )
            .unwrap()
            .revision,
        3
    );

    let split_at = frame * 151;
    assert_eq!(
        store
            .apply(
                3,
                "linked-split",
                false,
                vec![Operation::ClipSplit {
                    clip_id: "video-1".into(),
                    at_tick: split_at,
                    respect_links: true,
                }]
            )
            .unwrap()
            .revision,
        4
    );
    let project = store.snapshot().unwrap();
    assert_eq!(project.links.len(), 2);
    let audio_right = project
        .clips
        .iter()
        .find(|clip| clip.id() != "audio-1" && clip.track_id() == "a1")
        .unwrap();
    assert_eq!(audio_right.start_tick(), split_at);
    assert_eq!(
        audio_right.audio().unwrap().sample_offset_tick,
        split_at - frame
    );

    let mut changed_audio = audio;
    changed_audio.gain_db = -6.0;
    assert_eq!(
        store
            .apply(
                4,
                "audio-envelope",
                false,
                vec![Operation::AudioSet {
                    clip_id: "audio-1".into(),
                    audio: changed_audio,
                }]
            )
            .unwrap()
            .revision,
        5
    );
    assert_eq!(
        store
            .apply(
                5,
                "unlink-left",
                false,
                vec![Operation::LinkRemove {
                    link_id: "av-1".into()
                }]
            )
            .unwrap()
            .revision,
        6
    );
    let project = store.snapshot().unwrap();
    let left_video = project
        .clips
        .iter()
        .find(|clip| clip.id() == "video-1")
        .unwrap();
    assert!(
        matches!(left_video, storycut_core::Clip::Video(v) if v.audio_policy == VideoAudioPolicy::Muted)
    );
    let left_audio = project
        .clips
        .iter()
        .find(|clip| clip.id() == "audio-1")
        .unwrap();
    assert_eq!(left_audio.start_tick() % (TIMEBASE / 48_000), 0);
    assert_eq!(left_audio.audio().unwrap().gain_db, -6.0);
}

#[test]
fn subtitle_import_and_cue_edits_preserve_unknown_source_payload() {
    let temp = TempDir::new().unwrap();
    let (store, _) = new_store(&temp);
    store
        .apply(
            0,
            "subtitle-track",
            false,
            vec![Operation::TrackAdd {
                track: Track::new("subs", "S1", TrackKind::Subtitle),
            }],
        )
        .unwrap();
    let subtitle = Subtitle {
        id: "sub-1".into(),
        track_id: "subs".into(),
        format: SubtitleFormat::Vtt,
        offset_tick: 0,
        path: "captions.vtt".into(),
        raw_document: "WEBVTT\n\nNOTE keep this metadata\n".into(),
        cues: vec![Cue {
            id: "cue-1".into(),
            start_tick: 0,
            end_tick: frame_ticks(),
            text: "old".into(),
            source_payload: "line:90% position:50%".into(),
        }],
        preserve_unknown_syntax: true,
    };
    store
        .import_subtitles(1, "subtitle-import", false, vec![subtitle])
        .unwrap();
    let update = Cue {
        id: "cue-1".into(),
        start_tick: frame_ticks(),
        end_tick: frame_ticks() * 2,
        text: "new".into(),
        source_payload: "must-not-replace".into(),
    };
    store
        .apply(
            2,
            "cue-update",
            false,
            vec![Operation::SubtitleCueUpdate {
                document_id: "sub-1".into(),
                cue: update,
                edit_mode: storycut_core::SubtitleEditMode::TextAndTimePreserveSyntax,
                allow_lossy: false,
            }],
        )
        .unwrap();
    store
        .apply(
            3,
            "subtitle-shift",
            false,
            vec![Operation::SubtitleShift {
                document_id: "sub-1".into(),
                offset_tick: frame_ticks(),
            }],
        )
        .unwrap();
    store
        .apply(
            4,
            "cue-insert",
            false,
            vec![Operation::SubtitleCueInsert {
                document_id: "sub-1".into(),
                cue: Cue {
                    id: "cue-2".into(),
                    start_tick: frame_ticks() * 3,
                    end_tick: frame_ticks() * 4,
                    text: "extra".into(),
                    source_payload: "align:start".into(),
                },
                edit_mode: storycut_core::SubtitleEditMode::RawPayload,
                allow_lossy: false,
            }],
        )
        .unwrap();
    store
        .apply(
            5,
            "cue-remove",
            false,
            vec![Operation::SubtitleCueRemove {
                document_id: "sub-1".into(),
                cue_id: "cue-2".into(),
                allow_lossy: false,
            }],
        )
        .unwrap();
    let project = store.snapshot().unwrap();
    assert_eq!(
        project.subtitles[0].raw_document,
        "WEBVTT\n\nNOTE keep this metadata\n"
    );
    assert_eq!(project.subtitles[0].cues[0].text, "new");
    assert_eq!(
        project.subtitles[0].cues[0].source_payload,
        "line:90% position:50%"
    );
    assert_eq!(project.subtitles[0].offset_tick, frame_ticks());
    assert_eq!(project.subtitles[0].cues.len(), 1);
}

#[test]
fn transitions_and_track_operations_validate_overlaps_and_ripple_as_one_batch() {
    let temp = TempDir::new().unwrap();
    let (store, _) = new_store(&temp);
    store
        .import_assets(
            0,
            "media-1",
            false,
            vec![image_asset("still-asset", frame_ticks())],
        )
        .unwrap();
    let duration = TIMEBASE * 10;
    let overlap = TIMEBASE;
    let transition = |id: &str, from: &str, to: &str, start| Transition {
        id: id.into(),
        track_id: "v1".into(),
        from_clip_id: from.into(),
        to_clip_id: to.into(),
        start_tick: start,
        duration_ticks: overlap,
        kind: TransitionKind::CrossDissolve,
        curve: "linear".into(),
        audio_policy: "independent".into(),
    };
    let image = |id: &str, start| {
        storycut_core::Clip::Image(storycut_core::ImageClip {
            id: id.into(),
            track_id: "v1".into(),
            asset_id: "still-asset".into(),
            start_tick: start,
            duration_ticks: duration,
            source_in_tick: 0,
            motion: motion(duration),
        })
    };
    let mut ops = vec![
        Operation::TrackAdd {
            track: Track::new("v1", "V1", TrackKind::Video),
        },
        Operation::ClipAdd {
            clip: image("a", 0),
        },
        Operation::ClipAdd {
            clip: image("b", duration - overlap),
        },
        Operation::ClipAdd {
            clip: image("c", duration * 2 - overlap * 2),
        },
        Operation::TransitionSet {
            transition: transition("t1", "a", "b", duration - overlap),
        },
        Operation::TransitionSet {
            transition: transition("t2", "b", "c", duration * 2 - overlap * 2),
        },
        Operation::TrackAdd {
            track: Track::new("empty", "Empty", TrackKind::Image),
        },
    ];
    let first_transition = ops.remove(4);
    let second_transition = ops.remove(4);
    ops.insert(1, second_transition);
    ops.insert(1, first_transition);
    store
        .apply(1, "build-transition-timeline", false, ops)
        .unwrap();
    store
        .apply(
            2,
            "track-update",
            false,
            vec![Operation::TrackUpdate {
                track_id: "empty".into(),
                changes: TrackChanges {
                    name: Some("I1".into()),
                    ..Default::default()
                },
            }],
        )
        .unwrap();
    store
        .apply(
            3,
            "track-reorder",
            false,
            vec![Operation::TrackReorder {
                track_ids: vec!["empty".into(), "v1".into()],
            }],
        )
        .unwrap();
    assert!(
        store
            .apply(
                4,
                "reject-remove-transition",
                false,
                vec![Operation::TransitionRemove {
                    transition_id: "t1".into(),
                    overlap_resolution: storycut_core::OverlapResolution::RejectIfOverlap,
                    ripple_track_ids: vec![],
                }]
            )
            .is_err()
    );
    assert_eq!(store.snapshot().unwrap().revision, 4);
    store
        .apply(
            4,
            "butt-cut",
            false,
            vec![Operation::TransitionRemove {
                transition_id: "t1".into(),
                overlap_resolution: storycut_core::OverlapResolution::ButtCutRipple,
                ripple_track_ids: vec!["v1".into()],
            }],
        )
        .unwrap();
    let project = store.snapshot().unwrap();
    assert_eq!(
        project
            .clips
            .iter()
            .find(|clip| clip.id() == "b")
            .unwrap()
            .start_tick(),
        duration
    );
    assert_eq!(
        project
            .clips
            .iter()
            .find(|clip| clip.id() == "c")
            .unwrap()
            .start_tick(),
        duration * 2 - overlap
    );
    assert_eq!(
        project
            .transitions
            .iter()
            .find(|item| item.id == "t2")
            .unwrap()
            .start_tick,
        duration * 2 - overlap
    );
    store
        .apply(
            5,
            "remove-empty-track",
            false,
            vec![Operation::TrackRemove {
                track_id: "empty".into(),
                require_empty: true,
            }],
        )
        .unwrap();
    assert_eq!(store.snapshot().unwrap().tracks.len(), 1);
}

fn held_video_fixture(store: &ProjectStore, frame: u64) -> VideoClip {
    let source = frame * 300;
    store
        .import_assets(
            0,
            "held-import",
            false,
            vec![
                Asset {
                    id: "native".into(),
                    kind: AssetKind::Video,
                    path: "fixtures/native.mp4".into(),
                    probe_status: ProbeStatus::Probed,
                    duration_ticks: Some(source),
                    sha256: None,
                    streams: vec![Stream {
                        index: 0,
                        kind: StreamKind::Video,
                        time_base: Rational { num: 1, den: 30 },
                        sample_rate: None,
                    }],
                },
                Asset {
                    id: "voice".into(),
                    kind: AssetKind::Audio,
                    path: "fixtures/voice.wav".into(),
                    probe_status: ProbeStatus::Probed,
                    duration_ticks: Some(TIMEBASE * 20),
                    sha256: None,
                    streams: vec![Stream {
                        index: 0,
                        kind: StreamKind::Audio,
                        time_base: Rational {
                            num: 1,
                            den: 48_000,
                        },
                        sample_rate: Some(48_000),
                    }],
                },
            ],
        )
        .unwrap();
    let duration = frame * 12 + source + frame * 12;
    VideoClip {
        id: "held".into(),
        track_id: "v1".into(),
        asset_id: "native".into(),
        start_tick: 0,
        duration_ticks: duration,
        source_in_tick: 0,
        stream_index: 0,
        motion: motion_for(duration, frame),
        audio_policy: VideoAudioPolicy::Muted,
        hold_head_ticks: frame * 12,
        hold_tail_ticks: frame * 12,
    }
}

#[test]
fn video_holds_extend_the_timeline_but_not_the_source_and_refuse_trim_split() {
    let temp = TempDir::new().unwrap();
    let (store, _) = new_store(&temp);
    let frame = frame_ticks();
    let held = held_video_fixture(&store, frame);
    let revision = store.snapshot().unwrap().revision;
    // Timeline 324 frames from a 300-frame source is valid only because of the holds.
    let result = store
        .apply(
            revision,
            "held-add",
            false,
            vec![
                Operation::TrackAdd {
                    track: Track::new("v1", "V1", TrackKind::Video),
                },
                Operation::ClipAdd {
                    clip: storycut_core::Clip::Video(held.clone()),
                },
            ],
        )
        .unwrap();
    assert!(result.applied);
    let revision = store.snapshot().unwrap().revision;

    let mut too_long = held.clone();
    too_long.id = "too-long".into();
    too_long.start_tick = frame * 400;
    too_long.hold_head_ticks = 0;
    too_long.motion = motion_for(too_long.duration_ticks, frame);
    let error = store
        .apply(
            revision,
            "held-overrun",
            true,
            vec![Operation::ClipAdd {
                clip: storycut_core::Clip::Video(too_long),
            }],
        )
        .unwrap_err();
    assert!(error.to_string().contains("exceeds source duration"), "{error}");

    let split = store
        .apply(
            revision,
            "held-split",
            true,
            vec![Operation::ClipSplit {
                clip_id: "held".into(),
                at_tick: frame * 100,
                respect_links: true,
            }],
        )
        .unwrap_err();
    assert_eq!(split.code(), "UNSUPPORTED_FEATURE");
    let trim = store
        .apply(
            revision,
            "held-trim",
            true,
            vec![Operation::ClipTrim {
                clip_id: "held".into(),
                start_tick: 0,
                source_in_tick: 0,
                duration_ticks: frame * 200,
                keyframe_policy: storycut_core::KeyframePolicy::RetimeFull,
                respect_links: true,
            }],
        )
        .unwrap_err();
    assert_eq!(trim.code(), "UNSUPPORTED_FEATURE");
}

#[test]
fn track_ducking_is_validated_set_and_cleared_through_track_update() {
    let temp = TempDir::new().unwrap();
    let (store, _) = new_store(&temp);
    let revision = store.snapshot().unwrap().revision;
    store
        .apply(
            revision,
            "tracks",
            false,
            vec![
                Operation::TrackAdd {
                    track: Track::new("voice", "旁白", TrackKind::Audio),
                },
                Operation::TrackAdd {
                    track: Track::new("music", "配樂", TrackKind::Audio),
                },
                Operation::TrackAdd {
                    track: Track::new("pictures", "畫面", TrackKind::Video),
                },
            ],
        )
        .unwrap();
    let revision = store.snapshot().unwrap().revision;
    let ducking = |source: &str| storycut_core::Ducking {
        source_track_id: source.into(),
        threshold: 0.015,
        ratio: 6.0,
        attack_ms: 30.0,
        release_ms: 500.0,
    };
    let update = |track: &str, value: Option<storycut_core::Ducking>| Operation::TrackUpdate {
        track_id: track.into(),
        changes: TrackChanges {
            ducking: Some(value),
            ..TrackChanges::default()
        },
    };
    for (track, source) in [("music", "music"), ("music", "pictures"), ("pictures", "voice"), ("music", "missing")] {
        let error = store
            .apply(revision, "bad-duck", true, vec![update(track, Some(ducking(source)))])
            .unwrap_err();
        assert_eq!(error.code(), "INVALID_ARGUMENT", "{track} by {source}: {error}");
    }
    store
        .apply(revision, "duck", false, vec![update("music", Some(ducking("voice")))])
        .unwrap();
    let project = store.snapshot().unwrap();
    let music = project.tracks.iter().find(|t| t.id == "music").unwrap();
    assert_eq!(music.ducking.as_ref().unwrap().source_track_id, "voice");
    // A ducked track cannot itself be a sidechain source (no chains).
    let error = store
        .apply(project.revision, "chain", true, vec![update("voice", Some(ducking("music")))])
        .unwrap_err();
    assert_eq!(error.code(), "INVALID_ARGUMENT");
    // JSON null clears it.
    let parsed: Operation = serde_json::from_value(json!({
        "op":"track.update","track_id":"music","changes":{"ducking":null}
    }))
    .unwrap();
    store.apply(project.revision, "unduck", false, vec![parsed]).unwrap();
    assert!(store.snapshot().unwrap().tracks.iter().all(|t| t.ducking.is_none()));
}
