# 夜燈說書製作規格（Night Lantern profile）

本文件定義 StoryCut 為「夜燈說書・聊齋」類旁白故事影片新增的能力。依據是 fish-s2pro-tts 專案《偷桃》v1 的實際成片流程（R22／R23 工作流程包）：旁白逐段合成、畫面長度跟旁白走、全切點 0.4 秒置中交叉淡化且不改變總長、10 秒原生生成影片加首尾凍格把手、配樂由旁白側鏈壓低、主控響度 −16 LUFS、雙語 ASS 燒錄與帶 alpha 的串場字卡。

GUI、CLI、MCP 仍共用同一 Rust 核心；以下每項都是可編輯、可檢視的工程資料或明確的輸出選項，不在渲染時偷偷加效果。

## 1. 影片凍格把手（`hold_head_ticks`／`hold_tail_ticks`）

`VideoClip` 新增兩個選填欄位，預設 0，舊工程不變：

- 時間軸長度 = `hold_head_ticks` + 來源片段長 + `hold_tail_ticks`。
- 來源片段長 = `duration_ticks − hold_head − hold_tail`，必須 > 0，從 `source_in_tick` 起算，不可超出素材長度。
- 頭把手重複來源第一格、尾把手重複來源最後一格（渲染用 FFmpeg `tpad` clone）。
- 三者都須對齊 project frame。
- 有把手的影片不可連結原聲（`av_sync`），因把手期間沒有對應聲音；夜燈說書的原生片本來就無音軌。
- 分割、修剪有把手的片段目前回 `UNSUPPORTED_FEATURE`，不猜測把手要跟哪一側走；需調整時移除該片段後重新組裝。

用途：置中淡化只落在把手上，原生片「核心」全程不透明。StoryCut 的淡化是「後一片段在重疊區淡入」，所以原生片的首尾把手長度必須等於**整個淡化窗**（0.4 秒 @30fps = 12 幀），核心才會在切點後半窗處開始、全不透明；這與 R22 `story_native_handles.py` 的 6 幀把手＋置中 12 幀淡化在畫面上等價。

## 2. 置中交叉淡化（不縮短片長）

既有 `cross_dissolve` 是「兩片段重疊區」淡化，組裝時會把總長縮短。夜燈說書要的是在切點前後各半格混合、總長與旁白時間軸不變。這不需要新的轉場種類：高階組裝工具在切點 `c` 把前一片段延長到 `c + h`、後一片段從 `c − h` 開始（`h` = 半個淡化窗），宣告長度 `2h` 的 `cross_dissolve`。圖片直接加長；影片以長度 `2h` 的凍格把手加長，原生片在時間軸上佔「核心 + 2h」（片頭／片尾只有一側淡化時為「核心 + h」），核心不被覆蓋。

## 3. 軌道側鏈壓低（`ducking`）

`Track` 新增選填 `ducking`：

```json
{"source_track_id":"narration","threshold":0.015,"ratio":6,"attack_ms":30,"release_ms":500}
```

- 只能設在音訊軌；來源必須是另一條音訊軌，且來源軌本身不可再設定 ducking（不允許鏈或迴圈）。
- 渲染時每條音訊軌先各自加總成匯流排，受壓軌以來源軌匯流排做 `sidechaincompress`，最後所有軌線性加總。來源軌被靜音／停用時不壓低。
- 以 `track_update` 設定或清除（`"ducking": null`）。

## 4. 主控響度（輸出選項 `master_loudness`）

`storycut_render_start`／`storycut_preview_range` 的頂層參數 `master_loudness`：

```json
{"integrated_lufs":-16,"true_peak_db":-1.5,"lra":11}
```

兩段式：先只算混音、以 FFmpeg 參考量表 `ebur128` 量測整合響度與 true peak，再套用線性增益；若增益會讓峰值超過上限，才加上取樣峰值限制器（報告 `peak_limited`）。不用 `loudnorm linear=true`：它在峰值或 LRA 不符時會默默改成動態模式，而且在濾鏡圖內量到的 `input_i` 與 `ebur128` 相差逾 2 LU（實測）。不指定時維持原本「線性加總、不自動正規化」的契約。回傳結果附量測值。

響度報告持久保存在 job 的 `master_loudness`，首次輸出、同鍵重放、`storycut_job_get` 都可讀取相同內容；回應的 `data.master_loudness` 保留為相容欄位。`preview_range` 也採用相同紀錄方式。未指定正規化時不新增此欄位。`lra` 目前僅記錄目標與量測值，不調整動態範圍；峰值限制仍是 sample peak，並非已驗收的 true-peak 上限保證。

## 5. ASS／SSA 樣式燒錄

`subtitle_mode: "burn"` 支援 ASS／SSA：保留原文件的 Script Info、Styles 與事件文字，由字幕核心套用 `offset_tick`。範圍輸出與精確影格預覽使用原始字幕時鐘，保留移動、淡入淡出及 karaoke 的事件相對時間。VTT 燒錄仍回 `UNSUPPORTED_FEATURE`。目前一次只支援一份 ASS／SSA；多份 SRT 可合併燒錄，ASS／SSA 不可與其他字幕文件同時燒錄。

## 6. 高階工具 `storycut_narration_assemble`

依「旁白段落」排出整支故事：

- `narration`：依序的旁白音訊素材，可設段間 `gap_seconds`。片頭長度由 `intro` 推算，沒有獨立的 `lead_in_seconds` 參數。每段接在上一段之後，時間軸位置精確到音訊取樣。
- `shots`：依序的畫面；每個畫面宣告涵蓋的旁白段範圍 `segments: [first, last]`（從 0 起算、含尾端），圖片長度 = 該範圍的旁白總長。影片（原生生成片）以來源自然長度放入並自動加凍格把手，可用 `align: "start" | "center" | "end"` 指定對齊方式。
- `intro`／`outro`：片頭（常是 10 秒原生片＋標題）與片尾卡。
- `transition`：`{"mode":"centered_dissolve","duration_seconds":0.4}` 或 `cut`。
- `music`：配樂 cue（素材、起訖秒、來源起點、增益、淡入淡出），預設依旁白 ducking，可設 `ducking` 參數物件調整或 `null` 停用。
- 原生片長於所屬旁白段時置中外溢到相鄰畫面；第一個／最後一個畫面若是原生片，須用 `align: start`／`end` 貼齊故事邊界；相鄰兩支原生片互相衝突時拒絕。
- 完成後回傳每段旁白在時間軸的起點（`segment_offsets`）、切點（`cuts`）、原生核心區間（`native_cores`），供字幕對時與檢查。圖片運鏡沿用 `storycut_focal_motion_apply`。

工具仍是一次交易、可 dry-run、冪等、可復原；任何畫面範圍不連續、影片長度放不下、淡化會蓋到原生核心，都在提交前拒絕。

## 驗收

實測（2026-09-29，Windows、FFmpeg N-124464）：`cargo test` 新增 render 5 項真 FFmpeg、core 2 項、command 2 項；`tests/product_acceptance/night_lantern.py` PASS（1524 幀＝50.8 秒、原生核心逐格原色、切點半混、alpha 字卡、雙語 ASS、配樂於旁白下降約 10 倍、成片 −16.0 LUFS、來源檔 SHA 不變）。

`tests/product_acceptance/night_lantern.py` 以合成素材（彩色圖片、10 秒含標記影片、旁白音、配樂、帶 alpha 字卡、雙語 ASS）走真 CLI 組裝與輸出，量測：總幀數等於旁白時間軸、原生片核心每格不透明、切點淡化前後各半窗、旁白區配樂被壓低、成片響度接近目標、ASS 字幕出現在指定時間。合成素材驗收不等於正式 YouTube 成片驗收。
