# Changelog

## 夜燈說書主線整合 — 2026-09-30

- 修正旁白組裝 MCP 回傳契約、錯誤型別的 CLI 參數與響度報告持久化；同鍵重試與 job_get 可取回原報告。
- 修正限幅時的音訊延遲，以及 ASS 局部輸出／精確影格預覽的動畫時鐘；移除會切斷 UTF-8 的手寫 ASS 解析器。
- 補齊桌面鎖定依賴、真媒體 CI 與可攜版製作規格文件。

## 夜燈說書規格（Night Lantern profile）— 2026-09-29

- 新增 `storycut_narration_assemble`：旁白段落決定畫面長度，原生影片凍格把手、不縮短片長的置中淡化、側鏈壓低配樂、alpha 串場字卡、回傳段落時間供字幕對時。
- `VideoClip.hold_head_ticks`／`hold_tail_ticks`、`Track.ducking`（相容舊工程，預設不存在）。
- 輸出選項 `master_loudness`（ebur128 兩段式、線性增益＋必要時峰值限制器）；ASS／SSA 樣式燒錄。
- 真 FFmpeg 測試與 `tests/product_acceptance/night_lantern.py` 端到端驗收。見 docs/NIGHT_LANTERN_PROFILE.md。

## 規格 v0.2.0 — 2026-09-18

- 將 CLI、MCP 及多層影音時間軸提升為第一個可用版本必備項。
- 用統一 tracks/clips 模型取代單一 video_track 限制；保留主畫面混排能力。
- 新增影片疊加、透明圖片軌、旁白／配樂／原聲／音效多音軌與波形需求。
- 聲音與影片採連結片段，所有剪切共用精確時間基準。
- 制定 33 個 MCP 工具、19 種 timeline_apply 操作、JSON 輸入輸出與 CLI 結束碼。
- 增加原子交易、樂觀版本檢查、冪等 key、預檢、線性 undo/redo。
- 使用 sample-safe 整數 tick；裁切／分割保留動畫及聲音包絡的原始時間域。
- 新增 28 秒、8 軌合成範例，以及完整批次輸入與預期狀態。
- 加入可執行的規格驗證器與負向測試；不宣稱有 App、渲染或 MCP 實機實作。

## 規格 v0.1.0

原始圖片動態／影片混剪／字幕／轉場規格。當時 CLI/MCP 與任意多層畫面列為後續項；此分期已由 v0.2.0 明確取代。
