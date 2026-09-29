# 夜燈說書規格待辦（給實作 Agent）

分支：`feat/night-lantern-profile`（2026-09-29，commit a047ccf）。先讀 [NIGHT_LANTERN_PROFILE.md](NIGHT_LANTERN_PROFILE.md)、README、AGENTS.md 與 docs/IMPLEMENTATION_STATUS.md 的 2026-09-29 條目。

## 共通規則

- 一項一個 commit。每項都要附測試，並更新對應的契約、文件與 IMPLEMENTATION_STATUS。提交前跑 `cargo test --workspace --offline`、`python -X utf8 -m unittest discover -s tests`、`python -X utf8 tools/validate_spec.py`。有動到渲染的項目，另跑 `python -X utf8 tests/product_acceptance/night_lantern.py --cli target/debug/storycut.exe`。
- 不虛報：沒跑過的就寫 NOT_RUN；合成素材的驗收不等於正式成片驗收。
- 舊工程要相容：新欄位一律給預設值（`serde(default)`），序列化時省略空值。
- 用者素材只讀取，不覆寫既有輸出。

---

## P0：先修（目前有錯）

### NL-01　`storycut_narration_assemble` 的 outputSchema 與實際回傳不符
- **現況**：`contracts/mcp-tools.json` 裡這個工具的 `outputSchema` 是從 `storycut_storyboard_assemble` 整份複製來的。實際的 `data` 另有 `segment_offsets`、`cuts`、`native_cores`、`lead_in_ticks`、`narration_end_tick`、`total_duration_ticks`、`dissolve_window_ticks`、`frame_ticks`、`visual_track_id`、`narration_track_id`、`music_track_id`、`overlay_track_id`，這些都沒有描述在 schema 裡。
- **要做**：依 `crates/storycut-command/src/narration.rs` 的 `plan()` 回傳內容，寫一份正確的 outputSchema，欄位型別要精確（tick 用 integer，秒用 number，可為空的 track id 用 `anyOf null`）。
- **驗收**：在 `crates/storycut-mcp/tests/protocol.rs` 新增一項：真的呼叫這個工具，拿回傳的 `structuredContent` 對照 outputSchema 驗證（用現有的 JSON Schema 驗證方式，或新增 dev-dependency），並加一筆負向範例。

### NL-02　響度報告沒有保存到 job
- **現況**：`master_loudness` 的量測結果只出現在第一次 `render_start` 的回應裡。用同一個冪等鍵重放、或事後呼叫 `job_get`，都拿不到。
- **要做**：把報告寫進 job 的結果紀錄（`.storycut/jobs/<id>.result.json` 或 ready job）。欄位名稱與型別要更新進 job 的 outputSchema，重放和 `job_get` 都要回傳同一份報告。
- **驗收**：新增一項指令層測試，涵蓋 render → 同鍵重放 → `job_get`，三次拿到的報告要完全相同。

---

## P1：做完就能完整取代 ffmpeg 腳本

### NL-03　從 fish-s2pro-tts 分集匯入：`storycut_import_night_lantern`（CLI 或工具）
- **目的**：把現有夜燈說書分集直接轉成 StoryCut 工程，不再手寫組裝請求。
- **輸入**：分集版本目錄，例如 `E:\fish-s2pro-tts\episodes\toutao\v1-20260929`。
- **對應關係**：

  | 分集資料 | StoryCut 對應 |
  |---|---|
  | `production/segments.json`＋`production/audio/raw/segNNNN.mp3` | narration segments |
  | `production/video/camera/*still-render-plan.json`，或 `full-r1/assembly-plan.json` 的 `visual_units` | shots 與每段涵蓋範圍；`native_motion` 單元轉成影片 shot，對齊方式由位置推算 |
  | `production/video/native-video-assets.json` | 原生片來源與 accepted 區間 |
  | `bookends/*-card*.json` 或 intro 原生片 | intro／outro |
  | `production/video/music/music-cues.json` | music cues |
  | `production/video/lantern-motion/overlays.json` | overlays |
  | `subtitles/r*/bilingual-burn.ass` | 字幕，offset 為 0，因為時間已是絕對時間 |

- **規則**：
  - 只讀分集目錄，工程寫在另外授權的工作區。
  - 媒體路徑要在工作區授權範圍內；不在範圍內時回傳清楚的錯誤並列出缺件，不可自行複製素材。
  - 先 dry-run，並輸出一份對照報告。
- **驗收**：用合成的分集目錄（放在 tests/fixtures，不可引用正式素材）做端到端測試，匯入後組裝出的總幀數要等於 assembly-plan 的 `total_frames`。

### NL-04　overlays 支援淡入淡出與位置
- **現況**：串場字卡或標題 PNG 只能整張硬出現、硬消失。片頭標題需要「1.0 秒淡入、6.8 秒淡出」（R22 規則），目前做不到。
- **要做**：overlay item 新增 `fade_in_seconds`、`fade_out_seconds`（寫成 motion opacity 關鍵影格）和選填的 `scale`、`x`、`y`；渲染已經支援 opacity 關鍵影格。intro 也要能附一張標題 overlay（`intro.title_overlay_asset_id`＋時間參數）。
- **驗收**：用真 FFmpeg 測試，抽格測淡入中段的 alpha 約為 50%，標題在 6.8＋1.0 秒後完全消失。

### NL-05　配樂 cue 比素材長時，自動分段接續或循環
- **現況**：cue 比音檔長時直接報錯，要使用者自己拆成多段。
- **要做**：cue 新增 `repeat: "loop" | "none"`。loop 時自動切成多段，接縫用可設定長度的交叉淡化（`acrossfade`，或兩段重疊加各自淡入淡出），不超過交易上限 500 ops。
- **驗收**：用 30 秒素材鋪 100 秒 cue，接縫處沒有斷點，頻譜量測 220 Hz 一直存在。

### NL-06　shots 可直接指定運鏡
- **現況**：組裝完還要另外呼叫 `storycut_focal_motion_apply`。
- **要做**：shot 加選填 `motion: {preset, focus:{x,y}, from_scale, to_scale, pan_amount}`，沿用 focal motion 的規劃程式碼（抽成共用函式，不要複製一份）。圖片延長把手之後，運鏡的時間範圍要涵蓋整段 clip。
- **驗收**：指令層測試，確認每張圖的 motion 關鍵影格與 `focal_motion_apply` 產生的完全相同。

### NL-07　雙語字幕產生器
- **目的**：取代 R23 的 `story_bilingual_subs.py`。
- **要做**：新工具，輸入 cue 稿（每段 zh/en 陣列）、`segment_offsets` 和選填的 Qwen3ASR 字級時間 JSON。依字級時間分配 cue，沒有字級時間就按比例分配。輸出 ASS（繁中在上、英文在下）以及 zh-TW／en SRT，並匯入成字幕軌。
- **檢查**：每行中文不超過 16 字、英文不超過 50 字；行首不可是「的了著過們嗎呢吧啊」；行尾不可有「，。、；：」。
- **驗收**：cue 串接後要與原稿逐字相符；ASR 比對率低於 0.85 時，要在報告中列出改用比例分配的段落。

---

## P2：強化與限制解除

### NL-08　有凍格把手的片段也能修剪和分割
- **現況**：一律回 `UNSUPPORTED_FEATURE`。
- **規則**：
  - 修剪頭端時先扣 hold_head，扣完才動到 source_in；尾端同理，先扣 hold_tail。
  - 分割點落在把手裡時，那一側只留把手，另一側保留完整來源。
  - 分割後兩側的 motion 仍要連續（沿用 `sample_offset_tick`）。
- **驗收**：核心測試涵蓋頭把手、核心、尾把手三種分割位置；真 FFmpeg 逐格比對分割前後，MAE＝0。

### NL-09　同時燒錄多份 ASS，或 ASS 與 SRT 並用
- **要做**：先合併 Styles，同名樣式衝突時加前綴並改寫事件；SRT 轉成預設樣式事件；再合併成一份文件燒錄。
- **驗收**：兩份 ASS 加一份 SRT 同時燒錄，三者都出現在指定時間。

### NL-10　真正的 true-peak 限制器
- **現況**：目前用 `alimiter`，只看取樣峰值。
- **要做**：先 4 倍超取樣再限制後降回，或改用 FFmpeg 支援 true-peak 的方法。報告要寫明用的是 true peak 還是 sample peak。
- **驗收**：強迫觸發限制的測試素材，輸出以 ebur128 量得的 true peak 不超過設定值＋0.1 dB。

### NL-11　長片的渲染效能與背景執行
- **現況**：整支片走同一個濾鏡圖同步渲染；約 6.5 分鐘、40 多層的片子還沒量過耗時與記憶體。
- **要做**：
  1. 用 1080p30、400 秒、40 張圖、4 支原生片、3 首配樂、1 份 ASS 的合成工程，量耗時與峰值記憶體，寫進 IMPLEMENTATION_STATUS。
  2. 如果太慢，改成依切點分段渲染再無損串接，並保留跨段淡化（先切在淡化窗之外）。
  3. 接上 daemon 的背景工作與 `job_cancel`（ARCHITECTURE 原本就規劃）。
- **驗收**：分段版與整段版逐幀比對 MAE 小於 1，音訊取樣數相同。

### NL-12　桌面 GUI 的旁白組裝面板
- **要做**：
  - 選旁白音檔：多選，依檔名自然排序。
  - 畫面清單可拖曳調整涵蓋的段落範圍，影片可選 align。
  - 片頭、片尾、配樂 cue（含 ducking 開關與參數）、overlay 列表。
  - 試算後在時間軸上預覽 cuts 和原生核心區間（核心區塗色，淡化區加斜線）。
  - 輸出對話框加 `master_loudness` 選項。
- **規則**：走共用 dispatcher，試算綁定 revision 與冪等鍵，跟 storyboard 面板一樣。
- **驗收**：用原生視窗實測 dry-run → 套用 → 復原／重做；CLI 回讀同一 revision。截圖證據放在 desktop/acceptance。

### NL-13　Track 屬性面板顯示並編輯 ducking
- **要做**：GUI 軌道屬性加 ducking 來源下拉（只列非 ducked 的音訊軌）與參數欄位；清除時送出 `null`。
- **驗收**：GUI 設定後 CLI 回讀一致，反向由 CLI 設定後 GUI 同步顯示。

### NL-14　clippy 清理與 CI
- **要做**：新程式碼清掉 `manual_is_multiple_of` 等警告，舊程式碼要一起改的話，另開一個 commit。CI 加入 `cargo test` 的 night_lantern 測試，以及 `tests/product_acceptance/night_lantern.py`（需安裝 FFmpeg 與 libass）。
- **驗收**：GitHub Actions 跑綠，並附上 run 連結。

### NL-15　合併分支
- **要做**：NL-01、NL-02 完成後開 PR `feat/night-lantern-profile → main`。PR 描述附上驗收報告摘要，由主人審核後才能合併；Agent 不可自行合併。
