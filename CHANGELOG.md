# Changelog

## 桌面拖放與影片縮圖 — 2026-10-01

- 桌面版拖放改為指標事件實作：素材庫→軌道、片段在軌道間移動（保留抓取偏移、幀對齊、落點標線、軌道相容性高亮、邊緣自動捲動、Esc 取消）。原因：Tauri 在 Windows 預設啟用原生拖放，會使 HTML5 DnD 失效。
- 新增 OS 檔案拖入：把影音／圖片拖進視窗即呼叫 `storycut_media_import`（仍限工作區內，不複製、不放寬授權）；拖入單一 `.storycut.json` 則開啟專案。
- 素材庫顯示影片／圖片縮圖（`thumbnail` 濾鏡取代表性幀，避開黑場片頭；同時最多 2 個 FFmpeg 解碼）。
- 新增唯讀 `storycut_ffmpeg_info`（桌面橋接）：顯示 FFmpeg 版本與 libx264／aac 是否可用；NVENC 選項改為停用，與核心「尚未驗證」一致。
- 未驗證：原生 OS 檔案拖入需在 Tauri 視窗實測；本次僅以瀏覽器預覽模式實測指標拖放，並以真 FFmpeg 跑縮圖測試。

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
