import { useEffect, useMemo, useRef, useState } from "react";
import { idempotencyKey, StoryCutApiError } from "./api";
import type { Asset, BackendMode, Track } from "./types";
import { TIMEBASE } from "./types";

type AudioRole = "narration" | "dialogue" | "music";
type TransitionPlan = { kind: "cut" } | { kind: "cross_dissolve"; duration_seconds: number };
type HighLevelRequest = { projectId: string; workspace: string | null; revision: number; idempotencyKey: string };

export interface StoryboardArguments {
  items: Array<{ asset_id: string; duration_seconds?: number }>;
  visual_track_id?: string;
  transition: TransitionPlan;
  original_video_audio: "separate_linked" | "muted";
  audio_tracks: Array<{
    role: AudioRole;
    track_id?: string;
    track_name?: string;
    clips: Array<{
      asset_id: string;
      start_seconds: number;
      duration_seconds?: number;
      gain_db: number;
      fade_in_seconds: number;
      fade_out_seconds: number;
      loop_to_visual_end?: boolean;
    }>;
  }>;
}

export interface StoryboardResult {
  committed: boolean;
  base_revision: number;
  created_track_ids: string[];
  created_clip_ids: string[];
  visual_duration_ticks: number;
  audio_duration_ticks: number;
  duration_ticks: number;
  transition: unknown;
  track_roles: unknown[];
  warnings: Array<{ code: string; message: string }>;
  diff: unknown;
}

interface AudioDraft {
  selected: boolean;
  role: AudioRole;
  trackId: string;
  trackName: string;
  start: string;
  gain: string;
  fadeIn: string;
  fadeOut: string;
  duration: string;
  loopToVisualEnd: boolean;
}

const ROLE_LABEL: Record<AudioRole, string> = { narration: "旁白", dialogue: "人物對白", music: "配樂" };
const naturalName = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
const basename = (path: string) => path.split(/[\\/]/).pop() ?? path;
const secondsOf = (ticks: number | null | undefined) => typeof ticks === "number" ? ticks / TIMEBASE : null;

export function StoryAssemblyPanel({
  projectId, revision, workspace, assets, tracks, mode, busy, supported, onAssemble,
}: {
  projectId: string;
  revision: number;
  workspace: string | null;
  assets: Asset[];
  tracks: Track[];
  mode: BackendMode;
  busy: boolean;
  supported: boolean;
  onAssemble: (args: StoryboardArguments, dryRun: boolean, request: HighLevelRequest) => Promise<StoryboardResult>;
}) {
  const visuals = useMemo(() => assets.filter((asset) => asset.kind === "image" || asset.kind === "video"), [assets]);
  const audioAssets = useMemo(() => assets.filter((asset) => asset.kind === "audio"), [assets]);
  const [selectedVisuals, setSelectedVisuals] = useState<string[]>([]);
  const [durations, setDurations] = useState<Record<string, string>>({});
  const [manualOrder, setManualOrder] = useState<string[] | null>(null);
  const [transitionKind, setTransitionKind] = useState<"cut" | "cross_dissolve">("cut");
  const [transitionSeconds, setTransitionSeconds] = useState("1");
  const [visualTrackId, setVisualTrackId] = useState("");
  const [originalAudio, setOriginalAudio] = useState<"separate_linked" | "muted">("separate_linked");
  const [audioDrafts, setAudioDrafts] = useState<Record<string, AudioDraft>>({});
  const [prepared, setPrepared] = useState<{ key: string; request: HighLevelRequest; args: StoryboardArguments; result: StoryboardResult } | null>(null);
  const [pendingCommit, setPendingCommit] = useState<{ key: string; request: HighLevelRequest; args: StoryboardArguments; result: StoryboardResult } | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const [lastCommit, setLastCommit] = useState<StoryboardResult | null>(null);
  const dryRunSequence = useRef(0);

  useEffect(() => {
    const visualIds = new Set(visuals.map((asset) => asset.id));
    setSelectedVisuals((current) => current.filter((id) => visualIds.has(id)));
    setManualOrder((current) => current ? current.filter((id) => visualIds.has(id)) : null);
    setDurations((current) => {
      const next = { ...current };
      for (const asset of visuals) if (asset.kind === "image" && next[asset.id] === undefined) next[asset.id] = "10";
      for (const id of Object.keys(next)) if (!visualIds.has(id)) delete next[id];
      return next;
    });
    setAudioDrafts((current) => {
      const next = { ...current };
      const audioIds = new Set(audioAssets.map((asset) => asset.id));
      for (const asset of audioAssets) {
        if (!next[asset.id]) next[asset.id] = defaultAudioDraft(asset);
      }
      for (const id of Object.keys(next)) if (!audioIds.has(id)) delete next[id];
      return next;
    });
  }, [visuals, audioAssets]);

  useEffect(() => { setPrepared(null); setPendingCommit(null); setLastCommit(null); dryRunSequence.current += 1; }, [projectId, workspace]);
  useEffect(() => { setPrepared(null); dryRunSequence.current += 1; }, [revision]);
  useEffect(() => { setPrepared(null); dryRunSequence.current += 1; }, [selectedVisuals, durations, manualOrder, transitionKind, transitionSeconds, visualTrackId, originalAudio, audioDrafts]);

  const orderedVisuals = useMemo(() => [...visuals].sort((a, b) => naturalName.compare(basename(a.path), basename(b.path))), [visuals]);
  const naturallySelected = orderedVisuals.filter((asset) => selectedVisuals.includes(asset.id)).map((asset) => asset.id);
  const selectedOrder = manualOrder?.filter((id) => selectedVisuals.includes(id)) ?? naturallySelected;
  const visualTrackOptions = tracks.filter((track) => !track.locked && (track.kind === "video" || track.kind === "image"));
  const audioTrackOptions = tracks.filter((track) => track.kind === "audio" && !track.locked);

  const displayedTotal = selectedOrder.reduce((sum, assetId) => {
    const asset = visuals.find((item) => item.id === assetId);
    if (!asset) return sum;
    const override = durations[assetId]?.trim();
    const overrideSeconds = override ? Number(override) : undefined;
    return sum + (overrideSeconds && Number.isFinite(overrideSeconds) && overrideSeconds > 0 ? overrideSeconds : secondsOf(asset.duration_ticks) ?? 0);
  }, 0);
  const transitionValue = Number(transitionSeconds);
  const estimatedTotal = Math.max(0, displayedTotal - (transitionKind === "cross_dissolve" ? Math.max(0, selectedOrder.length - 1) * (Number.isFinite(transitionValue) ? transitionValue : 0) : 0));
  const durationKnown = selectedOrder.every((assetId) => {
    const asset = visuals.find((item) => item.id === assetId);
    return Boolean(durations[assetId]?.trim()) || (asset?.duration_ticks ?? 0) > 0;
  });
  const currentArgs = buildStoryboardArgs({ selectedOrder, visuals, durations, transitionKind, transitionSeconds, visualTrackId, originalAudio, audioDrafts });
  const currentKey = JSON.stringify(currentArgs);
  const liveDryRunIdentity = useRef({ projectId, workspace, revision, key: currentKey });
  liveDryRunIdentity.current = { projectId, workspace, revision, key: currentKey };
  const currentPrepared = prepared?.key === currentKey && prepared.request.projectId === projectId && prepared.request.workspace === workspace && prepared.request.revision === revision ? prepared : null;
  const currentRetry = pendingCommit?.request.projectId === projectId && pendingCommit.request.workspace === workspace ? pendingCommit : null;

  function invalidate() { setPrepared(null); setLastCommit(null); setLocalError(null); dryRunSequence.current += 1; }

  function setVisualSelected(assetId: string, selected: boolean) {
    invalidate();
    setSelectedVisuals((current) => {
      if (selected) return current.includes(assetId) ? current : [...current, assetId];
      return current.filter((id) => id !== assetId);
    });
    setManualOrder((current) => current ? (selected ? (current.includes(assetId) ? current : [...current, assetId]) : current.filter((id) => id !== assetId)) : null);
  }

  function moveSelected(assetId: string, delta: -1 | 1) {
    const index = selectedOrder.indexOf(assetId);
    const other = index + delta;
    if (index < 0 || other < 0 || other >= selectedOrder.length) return;
    const nextOrder = [...selectedOrder];
    [nextOrder[index], nextOrder[other]] = [nextOrder[other], nextOrder[index]];
    setManualOrder(nextOrder);
    invalidate();
  }

  async function previewAssembly() {
    if (!canRun || !currentArgs) return;
    setLocalError(null); setLastCommit(null);
    const sequence = ++dryRunSequence.current;
    const request: HighLevelRequest = { projectId, workspace, revision, idempotencyKey: idempotencyKey("storyboard-assemble") };
    const argsKey = currentKey;
    try {
      const result = await onAssemble(currentArgs, true, request);
      const live = liveDryRunIdentity.current;
      if (sequence !== dryRunSequence.current || live.projectId !== request.projectId || live.workspace !== request.workspace || live.revision !== request.revision || live.key !== argsKey) return;
      setPrepared({ key: argsKey, request, args: currentArgs, result });
    } catch (error) {
      const live = liveDryRunIdentity.current;
      if (sequence === dryRunSequence.current && live.projectId === request.projectId && live.workspace === request.workspace && live.revision === request.revision && live.key === argsKey) setLocalError(error instanceof Error ? error.message : String(error));
    }
  }

  async function commitPrepared(candidate: NonNullable<typeof currentPrepared>) {
    if (busy) return;
    setLocalError(null);
    setPendingCommit(candidate);
    try {
      const result = await onAssemble(candidate.args, false, candidate.request);
      setLastCommit(result); setPrepared(null); setPendingCommit(null);
    } catch (error) {
      if (isDefiniteRejection(error)) setPendingCommit(null);
      setLocalError(error instanceof Error ? error.message : String(error));
    }
  }

  async function commitAssembly() {
    if (!currentPrepared || currentRetry || busy) return;
    await commitPrepared(currentPrepared);
  }

  async function retryCommit() {
    if (!currentRetry || busy) return;
    await commitPrepared(currentRetry);
  }

  function updateAudio(assetId: string, patch: Partial<AudioDraft>) {
    invalidate();
    setAudioDrafts((current) => ({ ...current, [assetId]: { ...(current[assetId] ?? defaultAudioDraft(audioAssets.find((item) => item.id === assetId)!)), ...patch } }));
  }

  const canRun = mode === "tauri" && supported && !busy && Boolean(projectId);

  return <div className="assembly-panel">
    <div className="assembly-intro">
      <div><span className="eyebrow">STORY ASSEMBLY</span><h3>說書排片</h3><p>先在清單整理順序，再預覽共用核心的交易結果；確認後才會寫入專案。</p></div>
      <div className="assembly-summary"><span>圖片／影片</span><strong>{selectedOrder.length} 項</strong><small>{durationKnown ? formatSeconds(estimatedTotal) : `${formatSeconds(estimatedTotal)} 起，影片長度由核心讀取`}</small></div>
    </div>

    {mode !== "tauri" && <div className="assembly-notice">此工作流程需要桌面版共用核心；預覽模式不會建立假片段。</div>}
    {mode === "tauri" && !supported && <div className="assembly-notice">核心尚未在能力清單列出 storycut_storyboard_assemble，組裝操作已停用。</div>}
    {localError && <div className="assembly-error" role="alert">{localError}</div>}

    <div className="assembly-columns">
      <section className="assembly-card visual-card">
          <div className="assembly-card-head"><div><span className="eyebrow">01 / VISUALS</span><strong>圖片與影片</strong></div><div className="assembly-actions"><button type="button" onClick={() => { invalidate(); setSelectedVisuals(orderedVisuals.map((asset) => asset.id)); setManualOrder(null); }}>全選</button><button type="button" onClick={() => { invalidate(); setSelectedVisuals([]); setManualOrder(null); }}>清除</button></div></div>
        <div className="assembly-visual-list">
          {orderedVisuals.map((asset) => {
            const selected = selectedVisuals.includes(asset.id);
            const selectedIndex = selectedOrder.indexOf(asset.id);
            const naturalDuration = secondsOf(asset.duration_ticks);
            return <div key={asset.id} className={`assembly-visual-row ${selected ? "is-selected" : ""}`}>
              <label className="assembly-check"><input type="checkbox" checked={selected} onChange={(event) => setVisualSelected(asset.id, event.currentTarget.checked)} /><span>{String(selectedIndex + 1).padStart(2, "0")}</span></label>
              <div className={`assembly-kind ${asset.kind}`}>{asset.kind === "image" ? "IMG" : "VID"}</div>
              <div className="assembly-asset-name" title={asset.path}><strong>{basename(asset.path)}</strong><small>{asset.kind === "image" ? "靜態圖片" : naturalDuration ? `原始長度 ${formatSeconds(naturalDuration)}` : "核心將讀取來源長度"}</small></div>
              <label className="assembly-duration"><span>秒</span><input aria-label={`${basename(asset.path)} 片段秒數`} inputMode="decimal" type="number" min="0.034" step="0.1" placeholder={asset.kind === "image" ? "10" : naturalDuration ? naturalDuration.toFixed(2) : "來源長度"} value={durations[asset.id] ?? (asset.kind === "image" ? "10" : "")} onChange={(event) => { invalidate(); setDurations((current) => ({ ...current, [asset.id]: event.target.value })); }} onBlur={(event) => { const value = event.currentTarget.value.trim(); if (value && (!(Number(value) > 0) || !Number.isFinite(Number(value)))) setDurations((current) => ({ ...current, [asset.id]: "" })); }} /></label>
              <div className="assembly-reorder"><button type="button" title="在排片順序中向上移動" aria-label={`將 ${basename(asset.path)} 在排片順序中向上移動`} disabled={!selected || selectedIndex <= 0} onClick={() => moveSelected(asset.id, -1)}>↑</button><button type="button" title="在排片順序中向下移動" aria-label={`將 ${basename(asset.path)} 在排片順序中向下移動`} disabled={!selected || selectedIndex < 0 || selectedIndex >= selectedOrder.length - 1} onClick={() => moveSelected(asset.id, 1)}>↓</button></div>
            </div>;
          })}
          {!orderedVisuals.length && <div className="assembly-empty">素材庫沒有可排片的圖片或影片。</div>}
        </div>
        <div className="assembly-settings-grid">
          <label className="field-label">視覺軌道<select value={visualTrackId} onChange={(event) => { invalidate(); setVisualTrackId(event.target.value); }}><option value="">讓核心建立主畫面軌</option>{visualTrackOptions.map((track) => <option key={track.id} value={track.id}>{track.name} · {track.kind === "video" ? "影片" : "圖片"}</option>)}</select></label>
          <label className="field-label">切換方式<select value={transitionKind} onChange={(event) => { invalidate(); setTransitionKind(event.target.value as "cut" | "cross_dissolve"); }}><option value="cut">硬切</option><option value="cross_dissolve">交叉淡化</option></select></label>
          {transitionKind === "cross_dissolve" && <label className="field-label">交叉淡化長度（秒）<input type="number" min="0.034" step="0.1" value={transitionSeconds} onChange={(event) => { invalidate(); setTransitionSeconds(event.target.value); }} /></label>}
          <label className="field-label">影片原聲<select value={originalAudio} onChange={(event) => { invalidate(); setOriginalAudio(event.target.value as "separate_linked" | "muted"); }}><option value="separate_linked">保留並連結音訊</option><option value="muted">靜音</option></select></label>
        </div>
        <div className="assembly-total"><span>估算視覺總長{transitionKind === "cross_dissolve" ? "（扣除重疊）" : ""}</span><strong>{durationKnown ? formatSeconds(estimatedTotal) : `至少 ${formatSeconds(estimatedTotal)}`}</strong></div>
      </section>

      <section className="assembly-card audio-card">
        <div className="assembly-card-head"><div><span className="eyebrow">02 / SOUND</span><strong>旁白、對白與配樂</strong></div><small>獨立選擇起點與淡化</small></div>
        <div className="assembly-audio-list">
          {audioAssets.map((asset) => {
            const draft = audioDrafts[asset.id] ?? defaultAudioDraft(asset);
            return <div className={`assembly-audio-row ${draft.selected ? "is-selected" : ""}`} key={asset.id}>
              <label className="audio-choose"><input type="checkbox" checked={draft.selected} onChange={(event) => updateAudio(asset.id, { selected: event.currentTarget.checked })} /><span className="audio-role-mark">♪</span><strong title={asset.path}>{basename(asset.path)}</strong></label>
              {draft.selected && <div className="assembly-audio-fields">
                <label className="field-label">用途<select value={draft.role} onChange={(event) => updateAudio(asset.id, { role: event.target.value as AudioRole, trackName: ROLE_LABEL[event.target.value as AudioRole] })}>{Object.entries(ROLE_LABEL).map(([role, label]) => <option value={role} key={role}>{label}</option>)}</select></label>
                <label className="field-label">軌道<select value={draft.trackId} onChange={(event) => updateAudio(asset.id, { trackId: event.target.value })}><option value="">核心建立／依用途</option>{audioTrackOptions.map((track) => <option key={track.id} value={track.id}>{track.name}</option>)}</select></label>
                {!draft.trackId && <label className="field-label">新軌名稱<input value={draft.trackName} onChange={(event) => updateAudio(asset.id, { trackName: event.target.value })} /></label>}
                <label className="field-label">開始秒數<input type="number" min="0" step="0.1" value={draft.start} onChange={(event) => updateAudio(asset.id, { start: event.target.value })} /></label>
                <label className="field-label">增益 dB<input type="number" min="-96" max="24" step="1" value={draft.gain} onChange={(event) => updateAudio(asset.id, { gain: event.target.value })} /></label>
                <label className="field-label">淡入秒數<input type="number" min="0" step="0.1" value={draft.fadeIn} onChange={(event) => updateAudio(asset.id, { fadeIn: event.target.value })} /></label>
                <label className="field-label">淡出秒數<input type="number" min="0" step="0.1" value={draft.fadeOut} onChange={(event) => updateAudio(asset.id, { fadeOut: event.target.value })} /></label>
                <label className="field-label">片段長度秒數<input type="number" min="0.034" step="0.1" placeholder={asset.duration_ticks ? secondsOf(asset.duration_ticks)!.toFixed(2) : "來源長度"} value={draft.duration} onChange={(event) => updateAudio(asset.id, { duration: event.target.value })} /></label>
                {draft.role === "music" && <label className="assembly-loop"><input type="checkbox" checked={draft.loopToVisualEnd} onChange={(event) => updateAudio(asset.id, { loopToVisualEnd: event.currentTarget.checked })} />循環到畫面片尾</label>}
              </div>}
            </div>;
          })}
          {!audioAssets.length && <div className="assembly-empty">素材庫沒有獨立音訊素材；匯入旁白、對白或配樂後可在此配置。</div>}
        </div>
        <div className="assembly-commit-row">
          <div className="assembly-dryrun-copy">{currentPrepared ? <><strong>核心 dry-run 已完成</strong><small>修訂 v{currentPrepared.result.base_revision} · 視覺 {formatSeconds(currentPrepared.result.visual_duration_ticks / TIMEBASE)} · 含聲音 {formatSeconds(currentPrepared.result.duration_ticks / TIMEBASE)}</small></> : <><strong>先預覽交易差異</strong><small>不寫入專案；需再按「套用組裝」才會提交。</small></>}</div>
          <button className="button button-quiet" type="button" disabled={!canRun || !currentArgs || selectedOrder.length === 0 || transitionKind === "cross_dissolve" && (!Number.isFinite(transitionValue) || transitionValue <= 0 || transitionValue > displayedTotal)} onClick={() => void previewAssembly()}>核心 dry-run</button>
          {currentRetry ? <button className="button button-primary" type="button" disabled={busy} onClick={() => void retryCommit()}>重試同一提交</button> : <button className="button button-primary" type="button" disabled={!canRun || !currentPrepared} onClick={() => void commitAssembly()}>套用組裝</button>}
        </div>
        {currentRetry && <div className="assembly-notice compact" role="status">提交結果尚未確認。重試會使用預覽時的修訂版與同一冪等鍵；已提交的操作會回傳原結果。</div>}
        {currentPrepared?.result.warnings?.length ? <div className="assembly-warnings"><strong>核心提示</strong>{currentPrepared.result.warnings.map((warning, index) => <p key={`${warning.code}-${index}`}>{warning.code}：{warning.message}</p>)}</div> : null}
        {lastCommit && <div className="assembly-success" role="status">已由共用核心提交 {lastCommit.created_clip_ids.length} 個片段；時間軸修訂已更新。</div>}
      </section>
    </div>
  </div>;
}

function defaultAudioDraft(_asset: Asset): AudioDraft {
  return { selected: false, role: "narration", trackId: "", trackName: ROLE_LABEL.narration, start: "0", gain: "0", fadeIn: "0", fadeOut: "0", duration: "", loopToVisualEnd: false };
}

function isDefiniteRejection(error: unknown) {
  return error instanceof StoryCutApiError && error.apiError.code !== "INTERNAL_ERROR";
}

function buildStoryboardArgs({ selectedOrder, visuals, durations, transitionKind, transitionSeconds, visualTrackId, originalAudio, audioDrafts }: {
  selectedOrder: string[];
  visuals: Asset[];
  durations: Record<string, string>;
  transitionKind: "cut" | "cross_dissolve";
  transitionSeconds: string;
  visualTrackId: string;
  originalAudio: "separate_linked" | "muted";
  audioDrafts: Record<string, AudioDraft>;
}): StoryboardArguments | null {
  const items = selectedOrder.map((assetId) => {
    const asset = visuals.find((candidate) => candidate.id === assetId);
    if (!asset) return null;
    const overrideText = durations[assetId]?.trim();
    const override = overrideText ? Number(overrideText) : undefined;
    if (overrideText && (!Number.isFinite(override) || (override ?? 0) <= 0)) return null;
    if (asset.kind === "image" && override === undefined) return { asset_id: asset.id, duration_seconds: 10 };
    if (override !== undefined) return { asset_id: asset.id, duration_seconds: override };
    return { asset_id: asset.id };
  });
  if (items.some((item) => item === null)) return null;
  const selectedAudio = Object.entries(audioDrafts).filter(([, draft]) => draft.selected);
  const groups = new Map<string, NonNullable<StoryboardArguments["audio_tracks"][number]> & { _trackKey: string }>();
  for (const [assetId, draft] of selectedAudio) {
    const start = Number(draft.start), gain = Number(draft.gain), fadeIn = Number(draft.fadeIn), fadeOut = Number(draft.fadeOut);
    const duration = draft.duration.trim() ? Number(draft.duration) : undefined;
    if (![start, gain, fadeIn, fadeOut].every(Number.isFinite) || start < 0 || gain < -96 || gain > 24 || fadeIn < 0 || fadeOut < 0 || (duration !== undefined && (!Number.isFinite(duration) || duration <= 0))) return null;
    if (draft.trackId === "" && !draft.trackName.trim()) return null;
    const key = `${draft.role}:${draft.trackId || draft.trackName.trim()}`;
    let group = groups.get(key);
    if (!group) {
      group = { role: draft.role, ...(draft.trackId ? { track_id: draft.trackId } : { track_name: draft.trackName.trim() }), clips: [], _trackKey: key };
      groups.set(key, group);
    }
    group.clips.push({ asset_id: assetId, start_seconds: start, ...(duration === undefined ? {} : { duration_seconds: duration }), gain_db: gain, fade_in_seconds: fadeIn, fade_out_seconds: fadeOut, ...(draft.role === "music" && draft.loopToVisualEnd ? { loop_to_visual_end: true } : {}) });
  }
  const dissolve = Number(transitionSeconds);
  if (transitionKind === "cross_dissolve" && (!Number.isFinite(dissolve) || dissolve <= 0)) return null;
  return {
    items: items as StoryboardArguments["items"],
    ...(visualTrackId ? { visual_track_id: visualTrackId } : {}),
    transition: transitionKind === "cut" ? { kind: "cut" } : { kind: "cross_dissolve", duration_seconds: dissolve },
    original_video_audio: originalAudio,
    audio_tracks: [...groups.values()].map(({ _trackKey: _ignored, ...group }) => group),
  };
}

function formatSeconds(seconds: number) {
  const safe = Math.max(0, Number.isFinite(seconds) ? seconds : 0);
  const minutes = Math.floor(safe / 60);
  const remaining = safe - minutes * 60;
  return `${String(minutes).padStart(2, "0")}:${remaining.toFixed(2).padStart(5, "0")}`;
}
