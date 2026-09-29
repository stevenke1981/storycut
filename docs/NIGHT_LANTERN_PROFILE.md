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
- 分割、修剪有把手的片段目前回 `UNSUPPORTED_FEATURE`，不猜測把手要跟哪一側走；需調整時先移除把手（`clip_hold_set`）再剪。

用途：置中淡化只落在把手上，原生片「核心」全程不透明。

## 2. 置中交叉淡化（不縮短片長）

既有 `cross_dissolve` 是「兩片段重疊區」淡化，組裝時會把總長縮短。夜燈說書要的是在切點前後各半格混合、總長與旁白時間軸不變。這不需要新的轉場種類：高階組裝工具在切點 `c` 把前一片段延長到 `c + h`、後一片段從 `c − h` 開始（`h` = 半個淡化窗），宣告長度 `2h` 的 `cross_dissolve`。圖片直接加長；影片用凍格把手加長，核心不被覆蓋。

## 3. 軌道側鏈壓低（`ducking`）

`Track` 新增選填 `ducking`：

```json
{"source_track_id":"narration","threshold":0.015,"ratio":6,"attack_ms":30,"release_ms":500}
```

- 只能設在音訊軌；來源必須是另一條音訊軌，且來源軌本身不可再設定 ducking（不允許鏈或迴圈）。
- 渲染時每條音訊軌先各自加總成匯流排，受壓軌以來源軌匯流排做 `sidechaincompress`，最後所有軌線性加總。來源軌被靜音／停用時不壓低。
- 以 `track_update` 設定或清除（`"ducking": null`）。

## 4. 主控響度（輸出選項 `master_loudness`）

`storycut_render_start`／`storycut_preview_range` 的 `options.master_loudness`：

```json
{"integrated_lufs":-16,"true_peak_db":-1.5,"lra":11}
```

兩段式：先只算混音量測 EBU R128，再以 `loudnorm` 線性模式套用。不指定時維持原本「線性加總、不自動正規化」的契約。回傳結果附量測值。

## 5. ASS／SSA 樣式燒錄

`subtitle_mode: "burn"` 支援 ASS／SSA：保留原文件的 Script Info、Styles 與事件文字，只依字幕 `offset_tick` 與輸出範圍改寫 `Dialogue` 起訖時間，交給 libass。VTT 燒錄仍回 `UNSUPPORTED_FEATURE`。同時燒錄多份字幕時須同格式。

## 6. 高階工具 `storycut_narration_assemble`

依「旁白段落」排出整支故事：

- `narration`：依序的旁白音訊素材，可設片頭長度 `lead_in_seconds`、段間 `gap_seconds`。每段接在上一段之後，時間軸位置精確到音訊取樣。
- `shots`：依序的畫面；每個畫面宣告涵蓋的旁白段範圍 `segments: [first, last]`（含），圖片長度 = 該範圍的旁白總長。影片（原生生成片）以來源自然長度放入並自動加凍格把手，可用 `anchor` 指定它對齊哪個畫面區間的起點或終點。
- `intro`／`outro`：片頭（常是 10 秒原生片＋標題）與片尾卡。
- `transition`：`{"mode":"centered_dissolve","duration_seconds":0.4}` 或 `cut`。
- `music`：配樂 cue（素材、起訖秒、來源起點、增益、淡入淡出），並可設 `duck_by_narration`。
- 完成後回傳每段旁白在時間軸的起點（`segment_offsets`），供字幕對時。

工具仍是一次交易、可 dry-run、冪等、可復原；任何畫面範圍不連續、影片長度放不下、淡化會蓋到原生核心，都在提交前拒絕。

## 驗收

`tests/product_acceptance/night_lantern.py` 以合成素材（彩色圖片、10 秒含標記影片、旁白音、配樂、帶 alpha 字卡、雙語 ASS）走真 CLI 組裝與輸出，量測：總幀數等於旁白時間軸、原生片核心每格不透明、切點淡化前後各半窗、旁白區配樂被壓低、成片響度接近目標、ASS 字幕出現在指定時間。合成素材驗收不等於正式 YouTube 成片驗收。
