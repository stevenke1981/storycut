export const TIMEBASE = 705_600_000;

export type MediaKind = "video" | "image" | "audio";
export type TrackKind = MediaKind | "subtitle";

export interface Stream {
  index: number;
  kind: "video" | "audio";
  time_base: { num: number; den: number };
  sample_rate: number | null;
}

export interface Asset {
  id: string;
  kind: MediaKind;
  path: string;
  probe_status: "unprobed" | "probed" | "offline" | "error";
  duration_ticks: number | null;
  sha256: string | null;
  streams: Stream[];
}

export interface Track {
  id: string;
  name: string;
  kind: TrackKind;
  locked: boolean;
  enabled: boolean;
  muted: boolean;
  solo: boolean;
  gain_db: number;
}

export interface MotionKeyframe {
  tick: number;
  x: number;
  y: number;
  scale: number;
  opacity: number;
}

export interface Motion {
  domain_duration_ticks: number;
  sample_offset_tick: number;
  fit: "contain" | "cover";
  avoid_exposed_edges: boolean;
  anchor: { x: number; y: number };
  interpolation: "linear" | "smoothstep";
  keyframes: MotionKeyframe[];
}

export interface AudioSettings {
  domain_duration_ticks: number;
  sample_offset_tick: number;
  gain_db: number;
  pan: number;
  muted: boolean;
  fade_in_ticks: number;
  fade_out_ticks: number;
  fade_curve: "linear_amplitude";
}

export interface Clip {
  id: string;
  track_id: string;
  asset_id: string;
  kind: MediaKind;
  start_tick: number;
  duration_ticks: number;
  source_in_tick: number;
  stream_index?: number;
  motion?: Motion;
  audio_policy?: "muted" | "separate_linked";
  audio?: AudioSettings;
}

export interface Project {
  schema_version: string;
  project_id: string;
  revision: number;
  name: string;
  timebase: number;
  canvas: {
    width: number;
    height: number;
    fps: { num: number; den: number };
    background: string;
    color_mode: "sdr_bt709";
  };
  audio_sample_rate: number;
  notes: string[];
  assets: Asset[];
  tracks: Track[];
  clips: Clip[];
  transitions: unknown[];
  links: unknown[];
  subtitles: unknown[];
}

export interface Timeline {
  tracks: Track[];
  clips: Clip[];
  transitions: unknown[];
  links: unknown[];
  subtitles: unknown[];
  duration_ticks: number;
}

export interface ApiError {
  code: string;
  message: string;
  retryable: boolean;
  details: Record<string, unknown>;
}

export interface Envelope<T> {
  api_version: string;
  ok: boolean;
  request_id: string;
  project_id: string | null;
  revision: number | null;
  data: T | null;
  error: ApiError | null;
  warnings: Array<{ code: string; message: string }>;
}

export interface Job {
  job_id: string;
  project_id: string;
  source_revision: number;
  kind: "render" | "preview_frame" | "preview_range";
  state: "queued" | "running" | "cancelling" | "cancelled" | "succeeded" | "failed" | "interrupted";
  progress: number | null;
  last_event_seq: number;
  artifacts: Array<{
    artifact_id: string;
    relative_path: string;
    mime_type: string;
    sha256: string;
    size_bytes: number;
  }>;
  failure_code: string | null;
}

export type BackendMode = "unavailable" | "tauri" | "demo";
