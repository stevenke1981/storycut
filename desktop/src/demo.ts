import type { Asset, Clip, Project, Timeline, Track } from "./types";

const motion = (duration: number) => ({
  domain_duration_ticks: duration,
  sample_offset_tick: 0,
  fit: "cover" as const,
  avoid_exposed_edges: true,
  anchor: { x: 0.5, y: 0.5 },
  interpolation: "smoothstep" as const,
  keyframes: [
    { tick: 0, x: -0.02, y: 0, scale: 1.04, opacity: 1 },
    { tick: Math.max(0, duration - 23_520_000), x: 0.02, y: 0, scale: 1.1, opacity: 1 },
  ],
});

const track = (id: string, name: string, kind: Track["kind"]): Track => ({ id, name, kind, locked: false, enabled: true, muted: false, solo: false, gain_db: 0 });

export function makeDemo() {
  const assets: Asset[] = [
    { id: "asset-coast", kind: "image", path: "預覽素材／海岸晨光.png", probe_status: "probed", duration_ticks: null, sha256: null, streams: [] },
    { id: "asset-street", kind: "image", path: "預覽素材／老街雨夜.jpg", probe_status: "probed", duration_ticks: null, sha256: null, streams: [] },
    { id: "asset-forest", kind: "video", path: "預覽素材／森林空拍.mp4", probe_status: "probed", duration_ticks: 12 * 705_600_000, sha256: null, streams: [{ index: 0, kind: "video", time_base: { num: 1, den: 30 }, sample_rate: null }, { index: 1, kind: "audio", time_base: { num: 1, den: 48_000 }, sample_rate: 48_000 }] },
    { id: "asset-voice", kind: "audio", path: "預覽素材／旁白_take03.wav", probe_status: "probed", duration_ticks: 28 * 705_600_000, sha256: null, streams: [{ index: 0, kind: "audio", time_base: { num: 1, den: 48_000 }, sample_rate: 48_000 }] },
  ];
  const tracks = [track("track-video", "主畫面", "video"), track("track-overlay", "疊圖", "image"), track("track-voice", "旁白", "audio"), track("track-music", "配樂", "audio"), track("track-sub", "字幕", "subtitle")];
  const clips: Clip[] = [
    { id: "clip-coast", track_id: "track-video", asset_id: "asset-coast", kind: "image", start_tick: 0, duration_ticks: 10 * 705_600_000, source_in_tick: 0, motion: motion(10 * 705_600_000) },
    { id: "clip-street", track_id: "track-video", asset_id: "asset-street", kind: "image", start_tick: 9 * 705_600_000, duration_ticks: 8 * 705_600_000, source_in_tick: 0, motion: motion(8 * 705_600_000) },
    { id: "clip-forest", track_id: "track-overlay", asset_id: "asset-forest", kind: "video", start_tick: 13 * 705_600_000, duration_ticks: 6 * 705_600_000, source_in_tick: 0, stream_index: 0, audio_policy: "muted", motion: motion(6 * 705_600_000) },
    { id: "clip-voice", track_id: "track-voice", asset_id: "asset-voice", kind: "audio", start_tick: 0, duration_ticks: 20 * 705_600_000, source_in_tick: 0, stream_index: 0, audio: { domain_duration_ticks: 20 * 705_600_000, sample_offset_tick: 0, gain_db: 0, pan: 0, muted: false, fade_in_ticks: 0, fade_out_ticks: 0, fade_curve: "linear_amplitude" } },
  ];
  const project: Project = { schema_version: "0.2.0-draft", project_id: "demo-preview", revision: 7, name: "夜行記・預覽剪輯", timebase: 705_600_000, canvas: { width: 1920, height: 1080, fps: { num: 30, den: 1 }, background: "#101319", color_mode: "sdr_bt709" }, audio_sample_rate: 48_000, notes: [], assets, tracks, clips, transitions: [], links: [], subtitles: [] };
  return { project, timeline: { tracks, clips, transitions: [], links: [], subtitles: [], duration_ticks: 22 * 705_600_000 } satisfies Timeline, assets };
}
