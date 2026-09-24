# 多軌時間軸與時間契約

## 1. 軌道，不只是素材清單

| kind | 接受內容 | 合成／播放 |
|---|---|---|
| video | video clip 或 image clip | 主畫面混排、B-roll、畫中畫 |
| image | image clip | 透明 PNG、標誌、靜態圖動態、疊圖 |
| audio | audio clip；來源可為音檔或影片的 audio stream | 原聲、旁白、配樂、音效 |
| subtitle | 字幕 document／cue | 保留原字幕語法與時間軸定位 |

tracks 陣列中視覺軌由底至頂排序；subtitle 軌固定在所有視覺軌之上，字幕軌互相順序依陣列。音軌排序只影響 UI，不影響線性加總。每個 kind 可以新增多條，不硬編碼只准兩條音軌。

video_clip.audio_policy='separate_linked' 必須存在唯一的 av_sync 連結音訊；'muted' 不從該 video clip 發聲。已有 audio clip 才是唯一聲音來源。連結的兩段需同素材、時間位置、時長與 source_in；stream index 不同且種類正確。

解除連結只移除同步關係，不刪音訊；video policy 改為 muted，而原 audio clip 仍独立播放。刪除這段音訊是另一個明確操作。

## 2. 全片時間與素材時間

所有全片範圍為半開區間 `[start_tick, start_tick + duration_ticks)`。
`source_in_tick` 是來源開始位置，不是全片位置。影片從原始第 2 秒取 10 秒，擺在全片第 18 秒：start=18 秒、source_in=2 秒、duration=10 秒。

工程 timebase = 705600000 ticks/second：

- 30 fps 一幀 = 23520000 ticks。
- 30000/1001 fps 一幀 = 23543520 ticks。
- 48 kHz 一個樣本 = 14700 ticks；44.1 kHz 一個樣本 = 16000 ticks。

工程限制 24 小時，所有 tick/加法需 checked；不使用累加浮點秒數。tick 常數不是產業標準，而是本案兼顧常見 fps 與 sample rate 的設計。無法整除的自訂幀率第一版拒絕，不能靜默取近似值。

GUI 秒數先以 decimal/rational 解析；image/video 起點、長度、transition 需對齊整幀。audio 對齊工程 sample；音訊 source_in 仍以來源 timestamp 解码和 resample，不強求来源 sample lattice 等於工程 lattice。字幕可保留原有毫秒或 ASS 百分秒精度。第一版每條字幕軌放一個文件；多語或多文件可使用多條字幕軌。

**AV 同步例外：** 30000/1001 fps 的單幀通常不是 48 kHz 的整數樣本數。已連結的 audio clip 保留與 video 完全相同的 tick 邊界，允許落在樣本之間；renderer 對每個「全片絕對端點」做 nearest ties-to-even sample 映射，然後以 end_sample−start_sample 決定樣本數。不能逐段累加取整後的長度。理想端點至實際輸出樣本的誤差最多半個樣本；GUI 保留真實 tick，不能冒稱任意 video frame 都恰好是音訊樣本邊界。

`--seconds` 類便利入口採 nearest ties-to-even 對齊，必須回應 requested/actual 的差異。契約的 tick 入口不自動取整，未對齊直接拒絕。

## 3. 28 秒範例

| 軌道 | 全片時間 | 內容 |
|---|---|---|
| V1 主畫面 | A 0–10；B 9–19；C 18–28 秒 | 兩張動態圖片＋一段影片；兩次 1 秒交叉淡化 |
| V2 影片疊加 | 20–24 秒 | 同一來源的另一裁切片段，縮小成畫中畫；此段原聲靜音 |
| I1 圖片 | 2–8 秒 | 透明標誌；不得把透明區自動填黑 |
| A1 原聲 | 18–28 秒 | 與 C 的 source 2–12 秒同步，頭 1 秒淡入 |
| A2 旁白 | 0–28 秒 | 獨立鎖定／移動；不隨一般畫面移動 |
| A3 配樂 | 0–28 秒 | -18 dB、頭 1 秒淡入、尾 2 秒淡出 |
| A4 音效 | 9–10 秒 | -6 dB 的一秒音效 |
| S1 字幕 | 1–4 與 10–18 秒 | 保留 VTT cue 的定位設定 |

範例資料在 `examples/multitrack.storycut.json`。以上是規劃的測試值，非實際媒體分析；素材未附、未渲染。28 秒 × 30 fps = 840 幀。

## 4. 時長與重疊

多軌的內容長度 = 所有 clip 結束點及 subtitle offset+cue end 的最大值。**不能把所有軌的時長相加，也不能在既定 start_tick 的最大結束點上再次扣轉場。** muted/disabled 軌仍保留時間占位，切換顯示不可改變片長。

export range 預設 `[0,ceil_frame(內容長度))`，音訊尾端少於一幀以靜音補齊。無畫面的區間顯示工程背景；不自行延長最後一張圖。使用者可明確選其他輸出範圍。

同一視覺軌重疊，必須剛好對應 transition 的起點／長度，第一版最多兩段同時活躍；中間片段的前後轉場合計不可超過其長度。跨視覺軌重疊是正常疊圖，不必放 transition。

音訊同軌與跨軌重疊都可，線性混音而不是覆蓋；UI 顯示音訊重疊区。影像轉場和聲音淡化分開；勾選「同步淡化原聲」要生成同一交易中的 audio.set，不暗改音訊。

## 5. 拖動／剪切／修剪

- move：一般只動目標及其 AV link；獨立旁白／字幕不變。跨軌需符合 kind；連結音訊維持所在音軌。
- trim：明確指定新 start/source_in/duration；影像與連結音訊同步更新。來源超限直接拒絕；圖片 source_in 固定 0。
- split：播放頭需严格落在片段內；左段保留原 ID，右段新 ID 由 server 產生並回傳。連結 A/V 一起分割并生成兩組 link；不得漏掉原聲。
- remove lift：留下空隙，不移動其他片段；連結片段一起刪除，不刪原始素材。
- remove ripple：切除全片區間並左移明列的 ripple_track_ids 及必需連結片段；穿過其他片段中間或锁定轨時，第一版拒绝並要求先 split。空清單不是「自動全部」。
- 修改有轉場的片段：批次要包含轉場的修正或移除；不能偷偷改長度。交叉淡化活動區內 split 第一版拒绝，待明确重建轉場。

在 schema 中 clip.move/trim/split/remove 的 respect_links 固定 true。要独立編輯原聲，先明確 link.remove；不存在绕过锁定／同步的 --force。

## 6. 分割時動畫及淡入不得跳動

motion 與 audio envelope 各有 `domain_duration_ticks` 及 `sample_offset_tick`。當地片段時間 t 取樣 domain 的 `sample_offset_tick+t`。

例如 10 秒圖在第 5 秒切開：左右各 5 秒，原運動 domain 仍為 10 秒；左 sample_offset=0，右=5 秒。原兩個 keyframe 不變，兩段接起來每幀與未切開相同。音訊亦不因 split 再從音量 0 起跳。

trim 的 resample_visible 保留被看見的原 domain；retime_full 明確把完整運動重新分配到新長度。延長圖片時通常用 retime_full；保留 domain 卻超出範圍應拒絕，不擅自保持最後一格。0 長度不合法；只有一幀時只存一個 keyframe。

## 7. UI 與鎖軌

提供縮放、吸附、縮圖、波形、拖曳入出點、播放頭、選取群組、鎖軌、mute/solo 與復原。audio track gain_db／muted／solo 對非音軌必須為 0/false/false；enabled 控制該軌整體輸出。

solo 只在 enabled audio tracks 中判斷；若其中任一 solo=true，只播放 enabled+solo+not muted 的軌。clip mute 永遠再排除自身；lock 只影響編輯，不影響播放。

解鎖是一個明確操作，不允許 Agent 為繞過錯誤擅自解鎖。鎖定任何連結成员時整個命令失敗，不留下半套修改。
