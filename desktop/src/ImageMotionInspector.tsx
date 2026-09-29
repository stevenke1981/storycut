import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { api, errorText, idempotencyKey, StoryCutApiError } from "./api";
import type { Asset, BackendMode, Clip } from "./types";
import { TIMEBASE } from "./types";

export type FocalPreset = "focus_zoom" | "static" | "pan_left" | "pan_right" | "pan_up" | "pan_down" | "zoom_in" | "zoom_out";
export interface FocalMotionArguments {
  targets: Array<{ clip_id: string; focus: { x: number; y: number } }>;
  preset: FocalPreset;
  from_scale?: number;
  to_scale?: number;
  pan_amount?: number;
}

export interface FocalResult {
  changed_clip_ids?: string[];
  warnings?: Array<{ code: string; message: string; clip_id?: string }>;
}
type HighLevelRequest = { projectId: string; workspace: string | null; revision: number; idempotencyKey: string };

const PRESETS: Array<{ value: FocalPreset; label: string }> = [
  { value: "focus_zoom", label: "主體聚焦拉近" },
  { value: "static", label: "靜止" },
  { value: "pan_left", label: "畫面內容向左" },
  { value: "pan_right", label: "畫面內容向右" },
  { value: "pan_up", label: "畫面內容向上" },
  { value: "pan_down", label: "畫面內容向下" },
  { value: "zoom_in", label: "放大" },
  { value: "zoom_out", label: "縮小" },
];

export function ImageMotionInspector({
  projectId, revision, workspace, clip, asset, assets, imageClips, mode, busy, supported, onApply,
}: {
  projectId: string;
  revision: number;
  workspace: string | null;
  clip: Clip;
  asset: Asset;
  assets: Asset[];
  imageClips: Clip[];
  mode: BackendMode;
  busy: boolean;
  supported: boolean;
  onApply: (args: FocalMotionArguments, dryRun: boolean, request: HighLevelRequest) => Promise<FocalResult>;
}) {
  const [focus, setFocus] = useState({ x: clip.motion?.anchor.x ?? 0.5, y: clip.motion?.anchor.y ?? 0.5 });
  const [preset, setPreset] = useState<FocalPreset>("focus_zoom");
  const [fromScale, setFromScale] = useState("1");
  const [toScale, setToScale] = useState("1.18");
  const [panAmount, setPanAmount] = useState("0.03");
  const [targetIds, setTargetIds] = useState<string[]>([clip.id]);
  const [thumbnail, setThumbnail] = useState<{ data_url: string; width: number; height: number } | null>(null);
  const [thumbnailState, setThumbnailState] = useState<"loading" | "ready" | "unavailable">(mode === "tauri" ? "loading" : "unavailable");
  const [thumbnailError, setThumbnailError] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const [prepared, setPrepared] = useState<{ key: string; request: HighLevelRequest; args: FocalMotionArguments; result: FocalResult } | null>(null);
  const [pendingCommit, setPendingCommit] = useState<{ key: string; request: HighLevelRequest; args: FocalMotionArguments; result: FocalResult } | null>(null);
  const sourceRef = useRef<HTMLButtonElement>(null);
  const dryRunSequence = useRef(0);
  const [sourceSize, setSourceSize] = useState({ width: 0, height: 0 });

  useEffect(() => {
    setFocus({ x: clip.motion?.anchor.x ?? 0.5, y: clip.motion?.anchor.y ?? 0.5 });
    setTargetIds([clip.id]);
    setPrepared(null);
    dryRunSequence.current += 1;
  }, [clip.id, clip.motion?.anchor.x, clip.motion?.anchor.y]);

  useEffect(() => {
    setPrepared(null); setPendingCommit(null); dryRunSequence.current += 1;
  }, [projectId, workspace]);

  useEffect(() => {
    let cancelled = false;
    setThumbnail(null); setThumbnailError(null);
    if (mode !== "tauri") { setThumbnailState("unavailable"); return () => { cancelled = true; }; }
    setThumbnailState("loading");
    void api.readAssetThumbnail(projectId, asset).then((result) => {
      if (!cancelled) { setThumbnail(result); setThumbnailState("ready"); }
    }).catch((error) => {
      if (!cancelled) { setThumbnailState("unavailable"); setThumbnailError(errorText(error)); }
    });
    return () => { cancelled = true; };
  }, [projectId, asset.id, asset.sha256, mode]);

  useEffect(() => {
    const element = sourceRef.current;
    if (!element) return;
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect;
      if (rect) setSourceSize({ width: rect.width, height: rect.height });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [thumbnailState, thumbnail]);

  useEffect(() => { setPrepared(null); dryRunSequence.current += 1; }, [revision]);

  const validTargets = useMemo(() => targetIds.filter((id) => imageClips.some((candidate) => candidate.id === id)), [targetIds, imageClips]);
  const assetsById = useMemo(() => new Map(assets.map((candidate) => [candidate.id, candidate])), [assets]);
  const scalePreset = preset === "focus_zoom" || preset === "zoom_in" || preset === "zoom_out";
  const from = Number(fromScale), to = Number(toScale), pan = Number(panAmount);
  const args: FocalMotionArguments | null = validTargets.length > 0 && (scalePreset ? Number.isFinite(from) && Number.isFinite(to) && from >= 1 && from <= 16 && to >= 1 && to <= 16 : Number.isFinite(pan) && pan > 0 && pan <= 0.5)
    ? { targets: validTargets.map((clipId) => ({ clip_id: clipId, focus })), preset, ...(scalePreset ? { from_scale: from, to_scale: to } : { pan_amount: pan }) }
    : null;
  const argsKey = args ? JSON.stringify(args) : "";
  const liveDryRunIdentity = useRef({ projectId, workspace, revision, key: argsKey });
  liveDryRunIdentity.current = { projectId, workspace, revision, key: argsKey };
  const currentPrepared = prepared?.key === argsKey && prepared.request.projectId === projectId && prepared.request.workspace === workspace && prepared.request.revision === revision ? prepared : null;
  const currentRetry = pendingCommit?.request.projectId === projectId && pendingCommit.request.workspace === workspace ? pendingCommit : null;
  const canApply = mode === "tauri" && supported && !busy && Boolean(args);
  const imageBox = thumbnail ? containedBox(sourceSize.width, sourceSize.height, thumbnail.width, thumbnail.height) : null;
  const markerStyle = imageBox ? { left: imageBox.left + focus.x * imageBox.width, top: imageBox.top + focus.y * imageBox.height } : undefined;

  function invalidatePrepared() {
    setPrepared(null); setLocalError(null); dryRunSequence.current += 1;
  }

  function setTarget(clipId: string, checked: boolean) {
    invalidatePrepared();
    setTargetIds((current) => checked ? [...new Set([...current, clipId])] : current.filter((id) => id !== clipId));
  }

  function focusFromPointer(clientX: number, clientY: number) {
    if (!sourceRef.current || !thumbnail) return;
    const rect = sourceRef.current.getBoundingClientRect();
    const box = containedBox(rect.width, rect.height, thumbnail.width, thumbnail.height);
    if (!box) return;
    const x = (clientX - rect.left - box.left) / box.width;
    const y = (clientY - rect.top - box.top) / box.height;
    if (x < 0 || x > 1 || y < 0 || y > 1) return;
    setFocus({ x: roundNormalized(x), y: roundNormalized(y) });
    invalidatePrepared();
  }

  function moveFocusByKeyboard(event: KeyboardEvent<HTMLButtonElement>) {
    const step = event.shiftKey ? 0.05 : 0.01;
    const directions: Record<string, { x: number; y: number }> = { ArrowLeft: { x: -step, y: 0 }, ArrowRight: { x: step, y: 0 }, ArrowUp: { x: 0, y: -step }, ArrowDown: { x: 0, y: step } };
    const delta = directions[event.key];
    if (!delta) return;
    event.preventDefault();
    setFocus((current) => ({ x: roundNormalized(current.x + delta.x), y: roundNormalized(current.y + delta.y) }));
    invalidatePrepared();
  }

  async function dryRun() {
    if (!canApply || !args) return;
    setLocalError(null);
    const sequence = ++dryRunSequence.current;
    const request: HighLevelRequest = { projectId, workspace, revision, idempotencyKey: idempotencyKey("focal-motion") };
    const key = argsKey;
    try {
      const result = await onApply(args, true, request);
      const live = liveDryRunIdentity.current;
      if (sequence !== dryRunSequence.current || live.projectId !== request.projectId || live.workspace !== request.workspace || live.revision !== request.revision || live.key !== key) return;
      setPrepared({ key, request, args, result });
    }
    catch (error) {
      const live = liveDryRunIdentity.current;
      if (sequence === dryRunSequence.current && live.projectId === request.projectId && live.workspace === request.workspace && live.revision === request.revision && live.key === key) setLocalError(error instanceof Error ? error.message : String(error));
    }
  }

  async function commitPrepared(candidate: NonNullable<typeof currentPrepared>) {
    if (busy) return;
    setLocalError(null);
    setPendingCommit(candidate);
    try { await onApply(candidate.args, false, candidate.request); setPrepared(null); setPendingCommit(null); }
    catch (error) {
      if (isDefiniteRejection(error)) setPendingCommit(null);
      setLocalError(error instanceof Error ? error.message : String(error));
    }
  }

  async function commit() {
    if (!currentPrepared || currentRetry || busy) return;
    await commitPrepared(currentPrepared);
  }

  async function retryCommit() {
    if (!currentRetry || busy) return;
    await commitPrepared(currentRetry);
  }

  return <div className="motion-inspector">
    <div className="inspector-section motion-focus-section">
      <div className="section-title">主體焦點 <span>來源圖片座標 0–1</span></div>
      {thumbnailState === "loading" && <div className="focus-thumbnail-placeholder">正在讀取所選圖片縮圖…</div>}
      {thumbnailState === "ready" && thumbnail && <button ref={sourceRef} type="button" className="focus-thumbnail" aria-label={`圖片焦點，水平 ${focus.x.toFixed(2)}、垂直 ${focus.y.toFixed(2)}；方向鍵可微調`} aria-valuetext={`x ${focus.x.toFixed(2)}, y ${focus.y.toFixed(2)}`} onClick={(event) => { if (event.detail > 0) focusFromPointer(event.clientX, event.clientY); }} onKeyDown={moveFocusByKeyboard}>
        <img src={thumbnail.data_url} alt="所選圖片素材縮圖" draggable={false} />
        {markerStyle && <span className="focus-marker" style={markerStyle}><i /></span>}
      </button>}
      {thumbnailState === "unavailable" && <div className="focus-thumbnail-placeholder">{mode === "demo" ? "示範素材沒有本機原圖；焦點編輯只對桌面專案的已選圖片啟用。" : thumbnailError ? "目前無法讀取此圖片縮圖。" : "圖片縮圖不可用。"}</div>}
      {thumbnailError && <small className="focus-thumb-error" title={thumbnailError}>{thumbnailError}</small>}
      <div className="focus-coordinate-readout"><span>x <strong>{focus.x.toFixed(3)}</strong></span><span>y <strong>{focus.y.toFixed(3)}</strong></span><button type="button" onClick={() => { setFocus({ x: 0.5, y: 0.5 }); invalidatePrepared(); }}>置中</button></div>
      <small className="focus-help">點選圖片定位主體；Tab 聚焦後可用方向鍵移動，按住 Shift 可加大步幅。</small>
    </div>

    <div className="inspector-section">
      <div className="section-title">圖片動態 <span>共用核心焦點工具</span></div>
      <label className="field-label">動態預設<select value={preset} onChange={(event) => { setPreset(event.target.value as FocalPreset); invalidatePrepared(); }}>{PRESETS.map((option) => <option value={option.value} key={option.value}>{option.label}</option>)}</select></label>
      {scalePreset ? <div className="two-fields"><label className="field-label">起始縮放<input type="number" min="1" max="16" step="0.01" value={fromScale} onChange={(event) => { setFromScale(event.target.value); invalidatePrepared(); }} /></label><label className="field-label">結束縮放<input type="number" min="1" max="16" step="0.01" value={toScale} onChange={(event) => { setToScale(event.target.value); invalidatePrepared(); }} /></label></div> : preset.startsWith("pan_") ? <label className="field-label">平移幅度（畫布比例）<input type="number" min="0.001" max="0.5" step="0.01" value={panAmount} onChange={(event) => { setPanAmount(event.target.value); invalidatePrepared(); }} /><small>核心會依圖片比例限制移動量，避免露出黑邊。</small></label> : <p className="motion-static-note">靜止預設會將所選圖片改為固定焦點，不做平移或縮放。</p>}
    </div>

    <div className="inspector-section motion-target-section">
      <div className="section-title">套用對象 <span>{validTargets.length} 張圖片片段</span></div>
      <div className="motion-target-list">{imageClips.map((candidate) => { const targetAsset = assetsById.get(candidate.asset_id); return <label key={candidate.id} title={`片段 ${candidate.id}`}><input type="checkbox" checked={validTargets.includes(candidate.id)} onChange={(event) => setTarget(candidate.id, event.currentTarget.checked)} /><span>{targetAsset ? basename(targetAsset.path) : candidate.asset_id} · {formatSeconds(candidate.duration_ticks / TIMEBASE)}</span>{candidate.id === clip.id && <em>目前選取</em>}</label>; })}{!imageClips.length && <small>時間軸上尚無圖片片段。</small>}</div>
      {mode === "tauri" && !supported && <div className="assembly-notice compact">核心能力清單尚未列出 storycut_focal_motion_apply。</div>}
      {localError && <div className="assembly-error" role="alert">{localError}</div>}
      {currentPrepared && <div className="focal-dryrun"><strong>dry-run 已通過 · {currentPrepared.result.changed_clip_ids?.length ?? validTargets.length} 個片段</strong>{currentPrepared.result.warnings?.map((warning, index) => <small key={`${warning.code}-${warning.clip_id ?? index}`}>{warning.code}{warning.clip_id ? ` · ${warning.clip_id}` : ""}：{warning.message}</small>)}</div>}
      <div className="focal-actions"><button className="button button-quiet" type="button" disabled={!canApply || thumbnailState !== "ready"} onClick={() => void dryRun()}>dry-run 預覽</button>{currentRetry ? <button className="button button-primary" type="button" disabled={busy} onClick={() => void retryCommit()}>重試同一提交</button> : <button className="button button-primary" type="button" disabled={!canApply || !currentPrepared} onClick={() => void commit()}>套用動態</button>}</div>
      {currentRetry && <small className="focus-help" role="status">提交結果尚未確認。重試會使用預覽時的修訂版與同一冪等鍵；已提交的操作會回傳原結果。</small>}
    </div>
  </div>;
}

function containedBox(width: number, height: number, imageWidth: number, imageHeight: number) {
  if (width <= 0 || height <= 0 || imageWidth <= 0 || imageHeight <= 0) return null;
  const scale = Math.min(width / imageWidth, height / imageHeight);
  const boxWidth = imageWidth * scale, boxHeight = imageHeight * scale;
  return { left: (width - boxWidth) / 2, top: (height - boxHeight) / 2, width: boxWidth, height: boxHeight };
}

function isDefiniteRejection(error: unknown) {
  return error instanceof StoryCutApiError && error.apiError.code !== "INTERNAL_ERROR";
}

function roundNormalized(value: number) { return Math.max(0, Math.min(1, Math.round(value * 1000) / 1000)); }
function basename(value: string) { return value.split(/[\\/]/).pop() ?? value; }
function formatSeconds(value: number) { return `${(Number.isFinite(value) ? value : 0).toFixed(2)}s`; }
