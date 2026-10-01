import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import type { ApiError, Asset, BackendMode, Envelope, Job, Project, Timeline, Track } from "./types";

export class StoryCutApiError extends Error {
  constructor(public readonly apiError: ApiError) {
    super(apiError.message);
    this.name = "StoryCutApiError";
  }
}

type TauriWindow = Window & { __TAURI_INTERNALS__?: { invoke?: (...args: unknown[]) => unknown } };

function hasTauriRuntime() {
  return typeof (window as TauriWindow).__TAURI_INTERNALS__?.invoke === "function";
}

export class StoryCutApi {
  readonly mode: BackendMode;
  private workspace: string | null = null;
  private thumbnailActive = 0;
  private readonly thumbnailWaiters: Array<() => void> = [];
  private readonly thumbnailCache = new Map<string, Promise<AssetThumbnail>>();

  constructor() {
    this.mode = hasTauriRuntime() ? "tauri" : "unavailable";
  }

  get workspacePath() { return this.workspace; }
  async setWorkspace(path: string) {
    if (!hasTauriRuntime()) throw this.unavailable("工作區只能由 StoryCut 桌面版選取。");
    await invoke<void>("storycut_set_workspace", { path });
    this.workspace = path;
  }

  async dispatch<T = unknown>(tool: string, args: Record<string, unknown>): Promise<Envelope<T>> {
    if (!this.workspace) throw this.unavailable("尚未選擇專案工作區。");
    if (!hasTauriRuntime()) throw this.unavailable("桌面核心目前無法使用。請在 StoryCut 桌面版開啟專案。");
    try {
      return await invoke<Envelope<T>>("storycut_dispatch", { workspace: this.workspace, tool, args });
    } catch (error) {
      const value = typeof error === "string" ? error : error instanceof Error ? error.message : "StoryCut 核心呼叫失敗。";
      throw new StoryCutApiError({ code: "INTERNAL_ERROR", message: value, retryable: false, details: {} });
    }
  }

  async checked<T>(tool: string, args: Record<string, unknown>): Promise<Envelope<T>> {
    const result = await this.dispatch<T>(tool, args);
    if (!result.ok || result.error) throw new StoryCutApiError(result.error ?? {
      code: "INTERNAL_ERROR", message: "核心回傳不完整的錯誤資料。", retryable: false, details: {},
    });
    return result;
  }

  async openProject(): Promise<string | null> {
    if (!hasTauriRuntime()) return null;
    const path = await open({ multiple: false, filters: [{ name: "StoryCut 專案", extensions: ["storycut.json"] }] });
    return typeof path === "string" ? path : null;
  }

  async newProjectPath(): Promise<string | null> {
    if (!hasTauriRuntime()) return null;
    return await save({ defaultPath: "新專案.storycut.json", filters: [{ name: "StoryCut 專案", extensions: ["storycut.json"] }] });
  }

  async importMedia(): Promise<string[]> {
    if (!hasTauriRuntime()) return [];
    const result = await open({ multiple: true, defaultPath: this.workspace ?? undefined, filters: [{ name: "影音與圖片", extensions: [...MEDIA_EXTENSIONS] }] });
    if (result === null) return [];
    return this.assertInWorkspace(Array.isArray(result) ? result : [result]);
  }

  /** Splits OS-dropped paths into importable media, a project file, and ignored entries. */
  classifyDropped(paths: string[]): { media: string[]; project: string | null; ignored: number } {
    const project = paths.length === 1 && /\.storycut\.json$/i.test(paths[0]) ? paths[0] : null;
    const media = project ? [] : paths.filter((path) => MEDIA_EXTENSIONS.includes((path.split(".").pop() ?? "").toLowerCase() as (typeof MEDIA_EXTENSIONS)[number]));
    return { media, project, ignored: project ? 0 : paths.length - media.length };
  }

  assertInWorkspace(paths: string[]): string[] {
    const outside = paths.filter((path) => !isWithinWorkspace(path, this.workspace));
    if (outside.length) throw new StoryCutApiError({ code: "PATH_DENIED", message: "素材必須位於目前專案工作區內。StoryCut 不會自動複製素材或放寬存取範圍。", retryable: false, details: { count: outside.length } });
    return paths;
  }

  /** Native OS file drops (Tauri reports real paths; HTML5 File objects do not). */
  async onFileDrop(handlers: { onHover: (active: boolean) => void; onDrop: (paths: string[]) => void }): Promise<() => void> {
    if (!hasTauriRuntime()) return () => {};
    return getCurrentWebview().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "enter" || payload.type === "over") handlers.onHover(true);
      else if (payload.type === "drop") { handlers.onHover(false); handlers.onDrop(payload.paths); }
      else handlers.onHover(false);
    });
  }

  async ffmpegInfo(): Promise<FfmpegInfo> {
    if (!hasTauriRuntime()) throw this.unavailable("FFmpeg 狀態只能由 StoryCut 桌面版讀取。");
    return await invoke<FfmpegInfo>("storycut_ffmpeg_info");
  }

  async chooseOutput(defaultPath: string): Promise<string | null> {
    if (!hasTauriRuntime()) return null;
    const path = await save({ defaultPath: this.workspace ? `${this.workspace}\\${defaultPath}` : defaultPath, filters: [{ name: "MP4 影片", extensions: ["mp4"] }] });
    if (path && !isWithinWorkspace(path, this.workspace)) throw new StoryCutApiError({ code: "PATH_DENIED", message: "輸出必須存放在目前專案工作區內。StoryCut 不會自動授權工作區外路徑。", retryable: false, details: {} });
    return path;
  }

  async readPreview(jobId: string, artifactId: string): Promise<{ mime_type: string; data_url: string }> {
    if (!hasTauriRuntime()) throw this.unavailable("預覽影格只能從 StoryCut 桌面核心讀取。");
    return await invoke("storycut_read_preview", { jobId, artifactId });
  }

  async readAssetThumbnail(projectId: string, asset: Asset): Promise<AssetThumbnail> {
    if (!hasTauriRuntime()) throw this.unavailable("素材縮圖只能由 StoryCut 桌面核心讀取。");
    if (asset.kind === "audio") throw this.unavailable("聲音素材沒有畫面縮圖。");
    const key = [this.workspace ?? "", projectId, asset.id, asset.sha256 ?? ""].join("\u0000");
    const cached = this.thumbnailCache.get(key);
    if (cached) return cached;
    const pending = this.thumbnailSlot(() => invoke<AssetThumbnail>("storycut_read_asset_thumbnail", { projectId, assetId: asset.id }))
      .then((thumbnail) => {
        if (thumbnail.asset_id !== asset.id || thumbnail.mime_type !== "image/png" || !thumbnail.data_url.startsWith("data:image/png;base64,")) {
          throw new Error("核心縮圖 bridge 回傳了不完整或非 PNG 資料。");
        }
        return thumbnail;
      })
      .catch((error) => {
        this.thumbnailCache.delete(key);
        throw error;
      });
    this.thumbnailCache.set(key, pending);
    return pending;
  }

  /** Runs at most two FFmpeg thumbnail decodes at a time so a large bin cannot spawn a process storm. */
  private thumbnailSlot<T>(task: () => Promise<T>): Promise<T> {
    const run = async () => {
      while (this.thumbnailActive >= 2) await new Promise<void>((resolve) => this.thumbnailWaiters.push(resolve));
      this.thumbnailActive += 1;
      try { return await task(); }
      finally { this.thumbnailActive -= 1; this.thumbnailWaiters.shift()?.(); }
    };
    return run();
  }

  private unavailable(message: string) {
    return new StoryCutApiError({ code: "UNSUPPORTED_FEATURE", message, retryable: false, details: {} });
  }
}

export const MEDIA_EXTENSIONS = ["mp4", "mov", "mkv", "webm", "jpg", "jpeg", "png", "webp", "wav", "mp3"] as const;

export interface FfmpegInfo {
  available: boolean;
  version: string | null;
  encoders: Record<string, boolean>;
}

export interface AssetThumbnail {
  asset_id: string;
  mime_type: "image/png";
  data_url: string;
  width: number;
  height: number;
}

function isWithinWorkspace(path: string, workspace: string | null) {
  if (!workspace) return false;
  const normalize = (value: string) => value.replaceAll("/", "\\").replace(/[\\]+$/, "").toLocaleLowerCase();
  const root = normalize(workspace);
  const target = normalize(path);
  return target === root || target.startsWith(`${root}\\`);
}

export const api = new StoryCutApi();

export function timelineOf(data: unknown): Timeline {
  const candidate = (data as { timeline?: Timeline } | null)?.timeline ?? data;
  const timeline = candidate as Timeline;
  if (!timeline || !Array.isArray(timeline.tracks) || !Array.isArray(timeline.clips)) {
    throw new Error("核心回傳的時間軸格式不完整，沒有將畫面狀態當成已保存資料。");
  }
  return timeline;
}

export function projectOf(data: unknown): Project {
  const project = (data as { project?: Project } | null)?.project;
  if (!project || typeof project.project_id !== "string") throw new Error("核心未回傳有效專案。");
  return project;
}

export function mediaOf(data: unknown): Asset[] {
  const assets = (data as { assets?: Asset[] } | null)?.assets;
  return Array.isArray(assets) ? assets : [];
}

export function jobOf(data: unknown): Job {
  const job = (data as { job?: Job } | null)?.job;
  if (!job || typeof job.job_id !== "string") throw new Error("核心未回傳有效工作狀態。");
  return job;
}

export function trackShape(kind: Track["kind"], index: number): Track {
  const prefix = { video: "V", image: "I", audio: "A", subtitle: "S" }[kind];
  return { id: `${kind}-${crypto.randomUUID().replaceAll("-", "").slice(0, 10)}`, name: `${prefix}${index + 1}`, kind, locked: false, enabled: true, muted: false, solo: false, gain_db: 0 };
}

export function idempotencyKey(prefix: string) {
  return `${prefix}-${crypto.randomUUID().replaceAll("-", "")}`;
}

export function errorText(error: unknown) {
  return error instanceof StoryCutApiError ? `${error.apiError.code}：${error.message}` : error instanceof Error ? error.message : String(error);
}
