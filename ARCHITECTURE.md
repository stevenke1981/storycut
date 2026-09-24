# Rust 架構 v0.2.0（待實作）

## 一個核心，三個入口

```text
桌面 GUI（Tauri + React） ─┐
CLI（storycut）           ├─ 本機 IPC ─ storycut-daemon ─ command-dispatcher
MCP stdio adapter        ┘                          ├─ editor-core / subtitle-core
                                                     ├─ project-store / history
                                                     ├─ media-service
                                                     └─ render-planner → render-worker
```

桌面與 CLI/MCP 共用工作區實例、revision、命令驗證與歷史。daemon 以每使用者／工作區單一實例鎖，自動啟動；不要求 GUI 開著。啟動競爭只能有一個 daemon 勝出；其餘連接既有實例。GUI 不可用舊快照整檔覆蓋 Agent 剛寫入的修改。

## 建議 crates

| crate | 責任 |
|---|---|
| storycut-core | 軌道、片段、連結、精確時間、轉場、immutable snapshot、19 操作 |
| storycut-subtitle | SRT/ASS/SSA/VTT 專屬文件、未知語法保留及編輯能力判定 |
| storycut-protocol | serde 類型、schemars schema、公共錯誤與版本 |
| storycut-store | 專案檔、journal、原子保存、恢復、持久冪等記錄 |
| storycut-media | ffprobe、來源 stream、指紋、波形、縮圖、代理 |
| storycut-render | 統一 render plan、畫面合成、音訊混音、快取與驗收 |
| storycut-daemon | 命令順序、工作所有權、OS IPC、修訂事件 |
| storycut-cli | clap 參數、JSON/JSONL stdout、exit code；不得依賴 GUI |
| storycut-mcp | 官方 rmcp 適配、工具註冊、協議生命週期與結果 [M3] |
| storycut-desktop | Tauri 2 commands、事件、Canvas 時間軸、預覽 |

這是目標模組，不是本包已有的 Rust workspace。實作選定實際 crate 版本後才產生 Cargo.lock；不得虛構已編譯版本。Tauri 能嵌入外部 binary，但需處理平台命名、打包和权限配置。[R3]

## 時間與剪切

使用整數 tick（每秒 705600000）與 rational fps。影像對齊 project frame，獨立音訊對齊 project sample；AV linked 音訊保留影片 tick 邊界，在輸出時依絕對端點做有界 sample 量化（詳見 TIMELINE）；來源 PTS／time_base 另存，不假設 VFR 來源第 N 幀等於 N/fps。計算用 checked integer／i128 中間值；TypeScript 接口受 24 小時上限與 safe integer 檢查。此數值是本案設計，不是 MCP 或 FFmpeg 的標準要求。

圖片的兩個端點在 motion.domain_duration_ticks 的第 0 幀與最後顯示幀。split/trim 保留原 motion 曲線並改 sample_offset_tick，不能把 smoothstep 截取後重新擬合兩端點，否則中段速度會變。音訊 fade 也以獨立時間域保存，分割不能重觸發淡入。

## 渲染順序

1. 從不可變 revision 取輸入、stream、裁切與指紋；先檢查離線或變更。
2. 視覺片段：解碼 → 旋轉／色彩正規化 → 依 source_in 裁切／對應 VFR → CFR → 幾何變換／透明度。
3. 每視覺軌內做已宣告的相鄰轉場，空白區域透明；軌道之間依底至頂 source-over，最底下為工程背景。字幕按指定軌順序最後渲染。
4. 音訊片段：明確 stream 解碼 → source trim → 統一 sample rate／layout → 依原包絡採樣 → clip gain／pan → timeline 定位 → track gain／mute／solo → master mix。
5. 所有輸入依同一精確範圍裁切／補齊；輸出不因某條短音軌提前結束。
6. 產物驗證成功後才可發布，記錄快照、依賴與來源指紋。

FFmpeg overlay、xfade、amix、afade、acrossfade 可作為實作元件，但不能拿 concat 取代多軌合成。xfade 輸入格式、尺寸、fps、timebase 要一致；FFmpeg amix 預設正規化，需明確設定而非依預設讓音量隨活躍輸入數變化。[R4]

本案音訊契約是線性加總既定 gain，預設不自動正規化，不偷偷加 limiter。master 顯示過載警告；如後續增加限制器，須列為明確可編輯效果。原聲只走獨立 audio clip，不能又由 video 解碼路径輸出一次。

## 預覽與字幕

快速預覽允許代理／低解析，但時序及效果參數與 render plan 相同；精確預覽與輸出走同一引擎。ASS 使用相同 libass／字型環境；VTT 保留與燒錄能力分開。更改前方時長、字型、素材指紋、軌序、字幕或 motion 時，需精確失效相關快取。[R6,R9]

多軌快取鍵包含所有影響該區間的圖層／音軌、revision、render engine、轉場 handles；只用檔名或片段 ID 當 key 不足。长片分段渲染要保留跨段轉場與音訊包絡。

## 持久性與生命周期

成功交易需 durable journal 後才回應，變更狀態和冪等結果不可分開落盤。JSON 專案是可交換文件；journal/lock/job database 是使用者工作區側車狀態，不混進原始素材。

使用者層級 daemon 擁有工作，stdio adapter 結束不能誤殺正在輸出的 FFmpeg。只有明確 job_cancel 或應用程式關閉政策可取消；重啟偵測未完成工作標為 interrupted，不假裝成功。詳細規範見 docs/RENDER_JOBS.md。

## 依賴與發行

依原 v0.1.0 要求保留 FFmpeg/libass 能力探測、GPU fallback、依賴鎖定、授權清單與乾淨 Windows 實測。Rust/FFmpeg 不代表全程 GPU；NVENC 只是編碼路徑之一。無 Windows 實測紀錄不可標為 GUI Release 已驗收。
