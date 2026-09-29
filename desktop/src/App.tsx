import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type DragEvent, type PointerEvent as ReactPointerEvent } from "react";
import { api, errorText, idempotencyKey, jobOf, mediaOf, projectOf, timelineOf } from "./api";
import { makeDemo } from "./demo";
import { ImageMotionInspector } from "./ImageMotionInspector";
import { StoryAssemblyPanel, type StoryboardResult } from "./StoryAssemblyPanel";
import { TIMEBASE, type Asset, type AudioSettings, type BackendMode, type Clip, type Envelope, type Job, type MediaKind, type Project, type Timeline, type Track, type TrackKind } from "./types";

type IconName = "folder" | "plus" | "play" | "pause" | "undo" | "redo" | "zoom" | "image" | "video" | "audio" | "subtitle" | "lock" | "unlock" | "eye" | "mute" | "solo" | "render" | "chevron" | "close" | "search" | "split" | "scissors" | "download" | "check";
type HighLevelRequest = { projectId: string; workspace: string | null; revision: number; idempotencyKey: string };

function Icon({ name, size = 16 }: { name: IconName; size?: number }) {
  const paths: Record<IconName, string> = {
    folder: "M3 6.5h7l2 2h9v10H3z M3 9h18", plus: "M12 5v14M5 12h14", play: "m8 5 11 7-11 7z", pause: "M8 5v14M16 5v14", undo: "M9 14 4 9l5-5M4 9h9a7 7 0 0 1 0 14h-1", redo: "m15 14 5-5-5-5m5 5h-9a7 7 0 0 0 0 14h1", zoom: "M4 12h16M12 4v16", image: "M4 4h16v16H4z M4 15l4-4 4 4 3-3 5 5 M14 8h.01", video: "M4 7h12v10H4z M16 10l4-3v10l-4-3z", audio: "M9 18V5l11-2v13 M9 9l11-2 M6 18a3 2 0 1 0 6 0 3 2 0 0 0-6 0 M17 16a3 2 0 1 0 6 0 3 2 0 0 0-6 0", subtitle: "M4 6h16v12H4z M7 10h4m2 0h4M7 14h10", lock: "M6 10h12v10H6z M8 10V7a4 4 0 0 1 8 0v3", unlock: "M6 10h12v10H6z M8 10V7a4 4 0 0 1 7-2", eye: "M2 12s3.5-6 10-6 10 6 10 6-3.5 6-10 6-10-6-10-6z M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6", mute: "M4 10v4h4l5 4V6l-5 4z M17 9l5 6m0-6-5 6", solo: "M4 5h16v14H4z M8 9h8M8 13h5", render: "M4 4h16v16H4z M8 4v4h8V4 M8 20v-7h8v7", chevron: "m6 9 6 6 6-6", close: "m5 5 14 14M19 5 5 19", search: "M20 20l-4-4 M17 10a7 7 0 1 1-14 0 7 7 0 0 1 14 0", split: "M12 3v18 M4 6h5l3 6 3-6h5 M4 18h5l3-6 3 6h5", scissors: "M6 6l12 12M18 6 6 18M6 6a2 2 0 1 0 0 .01M18 18a2 2 0 1 0 0 .01", download: "M12 3v12m-5-5 5 5 5-5 M4 18v3h16v-3", check: "m5 12 4 4L19 6",
  };
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[name]} /></svg>;
}

const KIND_ICON: Record<MediaKind | "subtitle", IconName> = { image: "image", video: "video", audio: "audio", subtitle: "subtitle" };
const KIND_LABEL: Record<TrackKind, string> = { video: "影片", image: "圖片", audio: "聲音", subtitle: "字幕" };

export function App() {
  const [mode, setMode] = useState<BackendMode>(api.mode);
  const [implementedTools, setImplementedTools] = useState<string[]>([]);
  const [implementedOperations, setImplementedOperations] = useState<string[]>([]);
  const [project, setProject] = useState<Project | null>(null);
  const [timeline, setTimeline] = useState<Timeline | null>(null);
  const [assets, setAssets] = useState<Asset[]>([]);
  const [selectedAsset, setSelectedAsset] = useState<string | null>(null);
  const [selectedClip, setSelectedClip] = useState<string | null>(null);
  const [playhead, setPlayhead] = useState(0);
  const [scale, setScale] = useState(24);
  const [playing, setPlaying] = useState(false);
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<"all" | MediaKind>("all");
  const [toast, setToast] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [job, setJob] = useState<Job | null>(null);
  const [subtitleMode, setSubtitleMode] = useState<"none" | "burn">("none");
  const [encoder, setEncoder] = useState<"h264_cpu" | "h264_nvenc">("h264_cpu");
  const [trimState, setTrimState] = useState<{ clipId: string; edge: "left" | "right"; origin: number; start: number; duration: number; original: Clip } | null>(null);
  const [dragClipId, setDragClipId] = useState<string | null>(null);
  const [previewArtifact, setPreviewArtifact] = useState<string | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  const [previewVideoArtifact, setPreviewVideoArtifact] = useState<string | null>(null);
  const [previewVideoUrl, setPreviewVideoUrl] = useState<string | null>(null);
  const [previewRange, setPreviewRange] = useState<{ startTick: number; durationTicks: number } | null>(null);
  const [previewState, setPreviewState] = useState<"idle" | "rendering" | "ready" | "unavailable">("idle");
  const [timelineView, setTimelineView] = useState<"timeline" | "assembly">("timeline");
  const [seekSeconds, setSeekSeconds] = useState("0");
  const previewVideo = useRef<HTMLVideoElement>(null);
  const timeScroll = useRef<HTMLDivElement>(null);
  const timelineRef = useRef<Timeline | null>(null);
  const projectRef = useRef<Project | null>(null);
  const sessionGeneration = useRef(0);
  const syncInFlight = useRef(false);
  const initialFitProject = useRef<string | null>(null);

  useEffect(() => { timelineRef.current = timeline; }, [timeline]);
  useEffect(() => { projectRef.current = project; }, [project]);
  useEffect(() => { setSeekSeconds((playhead / TIMEBASE).toFixed(3)); }, [playhead]);
  useEffect(() => {
    if (!toast) return;
    const id = window.setTimeout(() => setToast(null), 3500);
    return () => window.clearTimeout(id);
  }, [toast]);
  useEffect(() => {
    if (!playing || !timeline || mode !== "demo") return;
    const timer = window.setInterval(() => setPlayhead((current) => current >= timeline.duration_ticks ? 0 : current + TIMEBASE / 15), 67);
    return () => window.clearInterval(timer);
  }, [playing, timeline, mode]);

  useEffect(() => {
    if (!project || !timeline || mode !== "tauri" || busy || document.hidden || Boolean(trimState) || Boolean(dragClipId) || playing || previewState === "rendering") return;
    const projectId = project.project_id;
    const generation = sessionGeneration.current;
    const workspace = api.workspacePath;
    let disposed = false;
    const synchronize = async () => {
      if (disposed || syncInFlight.current || document.hidden || sessionGeneration.current !== generation || projectRef.current?.project_id !== projectId || api.workspacePath !== workspace) return;
      syncInFlight.current = true;
      try {
        const result = await api.checked<{ project: Project }>("storycut_project_get", { project_id: projectId });
        const latest = projectOf(result.data);
        if (disposed || sessionGeneration.current !== generation || api.workspacePath !== workspace || projectRef.current?.project_id !== projectId) return;
        if (latest.revision !== projectRef.current.revision) {
          await refresh(projectId, generation, workspace);
          if (!disposed && sessionGeneration.current === generation && projectRef.current?.project_id === projectId) setToast(`已同步外部修改 · 修訂版 ${latest.revision}。預覽已更新。`);
        }
      } catch (caught) {
        if (!disposed && sessionGeneration.current === generation && projectRef.current?.project_id === projectId) setError(`同步工程失敗：${errorText(caught)}`);
      } finally { syncInFlight.current = false; }
    };
    const timer = window.setInterval(() => { void synchronize(); }, 2000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [mode, project?.project_id, project?.revision, busy, timeline?.duration_ticks, trimState, dragClipId, playing, previewState]);

  useEffect(() => {
    if (!project || !timeline || initialFitProject.current === project.project_id) return;
    initialFitProject.current = project.project_id;
    const frame = window.requestAnimationFrame(() => fitTimeline());
    return () => window.cancelAnimationFrame(frame);
  }, [project?.project_id, timeline?.duration_ticks]);

  const safeAction = useCallback(async (action: () => Promise<void>) => {
    setError(null);
    try { await action(); }
    catch (caught) { setError(errorText(caught)); setToast("操作未套用，時間軸仍以核心回傳為準。"); }
  }, []);

  async function loadCapabilities() {
    try {
      const result = await api.checked<{ implemented_tools: string[]; implemented_operations: string[] }>("storycut_capabilities", {});
      setImplementedTools(result.data?.implemented_tools ?? []);
      setImplementedOperations(result.data?.implemented_operations ?? []);
    } catch (caught) { setError(errorText(caught)); }
  }

  const supportsTool = (tool: string) => mode === "demo" || implementedTools.includes(tool);
  const supportsOperation = (operation: string) => mode === "demo" || implementedOperations.includes(operation);

  async function refresh(projectId: string, expectedGeneration = sessionGeneration.current, expectedWorkspace = api.workspacePath) {
    if (expectedGeneration !== sessionGeneration.current || expectedWorkspace !== api.workspacePath) return;
    let snapshot: { projectResult: Envelope<{ project: Project }>; timelineResult: Envelope<Timeline>; mediaResult: Envelope<{ assets: Asset[] }> } | null = null;
    for (let attempt = 0; attempt < 3; attempt += 1) {
      const [projectResult, timelineResult, mediaResult] = await Promise.all([
        api.checked<{ project: Project }>("storycut_project_get", { project_id: projectId }),
        api.checked<Timeline>("storycut_timeline_get", { project_id: projectId, start_tick: 0, end_tick: null }),
        api.checked<{ assets: Asset[] }>("storycut_media_list", { project_id: projectId }),
      ]);
      const revision = projectOf(projectResult.data).revision;
      if (projectResult.revision === revision && timelineResult.revision === revision && mediaResult.revision === revision) {
        snapshot = { projectResult, timelineResult, mediaResult };
        break;
      }
    }
    if (!snapshot) throw new Error("專案在同步期間持續變更，請重新整理後再編輯。");
    if (expectedGeneration !== sessionGeneration.current || expectedWorkspace !== api.workspacePath) return;
    const { projectResult, timelineResult, mediaResult } = snapshot;
    await loadCapabilities();
    if (expectedGeneration !== sessionGeneration.current || expectedWorkspace !== api.workspacePath) return;
    const p = projectOf(projectResult.data);
    if (projectRef.current?.project_id !== p.project_id) setJob(null);
    previewVideo.current?.pause();
    setProject(p);
    setTimeline(timelineOf(timelineResult.data));
    setAssets(mediaOf(mediaResult.data));
    setSelectedClip((current) => current && timelineOf(timelineResult.data).clips.some((clip) => clip.id === current) ? current : null);
    setPreviewState("idle");
    setPreviewArtifact(null);
    setPreviewUrl(null);
    setPreviewVideoArtifact(null);
    setPreviewVideoUrl(null);
    setPreviewRange(null);
    setPlaying(false);
  }

  async function synchronizeProject() {
    if (!project || mode !== "tauri") return;
    const projectId = project.project_id;
    const generation = sessionGeneration.current;
    const workspace = api.workspacePath;
    await mutate(async () => {
      const result = await api.checked<{ project: Project }>("storycut_project_get", { project_id: projectId });
      const latest = projectOf(result.data);
      if (generation !== sessionGeneration.current || workspace !== api.workspacePath || projectRef.current?.project_id !== projectId) return;
      if (latest.revision !== projectRef.current.revision) {
        await refresh(projectId, generation, workspace);
        setToast(`已同步工程 · 修訂版 ${latest.revision}。舊預覽已失效。`);
      } else setToast(`工程已同步 · 修訂版 ${latest.revision}。`);
    });
  }

  async function openProject() {
    if (mode === "demo") { setToast("預覽專案不可開啟真實檔案。請關閉預覽並使用桌面版。"); return; }
    await safeAction(async () => {
      const path = await api.openProject();
      if (!path) return;
      const previousWorkspace = api.workspacePath;
      const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
      await api.setWorkspace(path.slice(0, slash));
      sessionGeneration.current += 1;
      initialFitProject.current = null;
      setBusy(true);
      try {
        const result = await api.checked<{ project: Project }>("storycut_project_open", { path });
        const opened = projectOf(result.data);
        await refresh(opened.project_id);
        setToast(`已開啟 ${opened.name}`);
      } catch (caught) {
        if (previousWorkspace) { await api.setWorkspace(previousWorkspace); sessionGeneration.current += 1; }
        throw caught;
      } finally { setBusy(false); }
    });
  }

  async function createProject() {
    if (mode === "demo") { setToast("預覽模式不能建立實際專案。"); return; }
    await safeAction(async () => {
      const path = await api.newProjectPath();
      if (!path) return;
      const slash = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
      const previousWorkspace = api.workspacePath;
      await api.setWorkspace(path.slice(0, slash));
      sessionGeneration.current += 1;
      initialFitProject.current = null;
      setBusy(true);
      try {
        const result = await api.checked<{ project: Project }>("storycut_project_create", {
          path,
          name: path.slice(slash + 1).replace(/\.storycut\.json$/i, "") || "未命名專案",
          canvas: { width: 1920, height: 1080, fps: { num: 30, den: 1 }, background: "#000000", color_mode: "sdr_bt709" },
          audio_sample_rate: 48000,
          idempotency_key: idempotencyKey("create-project"),
        });
        const created = projectOf(result.data);
        await refresh(created.project_id);
        setToast("空白專案已建立。請新增軌道與匯入素材。");
      } catch (caught) {
        if (previousWorkspace) { await api.setWorkspace(previousWorkspace); sessionGeneration.current += 1; }
        throw caught;
      } finally { setBusy(false); }
    });
  }

  function openDemo() {
    sessionGeneration.current += 1;
    initialFitProject.current = null;
    const next = makeDemo();
    setMode("demo");
    setProject(next.project);
    setTimeline(next.timeline);
    setAssets(next.assets);
    setSelectedAsset(null);
    setSelectedClip(null);
    setPlayhead(0);
    setPreviewVideoUrl(null);
    setPreviewRange(null);
    setPlaying(false);
    setError(null);
    setToast("預覽專案已開啟；此模式的修改不會儲存。");
  }

  async function importAssets() {
    if (!project || !timeline) return;
    if (mode === "demo") {
      setToast("請在預覽模式拖曳素材卡或使用匯入按鈕選擇本機檔案；不會寫入專案。");
      document.getElementById("demo-file-input")?.click();
      return;
    }
    if (!supportsTool("storycut_media_import")) { setError("核心尚未實作媒體匯入（UNSUPPORTED_FEATURE）。沒有變更專案素材。"); return; }
    await safeAction(async () => {
      const paths = await api.importMedia();
      if (!paths.length) return;
      setBusy(true);
      try {
        const result = await api.checked<{ committed: boolean; assets: Asset[] }>("storycut_media_import", {
          project_id: project.project_id, expected_revision: project.revision, idempotency_key: idempotencyKey("media-import"), dry_run: false, paths,
        });
        await refresh(project.project_id);
        setToast(`已匯入 ${mediaOf(result.data).length} 個素材`);
      } finally { setBusy(false); }
    });
  }

  async function addTrack(kind: TrackKind) {
    if (!project || !timeline) return;
    if (mode !== "demo" && !supportsOperation("track.add")) { setError("核心能力清單未宣告 track.add；軌道未建立。"); return; }
    const track = { id: `${kind}-${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`, kind, name: `${KIND_LABEL[kind]} ${timeline.tracks.filter((item) => item.kind === kind).length + 1}`, locked: false, enabled: true, muted: false, solo: false, gain_db: 0 };
    await mutate(async () => {
      if (mode === "demo") { setTimeline((current) => current && ({ ...current, tracks: [...current.tracks, track] })); setProject((current) => current && ({ ...current, tracks: [...current.tracks, track], revision: current.revision + 1 })); return; }
      await applyOperations(project, [{ op: "track.add", track }]);
      await refresh(project.project_id);
    });
  }

  async function addStarterTracks() {
    if (!project || !timeline || timeline.tracks.length || !supportsOperation("track.add")) return;
    const starter = [
      { id: "v1", name: "主畫面", kind: "video" as const },
      { id: "i1", name: "圖片疊圖", kind: "image" as const },
      { id: "a1", name: "原聲／旁白", kind: "audio" as const },
      { id: "a2", name: "配樂", kind: "audio" as const },
      { id: "s1", name: "字幕", kind: "subtitle" as const },
    ].map((track) => ({ op: "track.add", track: { ...track, locked: false, enabled: true, muted: false, solo: false, gain_db: 0 } }));
    await mutate(async () => {
      await applyOperations(project, starter);
      await refresh(project.project_id);
      setToast("已在核心建立主畫面、疊圖、兩條聲音與字幕軌。");
    });
  }

  async function applyOperations(currentProject: Project, operations: unknown[]) {
    return api.checked("storycut_timeline_apply", {
      project_id: currentProject.project_id,
      expected_revision: currentProject.revision,
      idempotency_key: idempotencyKey("timeline-edit"),
      dry_run: false,
      operations,
    });
  }

  async function runHighLevel<T>(tool: string, args: Record<string, unknown>, dryRun: boolean, request: HighLevelRequest): Promise<T> {
    if (!project || mode !== "tauri") throw new Error("此命令只在已開啟的桌面專案中執行；未寫入專案。");
    if (!implementedTools.includes(tool)) throw new Error(`核心能力清單尚未列出 ${tool}；未呼叫命令。`);
    if (request.projectId !== project.project_id || request.workspace !== api.workspacePath) throw new Error("專案或工作區已切換；這份預覽不會提交，請在目前專案重新 dry-run。");
    if (dryRun && request.revision !== project.revision) throw new Error("專案修訂已變更；這份 dry-run 尚未呼叫核心，請重新預覽。");
    if (busy) throw new Error("共用核心正在處理另一項操作，請稍後再試。");
    const projectId = project.project_id;
    const generation = sessionGeneration.current;
    const workspace = api.workspacePath;
    setError(null); setBusy(true);
    try {
      const result = await api.checked<T>(tool, {
        ...args,
        project_id: request.projectId,
        expected_revision: request.revision,
        idempotency_key: request.idempotencyKey,
        dry_run: dryRun,
      });
      if (result.data === null) throw new Error(`${tool} 沒有回傳交易結果。`);
      if (!dryRun && generation === sessionGeneration.current && workspace === api.workspacePath && request.workspace === workspace && projectRef.current?.project_id === projectId) {
        await refresh(projectId, generation, workspace);
        setToast(`${tool} 已由共用核心提交，時間軸與預覽已更新。`);
      }
      return result.data;
    } catch (caught) {
      if (generation === sessionGeneration.current && workspace === api.workspacePath && request.projectId === projectRef.current?.project_id) setError(errorText(caught));
      throw caught;
    } finally { setBusy(false); }
  }

  async function updateTrack(track: Track, changes: Partial<Track>) {
    if (!project || !timeline) return;
    if (mode !== "demo" && !supportsOperation("track.update")) { setError("核心能力清單未宣告 track.update；軌道狀態未變更。"); return; }
    await mutate(async () => {
      if (mode === "demo") {
        setTimeline((current) => current && ({ ...current, tracks: current.tracks.map((item) => item.id === track.id ? { ...item, ...changes } : item) }));
        setProject((current) => current && ({ ...current, tracks: current.tracks.map((item) => item.id === track.id ? { ...item, ...changes } : item), revision: current.revision + 1 }));
        return;
      }
      await applyOperations(project, [{ op: "track.update", track_id: track.id, changes }]);
      await refresh(project.project_id);
    });
  }

  async function updateAudioClip(clip: Clip, audio: AudioSettings) {
    if (!project || !clip.audio) return;
    if (audio.fade_in_ticks + audio.fade_out_ticks > clip.duration_ticks) { setError("淡入與淡出秒數合計不可超過片段長度；音訊未變更。"); return; }
    if (mode !== "demo" && !supportsOperation("audio.set")) { setError("核心能力清單未宣告 audio.set；片段音訊設定未變更。"); return; }
    await mutate(async () => {
      if (mode === "demo") {
        const next = { ...clip, audio };
        setTimeline((current) => current && ({ ...current, clips: current.clips.map((item) => item.id === clip.id ? next : item) }));
        setProject((current) => current && ({ ...current, clips: current.clips.map((item) => item.id === clip.id ? next : item), revision: current.revision + 1 }));
        return;
      }
      await applyOperations(project, [{ op: "audio.set", clip_id: clip.id, audio }]);
      await refresh(project.project_id);
    });
  }

  function fitTimeline() {
    const availableWidth = Math.max(120, (timeScroll.current?.clientWidth ?? 900) - 216 - 18);
    const duration = Math.max(18, (timelineRef.current?.duration_ticks ?? 0) / TIMEBASE + 3);
    setScale(Math.max(0.5, Math.min(148, availableWidth / duration)));
    if (timeScroll.current) timeScroll.current.scrollLeft = 0;
  }

  async function mutate(action: () => Promise<void>, recover?: () => Promise<void>) {
    if (busy) { setToast("共用核心正在處理另一項操作，請稍後再試。"); return; }
    setError(null); setBusy(true);
    try { await action(); }
    catch (caught) {
      setError(errorText(caught));
      try { await recover?.(); }
      catch (recoveryError) { setError(`${errorText(caught)}；重新讀取核心狀態也失敗：${errorText(recoveryError)}`); }
    }
    finally { setBusy(false); }
  }

  function compatibleTrack(asset: Asset, preferredTrack?: Track): Track | undefined {
    const candidates = (timeline?.tracks ?? []).filter((track) => !track.locked && (asset.kind === "image" ? track.kind === "image" || track.kind === "video" : track.kind === asset.kind));
    return preferredTrack && candidates.some((track) => track.id === preferredTrack.id) ? preferredTrack : candidates[0];
  }

  async function addAssetClip(assetId: string, targetTick = playhead, preferredTrack?: Track) {
    if (!project || !timeline) return;
    if (mode !== "demo" && !supportsOperation("clip.add")) { setError("核心能力清單未宣告 clip.add；素材仍留在素材庫。"); return; }
    const asset = assets.find((item) => item.id === assetId);
    if (!asset) return;
    const track = compatibleTrack(asset, preferredTrack);
    if (!track) { setError(`沒有可用的${KIND_LABEL[asset.kind]}軌道。先新增軌道，再把素材加入時間軸。`); return; }
    const start = snapTick(targetTick, project.canvas.fps.num, project.canvas.fps.den);
    const maxDuration = Math.max(TIMEBASE / project.canvas.fps.num, (asset.duration_ticks ?? 0));
    const duration = asset.kind === "image" ? 10 * TIMEBASE : Math.max(TIMEBASE / project.canvas.fps.num, maxDuration);
    const clipId = `clip-${crypto.randomUUID().replaceAll("-", "").slice(0, 14)}`;
    let clip: Clip;
    if (asset.kind === "image") clip = { id: clipId, track_id: track.id, asset_id: asset.id, kind: "image", start_tick: start, duration_ticks: duration, source_in_tick: 0, motion: defaultMotion(duration) };
    else if (asset.kind === "audio") {
      const stream = asset.streams.find((item) => item.kind === "audio");
      if (!stream) { setError("素材沒有已探測的音訊串流。請先檢查素材狀態。"); return; }
      clip = { id: clipId, track_id: track.id, asset_id: asset.id, kind: "audio", start_tick: start, duration_ticks: duration, source_in_tick: 0, stream_index: stream.index, audio: { domain_duration_ticks: duration, sample_offset_tick: 0, gain_db: 0, pan: 0, muted: false, fade_in_ticks: 0, fade_out_ticks: 0, fade_curve: "linear_amplitude" } };
    } else {
      const stream = asset.streams.find((item) => item.kind === "video");
      if (!stream) { setError("素材沒有已探測的視訊串流。請先檢查素材狀態。"); return; }
      const audioStream = asset.streams.find((item) => item.kind === "audio");
      if (audioStream && mode !== "demo" && !supportsOperation("link.create")) { setError("影片含有原聲，但核心未宣告 link.create；為避免無聲匯入，片段未加入。"); return; }
      clip = { id: clipId, track_id: track.id, asset_id: asset.id, kind: "video", start_tick: start, duration_ticks: duration, source_in_tick: 0, stream_index: stream.index, audio_policy: audioStream ? "separate_linked" : "muted", motion: defaultMotion(duration) };
    }
    await mutate(async () => {
      if (mode === "demo") {
        setTimeline((current) => current && ({ ...current, clips: [...current.clips, clip].sort((a, b) => a.start_tick - b.start_tick), duration_ticks: Math.max(current.duration_ticks, clip.start_tick + clip.duration_ticks) }));
        setProject((current) => current && ({ ...current, clips: [...current.clips, clip], revision: current.revision + 1 }));
        setSelectedClip(clip.id); return;
      }
      const operations: Array<Record<string, unknown>> = [{ op: "clip.add", clip }];
      if (clip.kind === "video" && clip.audio_policy === "separate_linked") {
        const audioStream = asset.streams.find((item) => item.kind === "audio");
        if (audioStream) {
          let audioTrack = timeline.tracks.find((item) => item.kind === "audio" && !item.locked);
          if (!audioTrack) {
            audioTrack = { id: `audio-${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`, name: "影片原聲", kind: "audio", locked: false, enabled: true, muted: false, solo: false, gain_db: 0 };
            operations.unshift({ op: "track.add", track: audioTrack });
          }
          const audioClipId = `audio-${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`;
          const audioClip: Clip = { id: audioClipId, track_id: audioTrack.id, asset_id: asset.id, kind: "audio", start_tick: clip.start_tick, duration_ticks: clip.duration_ticks, source_in_tick: clip.source_in_tick, stream_index: audioStream.index, audio: { domain_duration_ticks: clip.duration_ticks, sample_offset_tick: 0, gain_db: 0, pan: 0, muted: false, fade_in_ticks: 0, fade_out_ticks: 0, fade_curve: "linear_amplitude" } };
          operations.push({ op: "clip.add", clip: audioClip }, { op: "link.create", link: { id: `av-${crypto.randomUUID().replaceAll("-", "").slice(0, 12)}`, kind: "av_sync", clip_ids: [clip.id, audioClip.id] } });
        }
      }
      await applyOperations(project, operations);
      await refresh(project.project_id);
      setSelectedClip(clip.id);
      if (clip.kind === "video" && clip.audio_policy === "separate_linked") setToast("影片畫面與原聲已在同一筆核心交易中連結。");
    });
  }

  async function importDemoFiles(files: FileList | null) {
    if (!files || !project || !timeline) return;
    const imported: Asset[] = Array.from(files).map((file, index) => {
      const ext = file.name.split(".").pop()?.toLowerCase();
      const kind: MediaKind = ["png", "jpg", "jpeg", "webp"].includes(ext ?? "") ? "image" : ["wav", "mp3"].includes(ext ?? "") ? "audio" : "video";
      const duration = kind === "image" ? null : kind === "audio" ? 8 * TIMEBASE : 10 * TIMEBASE;
      return { id: `local-${Date.now()}-${index}`, kind, path: file.name, probe_status: "unprobed", duration_ticks: duration, sha256: null, streams: kind === "image" ? [] : [{ index: 0, kind: kind === "audio" ? "audio" : "video", time_base: { num: 1, den: 30 }, sample_rate: kind === "audio" ? 48_000 : null }, ...(kind === "video" ? [{ index: 1, kind: "audio" as const, time_base: { num: 1, den: 48_000 }, sample_rate: 48_000 }] : [])] };
    });
    setAssets((current) => [...current, ...imported]);
    setSelectedAsset(imported[0]?.id ?? null);
    setToast(`${imported.length} 個素材加入預覽清單；未探測、未寫入專案。`);
  }

  async function moveClip(clip: Clip, startTick: number, targetTrack: Track) {
    if (!project || !timeline || targetTrack.locked || clip.track_id === targetTrack.id && clip.start_tick === startTick) return;
    if (mode !== "demo" && !supportsOperation("clip.move")) { setError("核心能力清單未宣告 clip.move；片段未移動。"); return; }
    const start = snapTick(Math.max(0, startTick), project.canvas.fps.num, project.canvas.fps.den);
    if (mode === "demo") {
      const moved = { ...clip, track_id: targetTrack.id, start_tick: start };
      setTimeline((current) => current && ({ ...current, clips: current.clips.map((item) => item.id === clip.id ? moved : item), duration_ticks: Math.max(current.duration_ticks, start + clip.duration_ticks) }));
      setProject((current) => current && ({ ...current, clips: current.clips.map((item) => item.id === clip.id ? moved : item), revision: current.revision + 1 }));
      return;
    }
    await mutate(async () => { await applyOperations(project, [{ op: "clip.move", clip_id: clip.id, track_id: targetTrack.id, start_tick: start, respect_links: true }]); await refresh(project.project_id); });
  }

  async function trimClip(clip: Clip, startTick: number, duration: number) {
    if (!project || duration < 1 || !timeline) return;
    if (mode !== "demo" && !supportsOperation("clip.trim")) {
      setError("核心能力清單未宣告 clip.trim；片段未修剪。");
      try { await refresh(project.project_id); }
      catch (caught) { setError(`核心能力清單未宣告 clip.trim；重新讀取核心狀態也失敗：${errorText(caught)}`); }
      return;
    }
    const frame = TIMEBASE * project.canvas.fps.den / project.canvas.fps.num;
    const start = Math.max(0, Math.round(startTick / frame) * frame);
    const snappedDuration = Math.max(frame, Math.round(duration / frame) * frame);
    const sourceDelta = start - clip.start_tick;
    const sourceIn = clip.kind === "image" ? 0 : Math.max(0, clip.source_in_tick + sourceDelta);
    if (mode === "demo") {
      const updated = { ...clip, start_tick: start, duration_ticks: snappedDuration, source_in_tick: sourceIn };
      setTimeline((current) => current && ({ ...current, clips: current.clips.map((item) => item.id === clip.id ? updated : item), duration_ticks: Math.max(0, ...current.clips.filter((item) => item.id !== clip.id).map((item) => item.start_tick + item.duration_ticks), updated.start_tick + updated.duration_ticks) }));
      setProject((current) => current && ({ ...current, clips: current.clips.map((item) => item.id === clip.id ? updated : item), revision: current.revision + 1 }));
      return;
    }
    await mutate(async () => { await applyOperations(project, [{ op: "clip.trim", clip_id: clip.id, start_tick: start, source_in_tick: sourceIn, duration_ticks: snappedDuration, keyframe_policy: "resample_visible", respect_links: true }]); await refresh(project.project_id); }, async () => { await refresh(project.project_id); });
  }

  async function splitAtPlayhead() {
    const clip = timeline?.clips.find((item) => item.id === selectedClip);
    if (!clip || !project || playhead <= clip.start_tick || playhead >= clip.start_tick + clip.duration_ticks) return;
    if (mode === "demo") { setToast("預覽模式不執行核心分割。"); return; }
    if (!supportsOperation("clip.split")) { setError("核心能力清單未宣告 clip.split；片段未分割。"); return; }
    await mutate(async () => { await applyOperations(project, [{ op: "clip.split", clip_id: clip.id, at_tick: playhead, respect_links: true }]); await refresh(project.project_id); });
  }

  async function undoRedo(which: "undo" | "redo") {
    if (!project) return;
    if (mode === "demo") { setToast("預覽模式沒有共用核心歷史。"); return; }
    const tool = which === "undo" ? "storycut_history_undo" : "storycut_history_redo";
    await mutate(async () => { await api.checked(tool, { project_id: project.project_id, expected_revision: project.revision, idempotency_key: idempotencyKey(`history-${which}`), dry_run: false }); await refresh(project.project_id); });
  }

  async function showPreviewFrame(completed: Job) {
    if (!await previewSourceIsCurrent(completed)) { setPreviewState("idle"); setToast("預覽完成期間專案已有變更；舊修訂版影格已捨棄，請重新產生。"); return; }
    const artifact = completed.artifacts.find((item) => item.mime_type === "image/png");
    if (!artifact) throw new Error("核心工作完成，但沒有回傳 PNG 預覽影格。");
    const image = await api.readPreview(completed.job_id, artifact.artifact_id);
    if (image.mime_type !== "image/png" || !image.data_url.startsWith("data:image/png;base64,")) throw new Error("預覽 bridge 回傳了非 PNG 資料。");
    if (!await previewSourceIsCurrent(completed)) { setPreviewState("idle"); setToast("預覽完成期間專案已有變更；舊修訂版影格已捨棄，請重新產生。"); return; }
    setPreviewArtifact(artifact.relative_path);
    setPreviewUrl(image.data_url);
    setPreviewState("ready");
  }

  async function showPreviewRange(completed: Job, startTick: number, durationTicks: number) {
    if (!await previewSourceIsCurrent(completed)) { setPreviewState("idle"); setToast("影片預覽完成期間專案已有變更；舊修訂版預覽已捨棄，請重新產生。"); return; }
    const artifact = completed.artifacts.find((item) => item.mime_type === "video/mp4");
    if (!artifact) throw new Error("核心工作完成，但沒有回傳 MP4 影片預覽。" );
    const video = await api.readPreview(completed.job_id, artifact.artifact_id);
    if (video.mime_type !== "video/mp4" || !video.data_url.startsWith("data:video/mp4;base64,")) throw new Error("預覽 bridge 回傳了非 MP4 影片資料。" );
    if (!await previewSourceIsCurrent(completed)) { setPreviewState("idle"); setToast("影片預覽完成期間專案已有變更；舊修訂版預覽已捨棄，請重新產生。"); return; }
    setPreviewVideoArtifact(artifact.relative_path);
    setPreviewVideoUrl(video.data_url);
    setPreviewRange({ startTick, durationTicks });
    setPreviewState("ready");
  }

  async function previewSourceIsCurrent(completed: Job) {
    const current = projectRef.current;
    if (!current || current.project_id !== completed.project_id || current.revision !== completed.source_revision) return false;
    const latest = await api.checked<{ project: Project }>("storycut_project_get", { project_id: completed.project_id });
    return projectOf(latest.data).revision === completed.source_revision
      && projectRef.current?.project_id === completed.project_id
      && projectRef.current.revision === completed.source_revision;
  }

  async function requestPreview() {
    if (!project || mode === "demo") { setPreviewState("unavailable"); setToast("預覽畫面由渲染核心產生；互動預覽模式不會製作假預覽。"); return; }
    if (!supportsTool("storycut_preview_frame")) { setPreviewState("unavailable"); setError("核心能力清單未宣告 preview_frame；目前不能產生精確預覽。"); return; }
    await safeAction(async () => {
      setPreviewVideoUrl(null);
      setPreviewVideoArtifact(null);
      setPreviewRange(null);
      setPlaying(false);
      setPreviewArtifact(null);
      setPreviewUrl(null);
      setPreviewState("rendering");
      try {
      const result = await api.checked<{ job: Job }>("storycut_preview_frame", { project_id: project.project_id, revision: project.revision, idempotency_key: idempotencyKey("preview"), width: 1280, include_subtitles: true, at_tick: playhead });
      let latest = jobOf(result.data);
      setJob(latest);
      for (let attempt = 0; attempt < 80; attempt += 1) {
        if (latest.state === "succeeded") { await showPreviewFrame(latest); return; }
        if (["failed", "cancelled", "interrupted"].includes(latest.state)) {
          setPreviewState("unavailable");
          throw new Error(latest.failure_code ?? `預覽工作狀態：${latest.state}`);
        }
        await new Promise((resolve) => window.setTimeout(resolve, 600));
        const updated = await api.checked<{ job: Job }>("storycut_job_get", { job_id: latest.job_id, include_preview: false });
        latest = jobOf(updated.data);
        setJob(latest);
      }
      setToast("預覽工作仍在核心佇列中。可在右側工作面板更新狀態。");
      } catch (caught) {
        setPreviewState("unavailable");
        throw caught;
      }
    });
  }

  async function generateRangePreview() {
    if (!project || !timeline) return;
    if (mode !== "tauri") { setToast("影片預覽必須由 StoryCut 共用渲染核心產生。預覽模式不會製作假影片。"); return; }
    if (!supportsTool("storycut_preview_range")) { setPreviewState("unavailable"); setError("核心能力清單未宣告 preview_range；沒有產生影片預覽。"); return; }
    const frame = Math.max(1, Math.round(frameTicks(project)));
    if (timeline.duration_ticks < frame) { setError("時間軸尚無可播放片段。先把素材加入時間軸，再產生影片預覽。"); return; }
    const lastFrameStart = Math.max(0, timeline.duration_ticks - frame);
    const startTick = Math.min(Math.floor(playhead / frame) * frame, lastFrameStart);
    const durationTicks = Math.min(6 * TIMEBASE, timeline.duration_ticks - startTick);
    if (durationTicks < 1) { setError("播放頭已在時間軸末端，請先移到片段範圍內。"); return; }
    await safeAction(async () => {
      setBusy(true);
      setPlaying(false);
      setPreviewArtifact(null);
      setPreviewUrl(null);
      setPreviewVideoArtifact(null);
      setPreviewVideoUrl(null);
      setPreviewRange(null);
      setPreviewState("rendering");
      try {
        const result = await api.checked<{ job: Job }>("storycut_preview_range", {
          project_id: project.project_id,
          revision: project.revision,
          idempotency_key: idempotencyKey("preview-range"),
          width: Math.min(960, Math.max(160, project.canvas.width - (project.canvas.width % 2))),
          include_subtitles: true,
          start_tick: startTick,
          duration_ticks: durationTicks,
        });
        const completed = jobOf(result.data) as Job & { start_tick?: number; duration_ticks?: number };
        setJob(completed);
        if (completed.kind !== "preview_range" || completed.state !== "succeeded") {
          setPreviewState("unavailable");
          throw new Error(completed.failure_code ?? `影片預覽工作狀態：${completed.state}`);
        }
        const currentProject = await api.checked<{ project: Project }>("storycut_project_get", { project_id: project.project_id });
        if (projectOf(currentProject.data).revision !== completed.source_revision) {
          setPreviewState("idle");
          await refresh(project.project_id);
          setToast("影片預覽完成期間專案已有變更；舊修訂版預覽已捨棄，請重新產生。");
          return;
        }
        const artifact = completed.artifacts.find((item) => item.mime_type === "video/mp4");
        if (!artifact) throw new Error("核心完成影片預覽工作，但沒有回傳 MP4 產物。" );
        const loaded = await api.readPreview(completed.job_id, artifact.artifact_id);
        if (loaded.mime_type !== "video/mp4" || !loaded.data_url.startsWith("data:video/mp4;base64,")) throw new Error("桌面安全橋接沒有回傳 MP4 影片。" );
        if (!await previewSourceIsCurrent(completed)) {
          setPreviewState("idle");
          await refresh(project.project_id);
          setToast("影片預覽完成期間專案已有變更；舊修訂版預覽已捨棄，請重新產生。");
          return;
        }
        setPreviewVideoArtifact(artifact.relative_path);
        setPreviewVideoUrl(loaded.data_url);
        setPreviewRange({ startTick, durationTicks });
        setPreviewState("ready");
        setPlayhead(startTick);
        setToast("核心已同步產生影片預覽。按播放開始；預覽最長 6 秒，字幕依目前設定燒錄。");
      } catch (caught) {
        setPreviewState("unavailable");
        throw caught;
      } finally { setBusy(false); }
    });
  }

  async function togglePlayback() {
    if (mode === "demo") { setPlaying((value) => !value); return; }
    if (mode !== "tauri" || !project || !timeline) return;
    const video = previewVideo.current;
    if (!previewVideoUrl || !previewRange || !video) {
      await generateRangePreview();
      return;
    }
    if (!video.paused) { video.pause(); return; }
    const rangeEnd = previewRange.startTick + previewRange.durationTicks;
    let seekTick = playhead;
    if (video.ended && seekTick >= rangeEnd - frameTicks(project) / 2) {
      seekTick = previewRange.startTick;
      setPlayhead(seekTick);
    } else if (seekTick < previewRange.startTick || seekTick >= rangeEnd) {
      setPreviewVideoUrl(null);
      setPreviewVideoArtifact(null);
      setPreviewRange(null);
      setPreviewState("idle");
      await generateRangePreview();
      return;
    }
    video.currentTime = Math.max(0, Math.min(video.duration || 0, (seekTick - previewRange.startTick) / TIMEBASE));
    try { await video.play(); }
    catch (caught) { setPlaying(false); setError(`無法播放核心預覽影片：${errorText(caught)}`); }
  }

  function seekTo(tick: number) {
    const target = Math.min(timeline?.duration_ticks ?? 0, Math.max(0, tick));
    previewVideo.current?.pause();
    if (previewRange && target >= previewRange.startTick && target < previewRange.startTick + previewRange.durationTicks) {
      if (previewVideo.current) previewVideo.current.currentTime = (target - previewRange.startTick) / TIMEBASE;
    } else if (previewVideoUrl) {
      setPreviewVideoUrl(null);
      setPreviewVideoArtifact(null);
      setPreviewRange(null);
      setPreviewState("idle");
    }
    setPlayhead(target);
  }

  async function startRender() {
    if (!project || !timeline) return;
    if (mode === "demo") { setToast("預覽模式不會建立輸出影片。"); return; }
    if (!supportsTool("storycut_render_start")) { setError("核心能力清單未宣告 render_start；沒有建立工作或輸出檔。"); return; }
    await safeAction(async () => {
      const path = await api.chooseOutput(`${project.name}.mp4`);
      if (!path) return;
      setBusy(true);
      try {
        const result = await api.checked<{ job: Job }>("storycut_render_start", { project_id: project.project_id, revision: project.revision, path, overwrite: false, idempotency_key: idempotencyKey("render"), subtitle_mode: subtitleMode, encoder, range_start_tick: 0, range_end_tick: null });
        const completed = jobOf(result.data);
        setJob(completed);
        if (completed.state === "succeeded") {
          const artifact = completed.artifacts.find((item) => item.mime_type === "video/mp4");
          if (!artifact) throw new Error("核心將輸出工作標示為成功，但沒有回傳 MP4 產物。");
          setToast(`核心輸出完成：${artifact.relative_path}`);
        } else if (["failed", "cancelled", "interrupted"].includes(completed.state)) {
          throw new Error(`核心輸出${completed.state === "failed" ? "失敗" : "已停止"}：${completed.failure_code ?? completed.state}`);
        } else {
          setToast("輸出工作已送交核心；請在右側更新工作狀態。尚未確認影片完成。");
        }
      } finally { setBusy(false); }
    });
  }

  async function refreshJob() {
    if (!job || mode === "demo") return;
    await safeAction(async () => {
      const result = await api.checked<{ job: Job }>("storycut_job_get", { job_id: job.job_id, include_preview: true });
      const latest = jobOf(result.data);
      setJob(latest);
      try {
        if (latest.kind === "preview_frame" && latest.state === "succeeded") await showPreviewFrame(latest);
        else if (latest.kind === "preview_range" && latest.state === "succeeded" && previewRange) await showPreviewRange(latest, previewRange.startTick, previewRange.durationTicks);
        else if (latest.kind === "render" && latest.state === "succeeded" && !latest.artifacts.some((item) => item.mime_type === "video/mp4")) throw new Error("核心將輸出工作標示為成功，但沒有回傳 MP4 產物。");
        else if (["failed", "cancelled", "interrupted"].includes(latest.state)) {
          if (latest.kind !== "render") setPreviewState("unavailable");
          throw new Error(`核心${latest.kind === "render" ? "輸出" : "預覽"}${latest.state === "failed" ? "失敗" : "已停止"}：${latest.failure_code ?? latest.state}`);
        }
      } catch (caught) {
        if (latest.kind !== "render") setPreviewState("unavailable");
        throw caught;
      }
    });
  }

  async function cancelJob() {
    if (!job || !project || mode === "demo") return;
    await mutate(async () => {
      const result = await api.checked<{ job: Job }>("storycut_job_cancel", { job_id: job.job_id, idempotency_key: idempotencyKey("job-cancel") });
      setJob(jobOf(result.data));
    });
  }

  const filteredAssets = useMemo(() => assets.filter((asset) => (filter === "all" || asset.kind === filter) && asset.path.toLocaleLowerCase().includes(query.toLocaleLowerCase())), [assets, filter, query]);
  const selectedClipData = timeline?.clips.find((clip) => clip.id === selectedClip) ?? null;
  const selectedAssetData = assets.find((asset) => asset.id === (selectedClipData?.asset_id ?? selectedAsset)) ?? null;
  const contentSeconds = timeline ? Math.max(0, timeline.duration_ticks / TIMEBASE) : 0;
  const visibleSeconds = Math.max(contentSeconds + 3, 18);
  const pxPerSecond = scale;
  const timelineWidth = visibleSeconds * pxPerSecond;
  const rulerInterval = rulerStep(pxPerSecond);
  const atTime = formatTime(playhead / TIMEBASE);

  function commitNumericSeek() {
    const value = Number(seekSeconds);
    if (!Number.isFinite(value) || value < 0) { setError("播放頭時間請輸入 0 或更大的秒數。"); return; }
    const frame = frameTicks(project);
    seekTo(snapTick(Math.min(contentSeconds, value) * TIMEBASE, project?.canvas.fps.num ?? 30, project?.canvas.fps.den ?? 1));
    setSeekSeconds((Math.min(contentSeconds, value) * TIMEBASE / frame * frame / TIMEBASE).toFixed(3));
  }

  function tickFromClientX(clientX: number) {
    const bounds = timeScroll.current?.getBoundingClientRect();
    const left = (bounds?.left ?? 0) + 216;
    const horizontal = timeScroll.current?.scrollLeft ?? 0;
    return Math.max(0, ((clientX - left + horizontal) / pxPerSecond) * TIMEBASE);
  }

  function onTimelineDrop(event: DragEvent<HTMLDivElement>, track: Track) {
    event.preventDefault();
    if (track.locked) return;
    const assetId = event.dataTransfer.getData("application/x-storycut-asset");
    if (assetId) { void addAssetClip(assetId, tickFromClientX(event.clientX), track); return; }
    const clipId = event.dataTransfer.getData("application/x-storycut-clip");
    if (!clipId) return;
    const clip = timeline?.clips.find((item) => item.id === clipId);
    if (clip) void moveClip(clip, tickFromClientX(event.clientX), track);
    setDragClipId(null);
  }

  function beginTrim(event: ReactPointerEvent<HTMLButtonElement>, clip: Clip, edge: "left" | "right") {
    event.preventDefault(); event.stopPropagation();
    if (busy) return;
    if (timeline?.tracks.find((item) => item.id === clip.track_id)?.locked) return;
    setTrimState({ clipId: clip.id, edge, origin: event.clientX, start: clip.start_tick, duration: clip.duration_ticks, original: clip });
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function updateTrim(event: ReactPointerEvent<HTMLButtonElement>) {
    if (busy) return;
    if (!trimState) return;
    const clip = timeline?.clips.find((item) => item.id === trimState.clipId);
    if (!clip) return;
    const delta = ((event.clientX - trimState.origin) / pxPerSecond) * TIMEBASE;
    const frame = project ? TIMEBASE * project.canvas.fps.den / project.canvas.fps.num : TIMEBASE / 30;
    const d = Math.round(delta / frame) * frame;
    const previewStart = trimState.edge === "left" ? Math.max(0, trimState.start + d) : trimState.start;
    const previewDuration = trimState.edge === "left" ? Math.max(frame, trimState.duration - d) : Math.max(frame, trimState.duration + d);
    setTimeline((current) => current && ({ ...current, clips: current.clips.map((item) => item.id === clip.id ? { ...item, start_tick: previewStart, duration_ticks: previewDuration } : item) }));
  }

  async function endTrim() {
    if (!trimState) return;
    const clip = timelineRef.current?.clips.find((item) => item.id === trimState.clipId);
    if (clip && (clip.start_tick !== trimState.start || clip.duration_ticks !== trimState.duration)) await trimClip(trimState.original, clip.start_tick, clip.duration_ticks);
    setTrimState(null);
  }

  function seekFromEvent(event: ReactPointerEvent<HTMLDivElement>) {
    if (busy) return;
    if (trimState || dragClipId) return;
    if ((event.target as HTMLElement).closest(".clip-card")) return;
    seekTo(tickFromClientX(event.clientX));
  }

  const toolbarDisabled = !project || busy;

  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand"><span className="brand-mark">S</span><span>STORYCUT</span><span className="brand-sep">/</span><span className="brand-context">工作室</span></div>
        <div className="project-title"><span className={`status-dot ${mode}`} />{project?.name ?? "尚未開啟專案"}<span className="revision">{project ? `v${project.revision}` : "本機剪輯"}</span></div>
        <div className="top-actions">
          <button className="icon-btn" title="復原（共用歷史）" disabled={toolbarDisabled || mode === "demo" || !supportsTool("storycut_history_undo")} onClick={() => void undoRedo("undo")}><Icon name="undo" /></button>
          <button className="icon-btn" title="重做（共用歷史）" disabled={toolbarDisabled || mode === "demo" || !supportsTool("storycut_history_redo")} onClick={() => void undoRedo("redo")}><Icon name="redo" /></button>
          <span className="top-divider" />
          <button className="button button-quiet" disabled={busy || mode === "demo"} onClick={() => void openProject()}><Icon name="folder" /> 開啟</button>
          <button className="button button-quiet" disabled={busy || mode === "demo"} onClick={() => void createProject()}><Icon name="plus" /> 新專案</button>
          {mode === "tauri" && project && <button className="button button-quiet sync-project-button" disabled={busy} title="從共用核心重新讀取目前修訂" onClick={() => void synchronizeProject()}>同步工程</button>}
          <button className="button button-primary" disabled={!project || busy} onClick={() => void startRender()}><Icon name="render" /> 匯出</button>
        </div>
      </header>

      {mode === "demo" && <div className="demo-banner"><span>互動預覽</span> 目前是示範資料，任何剪輯都不會寫入專案或產生影片。<button onClick={() => { setMode("unavailable"); setProject(null); setTimeline(null); setAssets([]); setSelectedClip(null); }}>離開預覽</button></div>}
      {error && <div className="error-banner"><strong>核心操作失敗</strong><span>{error}</span><button onClick={() => setError(null)} aria-label="關閉"><Icon name="close" /></button></div>}

      <section className="workspace-grid">
        <aside className="asset-panel panel">
          <div className="panel-heading"><div><span className="eyebrow">PROJECT BIN</span><h2>素材庫 <span className="count-pill">{assets.length}</span></h2></div><button className="small-square" disabled={!project || busy || mode === "tauri" && !supportsTool("storycut_media_import")} title={mode === "tauri" && !supportsTool("storycut_media_import") ? "核心尚未實作 media_import" : "匯入工作區內媒體"} onClick={() => void importAssets()}><Icon name="plus" /></button></div>
          {!project ? <div className="asset-empty"><div className="empty-orbit"><Icon name="folder" size={22} /></div><strong>從一個故事開始</strong><p>開啟既有專案，或建立新專案，再匯入你的影像與聲音。</p><button className="button button-primary" disabled={mode === "demo"} onClick={() => void createProject()}>建立專案</button>{mode === "unavailable" && <button className="text-button" onClick={openDemo}>開啟介面預覽</button>}</div> : <>
            <div className="bin-toolbar"><label className="search-field"><Icon name="search" size={14} /><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="搜尋素材" /></label><select aria-label="素材類型篩選" value={filter} onChange={(event) => setFilter(event.target.value as typeof filter)}><option value="all">全部</option><option value="video">影片</option><option value="image">圖片</option><option value="audio">聲音</option></select></div>
            <div className="asset-list">{filteredAssets.map((asset) => <AssetItem key={asset.id} asset={asset} selected={selectedAsset === asset.id} canAdd={mode === "demo" || supportsOperation("clip.add")} onSelect={() => { setSelectedAsset(asset.id); setSelectedClip(null); }} onAdd={() => void addAssetClip(asset.id)} onDragStart={(event) => event.dataTransfer.setData("application/x-storycut-asset", asset.id)} />)}{filteredAssets.length === 0 && <div className="empty-filter">{assets.length ? "找不到符合的素材" : "素材庫目前是空的"}<button className="button button-quiet" disabled={mode === "tauri" && !supportsTool("storycut_media_import")} onClick={() => void importAssets()}><Icon name="plus" /> 匯入素材</button></div>}</div>
          <div className="bin-footer" title="素材必須位於專案資料夾或其子資料夾中；不會自動複製素材"><span><span className="green-led" /> 工作區內媒體</span><span>{assets.filter((asset) => asset.sha256).length} 已驗證</span></div>
          </>}
        </aside>

        <section className="center-column">
          <div className="preview-panel panel">
            <div className="preview-head"><div><span className="eyebrow">PROGRAM MONITOR</span><h2>預覽</h2></div><div className="preview-meta"><span className="quality-dot" />{project ? `${project.canvas.width} × ${project.canvas.height} · ${project.canvas.fps.num / project.canvas.fps.den} fps` : "等待專案"}<button className="preview-fit">適合視窗 <Icon name="chevron" size={13} /></button></div></div>
            <div className="preview-stage-wrap"><div className="preview-stage" style={{ aspectRatio: project ? `${project.canvas.width}/${project.canvas.height}` : "16/9" }}>
              <div className="stage-noise" />
              {!project ? <div className="preview-idle"><div className="idle-frame"><span>STORYCUT</span><span>FILM EDITOR</span></div><p>開啟專案，開始編輯時間軸</p></div> : previewState === "rendering" ? <div className="preview-idle"><div className="loader-ring" /><p>共用核心正在同步渲染影片預覽…</p><span>最多 6 秒 · 修訂版 {project.revision} · 完成前需等待，無法取消</span></div> : previewVideoUrl && previewRange ? <><video ref={previewVideo} className="range-preview" src={previewVideoUrl} controls playsInline aria-label="StoryCut 共用核心產生的影片預覽" onPlay={() => setPlaying(true)} onPause={() => setPlaying(false)} onTimeUpdate={(event) => { const video = event.currentTarget; setPlayhead(Math.min(previewRange.startTick + previewRange.durationTicks, previewRange.startTick + video.currentTime * TIMEBASE)); }} onEnded={() => { setPlaying(false); setPlayhead(previewRange.startTick + previewRange.durationTicks); }} /><div className="artifact-notice"><Icon name="check" /> 核心影片預覽 · {previewVideoArtifact?.split(/[\\/]/).pop()}</div><div className="frame-caption">工作 {job?.job_id} · 修訂版 {job?.source_revision} · {formatTime(previewRange.startTick / TIMEBASE)} 起</div></> : previewState === "ready" && previewUrl ? <><img className="frame-preview" src={previewUrl} alt={`核心產生的 ${atTime} 預覽影格`} /><div className="artifact-notice"><Icon name="check" /> 核心影格 · {previewArtifact?.split(/[\\/]/).pop()}</div><div className="frame-caption">工作 {job?.job_id} · 修訂版 {job?.source_revision}</div></> : <div className="preview-idle"><div className="play-emblem"><Icon name="play" size={24} /></div><p>{previewState === "unavailable" ? "核心預覽目前不可用" : selectedClipData ? selectedAssetData?.path : "尚未產生核心預覽影格"}</p><span>{atTime} / {formatTime(contentSeconds)}</span>{mode === "tauri" && <button className="button button-quiet" disabled={!project || busy || !supportsTool("storycut_preview_frame")} title={!supportsTool("storycut_preview_frame") ? "核心尚未實作 preview_frame" : "由共用核心產生此播放頭影格"} onClick={() => void requestPreview()}>產生精確影格</button>}{mode === "tauri" && <button className="button button-quiet" disabled={!project || busy || !supportsTool("storycut_preview_range")} title={!supportsTool("storycut_preview_range") ? "核心尚未實作 preview_range" : "同步渲染最多 6 秒 MP4 預覽"} onClick={() => void generateRangePreview()}>產生影片預覽</button>}</div>}
              <div className="safe-guide" />
              <span className="stage-corner top-left" /><span className="stage-corner top-right" /><span className="stage-corner bottom-left" /><span className="stage-corner bottom-right" />
            </div></div>
            <div className="transport"><div className="time-readout"><span>{atTime.slice(0, 8)}</span><span className="time-millis">.{atTime.slice(9)}</span><span className="time-slash">/</span><span className="duration-readout">{formatTime(contentSeconds).slice(0, 8)}</span></div><div className="transport-buttons"><button className="transport-skip" disabled={!timeline || busy} title="往前一格" onClick={() => seekTo(playhead - frameTicks(project))}>‹</button><button className={`transport-play ${playing ? "is-playing" : ""}`} disabled={!project || busy || mode === "unavailable" || mode === "tauri" && !supportsTool("storycut_preview_range")} title={mode === "demo" ? (playing ? "暫停示範播放頭" : "播放示範播放頭") : mode === "tauri" ? previewVideoUrl ? (playing ? "暫停核心影片預覽" : "播放核心影片預覽") : "同步產生最多 6 秒核心影片預覽；需等待渲染完成" : "請開啟 StoryCut 桌面版以連接共用核心"} onClick={() => void togglePlayback()}><Icon name={playing ? "pause" : "play"} size={17} /></button><button className="transport-skip" disabled={!timeline || busy} title="往後一格" onClick={() => seekTo(playhead + frameTicks(project))}>›</button></div><label className="transport-seek"><span>定位秒數</span><input aria-label="以秒定位播放頭" type="number" min="0" max={contentSeconds} step="0.001" value={seekSeconds} disabled={!timeline || busy} onChange={(event) => setSeekSeconds(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") commitNumericSeek(); }} /><button type="button" disabled={!timeline || busy} onClick={commitNumericSeek}>前往</button></label><div className="transport-info"><span className="fps-chip">{project ? `${project.canvas.fps.num}/${project.canvas.fps.den} fps` : "—"}</span></div></div>
          </div>

          <section className="timeline-panel panel">
            <div className="timeline-toolbar"><div className="timeline-title"><div><span className="eyebrow">EDIT SEQUENCE</span><h2>{timelineView === "assembly" ? "說書排片" : "時間軸"}</h2></div><div className="sequence-view-tabs" role="tablist" aria-label="編輯工作區"><button type="button" role="tab" aria-selected={timelineView === "timeline"} className={timelineView === "timeline" ? "active" : ""} onClick={() => setTimelineView("timeline")}>時間軸</button><button type="button" role="tab" aria-selected={timelineView === "assembly"} className={timelineView === "assembly" ? "active" : ""} disabled={!project} onClick={() => setTimelineView("assembly")}>故事組裝</button></div><span className="sequence-badge">{project?.canvas.width ?? 1920} × {project?.canvas.height ?? 1080}</span></div><div className="timeline-tools" hidden={timelineView !== "timeline"}><button className="icon-btn" title="分割所選片段（共用核心）" disabled={!selectedClipData || toolbarDisabled || !supportsOperation("clip.split")} onClick={() => void splitAtPlayhead()}><Icon name="scissors" /></button><span className="top-divider" /><span className="zoom-label">縮放</span><button className="zoom-button" title="縮小時間軸" onClick={() => setScale((value) => Math.max(0.5, value - (value < 8 ? 0.5 : 8)))}>−</button><input className="zoom-slider" aria-label="時間軸縮放" type="range" min="0.5" max="148" step="0.5" value={scale} onChange={(event) => setScale(Number(event.target.value))} /><button className="zoom-button" title="放大時間軸" onClick={() => setScale((value) => Math.min(148, value + (value < 8 ? 0.5 : 8)))}>+</button><button className="zoom-reset" onClick={fitTimeline}>適合</button><span className="zoom-value">{Math.round(scale / 68 * 100)}%</span></div></div>
            {!timeline ? <div className="timeline-empty"><div className="timeline-empty-mark">00:00</div><strong>時間軸尚未載入</strong><p>開啟專案後，所有片段與軌道會從 StoryCut 核心讀取。</p><button className="button button-quiet" onClick={openDemo}>查看介面預覽</button></div> : <>
            <div className="timeline-view-body" hidden={timelineView !== "timeline"}><div className="timeline-main" ref={timeScroll}>
              <div className="timeline-labels"><div className="track-header"><span>軌道</span><button className="small-square" disabled={!supportsOperation("track.add")} title={supportsOperation("track.add") ? "新增軌道" : "核心尚未實作 track.add"} onClick={() => document.getElementById("add-track-menu")?.classList.toggle("visible")}><Icon name="plus" size={14} /></button><div className="add-track-menu" id="add-track-menu">{(["video", "image", "audio", "subtitle"] as TrackKind[]).map((kind) => <button key={kind} onClick={() => { document.getElementById("add-track-menu")?.classList.remove("visible"); void addTrack(kind); }}><Icon name={KIND_ICON[kind]} size={14} /> 新增{KIND_LABEL[kind]}軌</button>)}</div></div>{timeline.tracks.map((track) => <TrackLabel key={track.id} track={track} canUpdate={mode === "demo" || supportsOperation("track.update")} onUpdateName={(name) => void updateTrack(track, { name })} onUpdateGain={(gain_db) => void updateTrack(track, { gain_db })} onToggleLock={() => void updateTrack(track, { locked: !track.locked })} onToggleMute={() => void updateTrack(track, { muted: !track.muted })} onToggleSolo={() => void updateTrack(track, { solo: !track.solo })} onToggleEnabled={() => void updateTrack(track, { enabled: !track.enabled })} />)}<div className="track-add-row"><button disabled={!supportsOperation("track.add")} onClick={() => document.getElementById("add-track-menu")?.classList.toggle("visible")}><Icon name="plus" size={13} /> 新增軌道</button></div></div>
              <div className="timeline-canvas" style={{ width: timelineWidth, ["--px-per-second" as string]: `${pxPerSecond}px` }}>
                <div className="ruler" onPointerDown={seekFromEvent} style={{ width: timelineWidth }}><div className="ruler-base" />{Array.from({ length: Math.ceil(visibleSeconds / rulerInterval) + 1 }, (_, index) => index * rulerInterval).map((sec) => <div key={sec} className="ruler-tick" style={{ left: sec * pxPerSecond }}><span>{formatTime(sec)}</span></div>)}</div>
                <div className="playhead-line" style={{ left: (playhead / TIMEBASE) * pxPerSecond }} />
                {timeline.tracks.map((track) => <TrackLane key={track.id} track={track} clips={timeline.clips.filter((clip) => clip.track_id === track.id)} assets={assets} selectedClip={selectedClip} dragClipId={dragClipId} scale={pxPerSecond} timelineWidth={timelineWidth} onClipSelect={(clip) => { setSelectedClip(clip.id); setSelectedAsset(clip.asset_id); }} onClipDragStart={(clip) => { setDragClipId(clip.id); }} onClipDragEnd={() => setDragClipId(null)} onDrop={(event) => onTimelineDrop(event, track)} onDragOver={(event) => event.preventDefault()} onTrimStart={beginTrim} onTrimMove={updateTrim} onTrimEnd={() => void endTrim()} onPointerDown={seekFromEvent} />)}
                <div className="timeline-tail" style={{ left: timelineWidth - 8 }}>片尾</div>
              </div>
            </div></div>
            {project && <div className="assembly-view-body" hidden={timelineView !== "assembly"}><StoryAssemblyPanel key={`${project.project_id}:${api.workspacePath ?? ""}`} projectId={project.project_id} revision={project.revision} workspace={api.workspacePath} assets={assets} tracks={timeline.tracks} mode={mode} busy={busy} supported={mode === "tauri" && implementedTools.includes("storycut_storyboard_assemble")} onAssemble={(args, dryRun, request) => runHighLevel<StoryboardResult>("storycut_storyboard_assemble", args as unknown as Record<string, unknown>, dryRun, request)} /></div>}
            </>}
            <div className="timeline-status"><div><span className="green-led" /> {project ? `修訂版 ${project.revision} · ${timeline?.tracks.length ?? 0} 軌道 · ${timeline?.clips.length ?? 0} 片段` : "共用核心待連線"}</div><div>{mode === "demo" ? "預覽資料 · 不會保存" : project ? timelineView === "assembly" ? "核心規劃 · dry-run 後明確提交" : "拖曳以移動 · 拖曳片段兩端以修剪" : "—"}</div></div>
          </section>
        </section>

        <aside className="inspector-panel panel">
          <div className="panel-heading inspector-heading"><div><span className="eyebrow">INSPECTOR</span><h2>屬性</h2></div><span className="inspector-kicker">{selectedClipData ? "CLIP" : selectedAssetData ? "MEDIA" : "PROJECT"}</span></div>
          {!project ? <div className="inspector-empty"><div className="inspector-empty-icon"><Icon name="image" size={21} /></div><strong>尚未選取項目</strong><p>開啟一個專案，或選取素材與片段來查看資訊。</p></div> : selectedClipData ? <div className="inspector-content inspector-combined"><ClipInspector clip={selectedClipData} asset={selectedAssetData} track={timeline?.tracks.find((item) => item.id === selectedClipData.track_id)} onChange={(patch) => void trimClip(selectedClipData, selectedClipData.start_tick, patch.duration_ticks ?? selectedClipData.duration_ticks)} onAudioChange={(audio) => void updateAudioClip(selectedClipData, audio)} sampleRate={project.audio_sample_rate} />{selectedClipData.kind === "image" && selectedAssetData?.kind === "image" && <ImageMotionInspector key={`${project.project_id}:${api.workspacePath ?? ""}:${selectedClipData.id}`} projectId={project.project_id} revision={project.revision} workspace={api.workspacePath} clip={selectedClipData} asset={selectedAssetData} assets={assets} imageClips={(timeline?.clips ?? []).filter((clip) => clip.kind === "image" && timeline?.tracks.some((track) => track.id === clip.track_id && !track.locked))} mode={mode} busy={busy} supported={mode === "tauri" && implementedTools.includes("storycut_focal_motion_apply")} onApply={(args, dryRun, request) => runHighLevel("storycut_focal_motion_apply", args as unknown as Record<string, unknown>, dryRun, request)} />}</div> : selectedAssetData ? <AssetInspector asset={selectedAssetData} onAdd={() => void addAssetClip(selectedAssetData.id)} /> : <ProjectInspector project={project} timeline={timeline} onAddDefaults={() => void addStarterTracks()} canAddDefaults={mode === "demo" || supportsOperation("track.add")} />}
          <div className="inspector-bottom"><div className="render-card"><div className="render-card-top"><div className="render-icon"><Icon name="render" size={15} /></div><div><strong>輸出設定</strong><span>H.264 · AAC · MP4</span></div></div><label className="field-label">字幕處理<select disabled={!project} value={subtitleMode} onChange={(event) => setSubtitleMode(event.target.value as typeof subtitleMode)}><option value="none">不燒錄字幕</option><option value="burn">燒錄字幕</option></select></label><label className="field-label">編碼器<select disabled={!project} value={encoder} onChange={(event) => setEncoder(event.target.value as typeof encoder)}><option value="h264_cpu">H.264 · CPU</option><option value="h264_nvenc">H.264 · NVIDIA NVENC</option></select></label><button className="button button-primary render-button" disabled={!project || busy || mode === "tauri" && !supportsTool("storycut_render_start")} title={mode === "tauri" && !supportsTool("storycut_render_start") ? "核心尚未實作 render_start" : "匯出影片"} onClick={() => void startRender()}><Icon name="render" /> 開始匯出</button></div>{job && <div className="job-card"><div className="job-title"><strong>{job.kind === "render" ? "影片輸出工作" : "預覽工作"}</strong><span className={`job-state ${job.state}`}>{job.state}</span></div><div className="job-meta">來源修訂 v{job.source_revision} · {job.job_id}</div><div className="progress-track"><i style={{ width: `${Math.round((job.progress ?? (job.state === "succeeded" ? 1 : 0)) * 100)}%` }} /></div><div className="job-actions"><span>{job.progress == null ? "等待核心狀態" : `${Math.round(job.progress * 100)}%`}</span><button onClick={() => void refreshJob()}>更新狀態</button>{["queued", "running", "cancelling"].includes(job.state) && <button onClick={() => void cancelJob()}>取消</button>}</div></div>}</div>
        </aside>
      </section>

      <input id="demo-file-input" type="file" multiple accept="image/*,video/*,audio/*" hidden onChange={(event) => { void importDemoFiles(event.target.files); event.currentTarget.value = ""; }} />
      {toast && <div role="status" className="toast"><span className="toast-check"><Icon name={error ? "close" : "check"} size={14} /></span>{toast}</div>}
      {busy && <div className="busy-indicator"><span /> 正在與共用核心同步…</div>}
    </main>
  );
}

function TrackLabel({ track, canUpdate, onUpdateName, onUpdateGain, onToggleLock, onToggleMute, onToggleSolo, onToggleEnabled }: { track: Track; canUpdate: boolean; onUpdateName: (name: string) => void; onUpdateGain: (gain: number) => void; onToggleLock: () => void; onToggleMute: () => void; onToggleSolo: () => void; onToggleEnabled: () => void }) {
  const [name, setName] = useState(track.name);
  const [gain, setGain] = useState(track.gain_db);
  useEffect(() => { setName(track.name); setGain(track.gain_db); }, [track.id, track.name, track.gain_db]);
  function commitName() { const value = name.trim(); if (!value) { setName(track.name); return; } if (canUpdate && !track.locked && value !== track.name) onUpdateName(value); }
  function commitGain() { if (canUpdate && !track.locked && Number.isFinite(gain) && gain !== track.gain_db) onUpdateGain(gain); }
  return <div className={`track-label ${track.kind} ${track.locked ? "is-locked" : ""}`}><div className="track-name-row"><span className={`track-kind ${track.kind}`}><Icon name={KIND_ICON[track.kind]} size={12} /></span><input aria-label={`${track.name} 軌道名稱`} className="track-name-editor" value={name} maxLength={200} disabled={!canUpdate || track.locked} title={canUpdate ? "修改軌道名稱" : "核心尚未實作 track.update"} onChange={(event) => setName(event.target.value)} onBlur={commitName} onKeyDown={(event) => { if (event.key === "Enter") event.currentTarget.blur(); }} /><button className={`track-eye ${track.enabled ? "active" : ""}`} title={track.enabled ? "隱藏軌道" : "顯示軌道"} onClick={onToggleEnabled}><Icon name="eye" size={13} /></button></div><div className="track-controls"><span className="track-index">{track.kind === "video" ? "V" : track.kind === "image" ? "I" : track.kind === "audio" ? "A" : "S"}</span>{track.kind === "audio" && <><button className={track.muted ? "control-active" : ""} title={track.muted ? "取消靜音" : "靜音"} onClick={onToggleMute}>M</button><button className={track.solo ? "solo-active" : ""} title={track.solo ? "取消獨奏" : "獨奏"} onClick={onToggleSolo}>S</button><label className="track-gain" title={`軌道增益 ${gain} dB`}><span>{gain}dB</span><input aria-label={`${track.name} 軌道增益 dB`} type="range" min="-48" max="12" step="1" value={gain} disabled={!canUpdate || track.locked} onChange={(event) => setGain(Number(event.target.value))} onPointerUp={commitGain} onBlur={commitGain} /></label></>}<button className={track.locked ? "control-active" : ""} title={track.locked ? "解除軌道鎖定" : "鎖定軌道"} onClick={onToggleLock}><Icon name={track.locked ? "lock" : "unlock"} size={12} /></button></div></div>;
}

function AssetItem({ asset, selected, canAdd, onSelect, onAdd, onDragStart }: { asset: Asset; selected: boolean; canAdd: boolean; onSelect: () => void; onAdd: () => void; onDragStart: (event: DragEvent<HTMLDivElement>) => void }) {
  return <div className={`asset-item ${selected ? "selected" : ""}`} onClick={onSelect} draggable={canAdd} onDragStart={onDragStart}>
    <div className={`asset-thumb ${asset.kind}`} aria-hidden="true"><div className="thumb-object">{asset.kind === "video" ? <Icon name="play" size={15} /> : <Icon name={KIND_ICON[asset.kind]} size={17} />}</div><span className="media-duration">{asset.kind === "image" ? "IMAGE" : formatTime((asset.duration_ticks ?? 0) / TIMEBASE)}</span></div>
    <div className="asset-info"><div className="asset-name" title={asset.path}>{asset.path.split(/[\\/]/).pop()}</div><div className="asset-meta"><Icon name={KIND_ICON[asset.kind]} size={12} /><span>{KIND_LABEL[asset.kind]}</span><span className="meta-dot">·</span><span>{asset.probe_status === "probed" ? "已檢查" : asset.probe_status === "unprobed" ? "未探測" : "離線"}</span></div></div>
    <button className="asset-add" title={canAdd ? "加入時間軸" : "核心尚未實作 clip.add"} disabled={!canAdd} onClick={(event) => { event.stopPropagation(); onAdd(); }}><Icon name="plus" size={15} /></button>
  </div>;
}

function TrackLane({ track, clips, assets, selectedClip, dragClipId, scale, timelineWidth, onClipSelect, onClipDragStart, onClipDragEnd, onDrop, onDragOver, onTrimStart, onTrimMove, onTrimEnd, onPointerDown }: {
  track: Track; clips: Clip[]; assets: Asset[]; selectedClip: string | null; dragClipId: string | null; scale: number; timelineWidth: number;
  onClipSelect: (clip: Clip) => void; onClipDragStart: (clip: Clip) => void; onClipDragEnd: () => void; onDrop: (event: DragEvent<HTMLDivElement>) => void; onDragOver: (event: DragEvent<HTMLDivElement>) => void;
  onTrimStart: (event: ReactPointerEvent<HTMLButtonElement>, clip: Clip, edge: "left" | "right") => void; onTrimMove: (event: ReactPointerEvent<HTMLButtonElement>) => void; onTrimEnd: () => void; onPointerDown: (event: ReactPointerEvent<HTMLDivElement>) => void;
}) {
  return <div className={`track-lane ${track.kind} ${track.enabled ? "" : "disabled"} ${track.locked ? "locked" : ""}`} style={{ width: timelineWidth }} onDrop={onDrop} onDragOver={onDragOver} onPointerDown={onPointerDown}>
    <div className="lane-grid" />
    {clips.map((clip) => {
      const asset = assets.find((item) => item.id === clip.asset_id);
      const startX = clip.start_tick / TIMEBASE * scale;
      const width = Math.max(Math.min(38, Math.max(5, scale * 10)), clip.duration_ticks / TIMEBASE * scale);
      const style = { left: startX, width, opacity: track.enabled ? 1 : 0.38 } as CSSProperties;
      return <div key={clip.id} className={`clip-card ${clip.kind} ${width < 78 ? "compact" : ""} ${selectedClip === clip.id ? "selected" : ""} ${track.locked ? "locked" : ""} ${dragClipId === clip.id ? "dragging" : ""}`} title={`${asset?.path.split(/[\\/]/).pop() ?? clip.id} · ${formatTime(clip.start_tick / TIMEBASE)} · ${formatTime(clip.duration_ticks / TIMEBASE)}`} style={style} draggable={!track.locked} onClick={(event) => { event.stopPropagation(); onClipSelect(clip); }} onDragStart={(event) => { event.dataTransfer.setData("application/x-storycut-clip", clip.id); onClipDragStart(clip); }} onDragEnd={onClipDragEnd} onDoubleClick={() => onClipSelect(clip)}>
        <button className="trim-handle left" aria-label="修剪片段起點" onPointerDown={(event) => onTrimStart(event, clip, "left")} onPointerMove={onTrimMove} onPointerUp={onTrimEnd} />
        {clip.kind === "audio" ? <><span className="audio-strip-tag">AUDIO STRIP</span><span className="clip-label">{asset?.path.split(/[\\/]/).pop()}</span></> : <><div className={`clip-image ${clip.kind}`}><Icon name={clip.kind === "video" ? "play" : "image"} size={11} /></div><span className="clip-label">{asset?.path.split(/[\\/]/).pop()}</span><span className="clip-subtitle">{formatTime(clip.duration_ticks / TIMEBASE)} · {clip.kind === "video" ? "VIDEO" : "IMAGE"}</span></>}
        {track.locked && <span className="clip-lock"><Icon name="lock" size={11} /></span>}
        <button className="trim-handle right" aria-label="修剪片段結尾" onPointerDown={(event) => onTrimStart(event, clip, "right")} onPointerMove={onTrimMove} onPointerUp={onTrimEnd} />
      </div>;
    })}
  </div>;
}

function ClipInspector({ clip, asset, track, onChange, onAudioChange, sampleRate }: { clip: Clip; asset: Asset | null; track?: Track; onChange: (patch: Partial<Clip>) => void; onAudioChange: (audio: AudioSettings) => void; sampleRate: number }) {
  const [duration, setDuration] = useState((clip.duration_ticks / TIMEBASE).toFixed(2));
  useEffect(() => setDuration((clip.duration_ticks / TIMEBASE).toFixed(2)), [clip.id, clip.duration_ticks]);
  const [audioGain, setAudioGain] = useState((clip.audio?.gain_db ?? 0).toString());
  const [audioFadeIn, setAudioFadeIn] = useState(((clip.audio?.fade_in_ticks ?? 0) / TIMEBASE).toFixed(2));
  const [audioFadeOut, setAudioFadeOut] = useState(((clip.audio?.fade_out_ticks ?? 0) / TIMEBASE).toFixed(2));
  useEffect(() => {
    setAudioGain((clip.audio?.gain_db ?? 0).toString());
    setAudioFadeIn(((clip.audio?.fade_in_ticks ?? 0) / TIMEBASE).toFixed(2));
    setAudioFadeOut(((clip.audio?.fade_out_ticks ?? 0) / TIMEBASE).toFixed(2));
  }, [clip.id, clip.audio?.gain_db, clip.audio?.fade_in_ticks, clip.audio?.fade_out_ticks, clip.audio?.muted]);
  function sampleAlignedTicks(value: string) {
    const seconds = Number(value);
    if (!Number.isFinite(seconds) || seconds < 0) return null;
    const sampleTicks = TIMEBASE / sampleRate;
    return Number.isInteger(sampleTicks) ? Math.round(seconds * sampleRate) * sampleTicks : Math.round(seconds * TIMEBASE);
  }
  function saveAudio(patch: Partial<AudioSettings>) {
    if (!clip.audio) return;
    const gain = Number(audioGain);
    const fadeIn = sampleAlignedTicks(audioFadeIn);
    const fadeOut = sampleAlignedTicks(audioFadeOut);
    if (!Number.isFinite(gain) || gain < -96 || gain > 24 || fadeIn === null || fadeOut === null || fadeIn + fadeOut > clip.duration_ticks) return;
    onAudioChange({ ...clip.audio, gain_db: gain, fade_in_ticks: fadeIn, fade_out_ticks: fadeOut, ...patch });
  }
  return <div className="clip-inspector-content"><div className="selected-item-card"><div className={`selected-thumb ${clip.kind}`}><Icon name={KIND_ICON[clip.kind]} size={20} /></div><div><span className="eyebrow">{clip.kind.toUpperCase()} CLIP</span><strong>{asset?.path.split(/[\\/]/).pop() ?? clip.id}</strong><span className="muted-text">位於 {track?.name ?? clip.track_id}</span></div></div><div className="inspector-section"><div className="section-title">基本設定 <span>編輯送交共用核心</span></div><div className="two-fields"><label className="field-label">起點<input readOnly value={formatTime(clip.start_tick / TIMEBASE)} /></label><label className="field-label">素材入點<input readOnly value={formatTime(clip.source_in_tick / TIMEBASE)} /></label></div><label className="field-label">片段長度<input value={duration} inputMode="decimal" onChange={(event) => setDuration(event.target.value)} onBlur={() => { const value = Number(duration); if (Number.isFinite(value) && value > 0) onChange({ duration_ticks: Math.round(value * TIMEBASE) }); }} /><small>秒 · 影像以專案幀率對齊</small></label><div className="readout-row"><span>素材狀態</span><span className={`probe-pill ${asset?.probe_status}`}>{asset?.probe_status === "probed" ? "已探測" : asset?.probe_status === "offline" ? "素材離線" : "未探測"}</span></div><div className="readout-row"><span>片段 ID</span><code>{clip.id}</code></div>{clip.kind === "video" && <div className="readout-row"><span>原聲</span><span>{clip.audio_policy === "separate_linked" ? "已連結音軌" : "靜音"}</span></div>}</div>{clip.kind === "audio" && clip.audio && <div className="inspector-section"><div className="section-title">片段音訊 <span>獨立音訊處理</span></div><label className="field-label">片段增益 dB<input type="number" min="-96" max="24" step="1" value={audioGain} onChange={(event) => setAudioGain(event.target.value)} onBlur={() => saveAudio({})} /></label><div className="two-fields"><label className="field-label">淡入秒數<input type="number" min="0" step="0.1" value={audioFadeIn} onChange={(event) => setAudioFadeIn(event.target.value)} onBlur={() => saveAudio({})} /></label><label className="field-label">淡出秒數<input type="number" min="0" step="0.1" value={audioFadeOut} onChange={(event) => setAudioFadeOut(event.target.value)} onBlur={() => saveAudio({})} /></label></div><label className="audio-mute-check"><input type="checkbox" checked={clip.audio.muted} onChange={(event) => saveAudio({ muted: event.currentTarget.checked })} />靜音此片段</label></div>}<div className="inspector-section"><div className="section-title">時間位置 <span>timebase 705.6 MHz</span></div><div className="position-bar"><i style={{ left: `${Math.min(95, clip.start_tick / Math.max(1, clip.start_tick + clip.duration_ticks) * 100)}%` }} /><b style={{ width: `${Math.min(80, clip.duration_ticks / Math.max(clip.start_tick + clip.duration_ticks, clip.duration_ticks) * 100)}%` }} /></div><div className="position-ends"><span>{formatTime(clip.start_tick / TIMEBASE)}</span><span>{formatTime((clip.start_tick + clip.duration_ticks) / TIMEBASE)}</span></div></div></div>;
}

function AssetInspector({ asset, onAdd }: { asset: Asset; onAdd: () => void }) {
  return <div className="inspector-content"><div className={`asset-inspector-art ${asset.kind}`}><div className="art-light" /><Icon name={KIND_ICON[asset.kind]} size={24} /><span>{asset.kind === "image" ? "STATIC IMAGE" : asset.kind === "video" ? "VIDEO SOURCE" : "AUDIO SOURCE"}</span></div><div className="inspector-section"><div className="section-title">素材資訊</div><div className="readout-row"><span>類型</span><span>{KIND_LABEL[asset.kind]}</span></div><div className="readout-row"><span>長度</span><span>{asset.duration_ticks ? formatTime(asset.duration_ticks / TIMEBASE) : "靜態圖片"}</span></div><div className="readout-row"><span>媒體探測</span><span className={`probe-pill ${asset.probe_status}`}>{asset.probe_status === "probed" ? "已探測" : "未探測"}</span></div><div className="readout-row vertical"><span>來源路徑</span><code title={asset.path}>{asset.path}</code></div>{asset.streams.map((stream) => <div className="readout-row" key={stream.index}><span>Stream {stream.index}</span><span>{stream.kind}{stream.sample_rate ? ` · ${stream.sample_rate / 1000} kHz` : ""}</span></div>)}</div><button className="button button-primary inspector-add" onClick={onAdd}><Icon name="plus" /> 加入時間軸</button></div>;
}

function ProjectInspector({ project, timeline, onAddDefaults, canAddDefaults }: { project: Project; timeline: Timeline | null; onAddDefaults: () => void; canAddDefaults: boolean }) {
  const video = timeline?.tracks.filter((track) => track.kind === "video" || track.kind === "image").length ?? 0;
  const audio = timeline?.tracks.filter((track) => track.kind === "audio").length ?? 0;
  return <div className="inspector-content"><div className="project-art"><div className="project-art-bars"><i /><i /><i /><i /><i /></div><span>STORYCUT PROJECT</span><strong>{project.name}</strong></div><div className="inspector-section"><div className="section-title">專案摘要</div><div className="readout-row"><span>畫布</span><span>{project.canvas.width} × {project.canvas.height}</span></div><div className="readout-row"><span>影格率</span><span>{project.canvas.fps.num}/{project.canvas.fps.den} fps</span></div><div className="readout-row"><span>內容長度</span><span>{formatTime((timeline?.duration_ticks ?? 0) / TIMEBASE)}</span></div><div className="readout-row"><span>媒體</span><span>{project.assets.length} 項</span></div><div className="readout-row"><span>畫面／聲音軌</span><span>{video} / {audio}</span></div><div className="readout-row"><span>修訂</span><span>v{project.revision}</span></div></div>{timeline?.tracks.length === 0 && <div className="starter-tracks"><strong>空白時間軸</strong><p>預設建立主畫面、圖片疊圖、兩條聲音與字幕軌。</p><button className="button button-quiet" disabled={!canAddDefaults} onClick={onAddDefaults}>建立常用軌道</button>{!canAddDefaults && <small>目前核心尚未宣告 track.add。</small>}</div>}<div className="workspace-note"><span className="green-led" /><div><strong>本機工作區</strong><p>素材須位於專案資料夾內，不會複製或額外授權。</p></div></div></div>;
}

function defaultMotion(duration: number) {
  return { domain_duration_ticks: duration, sample_offset_tick: 0, fit: "cover" as const, avoid_exposed_edges: true, anchor: { x: 0.5, y: 0.5 }, interpolation: "smoothstep" as const, keyframes: [{ tick: 0, x: 0, y: 0, scale: 1, opacity: 1 }, { tick: Math.max(0, duration - 23_520_000), x: 0, y: 0, scale: 1, opacity: 1 }] };
}

function snapTick(tick: number, fpsNum: number, fpsDen: number) {
  const frame = TIMEBASE * fpsDen / fpsNum;
  return Math.max(0, Math.round(tick / frame) * frame);
}

function rulerStep(pxPerSecond: number) {
  return [5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600, 7200].find((seconds) => seconds * pxPerSecond >= 65) ?? 7200;
}

function frameTicks(project: Project | null) { return project ? TIMEBASE * project.canvas.fps.den / project.canvas.fps.num : TIMEBASE / 30; }

function formatTime(seconds: number) {
  const safe = Math.max(0, Number.isFinite(seconds) ? seconds : 0);
  const totalMs = Math.floor(safe * 1000);
  const hh = Math.floor(totalMs / 3_600_000);
  const mm = Math.floor(totalMs / 60_000) % 60;
  const ss = Math.floor(totalMs / 1000) % 60;
  const ms = totalMs % 1000;
  return `${String(hh).padStart(2, "0")}:${String(mm).padStart(2, "0")}:${String(ss).padStart(2, "0")}.${String(ms).padStart(3, "0")}`;
}
