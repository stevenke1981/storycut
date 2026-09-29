# StoryCut 實作與驗收紀錄

## 2026-09-29：夜燈說書製作規格（Night Lantern profile）

依 fish-s2pro-tts《偷桃》v1 實際成片流程補齊能力，規格見 [NIGHT_LANTERN_PROFILE.md](NIGHT_LANTERN_PROFILE.md)。

- 核心：`VideoClip.hold_head_ticks`／`hold_tail_ticks` 凍格把手（有把手片段拒絕修剪／分割、不可連原聲）；`Track.ducking` 側鏈壓低（`track_update` 設定、`null` 清除；來源須為另一條未被壓低的音訊軌）。
- 渲染：把手以 `tpad` clone 重建可見範圍（含範圍全落在把手內）；有 ducking 時改為分軌匯流排＋`sidechaincompress`，否則維持原線性單一 amix；`master_loudness` 兩段式（`ebur128` 量測→線性增益＋必要時峰值限制器）；ASS／SSA 經字幕核心匯出後依 offset／範圍改寫事件時間交給 libass。
- 指令：新增 `storycut_narration_assemble`（MCP／CLI 共 32 項實作工具；契約 36 項）；`render_start`／`preview_range` 接受 `master_loudness`，render 回應附響度報告。
- 驗證：`cargo test --workspace` 全過（含新增 render 5、core 2、command 2 項）；Python 契約 59/59；`tests/product_acceptance/night_lantern.py` 真 CLI PASS，報告於 `target/night-lantern-acceptance/run-*/夜燈說書/report.json`。
- 未做：桌面 GUI 尚無旁白組裝面板（CLI／MCP 可用）；把手片段的修剪／分割；ASS 與其他字幕文件同時燒錄；GPU 編碼。合成素材驗收不等於正式 YouTube 成片驗收。

## 2026-09-30：NL-01 storycut_narration_assemble outputSchema 修正

- 問題：`storycut_narration_assemble` 的 `outputSchema` 是從 `storycut_storyboard_assemble` 複製而來，`data` 的 properties／required 與 `narration_assemble()` 實際回傳不符（含 `plan()` 的 `segment_offsets`、`cuts`、`native_cores`、`visual_track_id`、`narration_track_id`、`frame_ticks`、`dissolve_window_ticks`、`lead_in_ticks`、`narration_end_tick`、`total_duration_ticks`、`created_track_ids`，以及 `narration_assemble()` 加的 `committed`、`base_revision`、`changed_ids`、`duration_ticks`）。
- 修正：重寫 `contracts/mcp-tools.json` 中 `storycut_narration_assemble` 的 `outputSchema.allOf[0].then.properties.data`：properties 改為上述所有實際欄位；`required` 列出所有非 nullable 欄位；nullable track ID（`music_track_id`、`overlay_track_id`）用 `anyOf[string, null]`、不放入 required；`additionalProperties` 改為 `true`（plan 回傳可能附帶額外欄位）。
- 測試：在 `crates/storycut-mcp/tests/protocol.rs` 新增 `narration_assemble_output_matches_schema`：正向範例以假 dispatch 回傳含所有必填欄位的 response，確認 `structuredContent.data` 的各欄位存在且型別正確；負向範例確認缺少 `narration_end_tick` 的 fixture 確實沒有該欄位。
- 驗收：`cargo test -p storycut-mcp protocol` 新測試通過；`python -X utf8 tools/validate_spec.py` 通過。

## 2026-09-27：夜燈說書工作流程

本節是本次新增功能的驗收；下方 2026-09-24 紀錄保留為既有基線。

- 新增 `storycut_storyboard_assemble` 與 `storycut_focal_motion_apply`，GUI、CLI、MCP 共用同一 Rust 交易核心。圖片預設 10 秒；支援有序混合影片、連結原聲、旁白／角色對話／循環配樂，以及手動焦點推近與四方向平移。實際工具清單新增兩項，共 31 項；catalog 的 35 項包含尚未實作的目標工具。
- 桌面新增說書排片面板、唯讀來源縮圖、焦點選擇與批次運鏡；提供片段音量／淡入淡出／靜音、軌道名稱／音量、長片適合視窗和數字跳轉，以及外部 Agent 修改同步。
- 高階規劃在工程鎖內檢查修訂、規劃、提交與持久化冪等結果；已提交請求在其他修改或 undo 後重播，不會再新增片段。桌面試算綁定原修訂與同一冪等鍵。
- renderer 只讀取目前輸出範圍內實際參與合成的素材；大型 filtergraph 改用有界暫存檔，並限制輸入解碼執行緒。修正音訊淡出公式把淡出起點誤當結束的錯誤，保留真音訊回歸測試。

### 真媒體與原生視窗證據

| 項目 | 實際結果 |
|---|---|
| 72 張圖片完整故事 | `tests/product_acceptance/story_workflow.py` 經真 CLI 建立 72 張 320×180 圖片、2 秒含原聲影片、旁白／對話及循環配樂，共 180 片段。真 MCP 子程序套用全部圖片運鏡；輸出 722 秒、30 fps、21,660 幀 MP4。每張圖片 10 秒；來源 76 檔 hash 前後相同。 |
| 逐幀焦點與放大 | 首／中／尾抽幀中，主體中心由 `(216.5,62.5)` 移至 `(159.5,90.5)`，標記寬由 18 至 32 像素；符合向主體推近的方向及放大範圍。 |
| 混音與片尾淡出 | 頻譜量測旁白僅在指定 1 秒起段落、角色對話僅在 4 秒起段落出現；7.5 秒仍有循環配樂。片尾 720.5／721.5 秒配樂 220 Hz 振幅 561.8／306.4，原影片 660 Hz 原聲仍存在。舊淡出公式在 721.5 秒配樂振幅只有 0.1；修正後重跑完整成片通過。 |
| 長片範圍預覽 | 同一 722 秒工程的 711–713 秒預覽實測 0.888 秒完成；完整低解析度成片耗時 210.558 秒。這是本機一次量測，不是 4K 或通用效能保證。 |
| 原生桌面組裝 | Windows Tauri release 視窗用原生選檔對話框開啟中文路徑工程，GUI dry-run 未改 revision；套用後磁碟為 72 張 10 秒圖片、5 軌、180 片段。縮圖實際解碼為 640×360，點選焦點後設定 160% 推近，核心工程 motion 回讀正確。 |
| 原生播放與音訊編輯 | 真核心 6 秒 MP4 在 WebView 播放：`readyState=4`、`error=null`、`paused=false` 且時間超過 1 秒。GUI 將對話改為 -9 dB、淡出 0.5 秒並靜音後，獨立工程檔回讀一致。CLI 外部更名軌道後 GUI 自動顯示 revision 7 與新名稱；數字跳轉至 711 秒通過。 |
| 最終桌面版本回歸 | v7 試算後，CLI 外部修改至 v8，GUI 自動停用舊「套用組裝」。重新試算提交 v9 產生 181 片段／6 軌；GUI 復原 v10 回 180／5、重做 v11 回 181／6、再復原 v12 回 180／5。此驗收找到並修正既有 Undo/Redo 少傳 `dry_run` 的問題。v13 重新點選圖片焦點、設定 160% 推近，6 秒原生 MP4 真播放至 2.032 秒；批次清單顯示 72 個素材檔名。 |
| 最終自動檢查 | `cargo test --workspace --locked` 72/72、桌面 `cargo test --manifest-path desktop/src-tauri/Cargo.toml --locked --lib` 4/4、Python 契約測試 59/59 通過。`validate_spec.py` 驗證 35 工具契約及 7 份輸入範例；Rust 格式、diff check、TypeScript/Vite、CLI release 與 Tauri release 建置通過。CI 已加入完整 72 圖真媒體腳本，尚未在 GitHub runner 執行此新增步驟。 |

完整真媒體報告：`target/story-workflow-acceptance/run-44j_3c_j/夜燈說書/report.json`；成片與焦點首中尾圖保留在同目錄。原生操作證據：`target/native-story-acceptance/run-_gwer896/native-workflow-report.json`、`native-final-report.json` 與同目錄截圖。報告各自記錄受測 EXE SHA-256；完整成片在最後參數驗證及 UI 修正前產生，最後修正另有單元與原生回歸。這些均是隔離驗收素材，未使用或修改正式 YouTube 原始素材。

自動人物辨識、語意對齊旁白、自動配樂 ducking 尚未提供。非常規 SAR／旋轉中繼資料的焦點座標尚未完成端到端驗收；一般方形像素 PNG 的焦點／運鏡已實測。渲染仍同步等待；背景取消與 GPU 限制見下方。

## 2026-09-24 基線

日期：2026-09-24。執行環境：Windows、Rust 1.94.1、Node.js 24.20.0、npm 11.17.0、FFmpeg/ffprobe 8.1.1（有 libass）。本頁記錄實際執行的結果；`VALIDATION.md` 是 2026-09-18 原始規格包的歷史檢查紀錄。

## 已實作

- Rust `storycut-core` 是 GUI、JSON CLI、stdio MCP 共用的唯一工程狀態與操作入口；工程有 revision、批次交易、dry-run、冪等結果、undo/redo 與磁碟側車。
- 多軌圖片／影片／聲音／字幕模型，19 種 `timeline_apply` 操作；實際可呼叫工具以 `storycut_capabilities` 與 MCP `tools/list` 為準。
- FFmpeg/ffprobe 真素材探測、PNG 畫格預覽、CPU H.264/AAC MP4 渲染、基本交叉淡化與 SRT 純文字字幕燒錄。
- Rust renderer 支援 linear/smoothstep 圖片／影片逐幀平移、縮放與透明度，分割後依 `sample_offset_tick` 延續原曲線。
- Agent 可呼叫 `storycut_preview_range` 輸出含混音的短 MP4；工作檔以暫存、同步、SHA-256 驗證及不可覆寫發布，工作記錄與字幕輸出 intent 採原子發布。
- SRT、ASS、SSA、WebVTT 解析與 sidecar 匯出；原始文件及未知語法保留，跨格式有樣式流失報告，需 `allow_lossy` 才可輸出。
- Tauri 2 + React 原生視窗與共同 Rust dispatcher bridge。GUI 顯示素材庫、預覽、時間軸、屬性、字幕與匯出控制。
- Windows daemon/client 的獨立 IPC 骨架已具備目前使用者 SID 限制的 named pipe、單一工作區 lease、有界請求佇列及超載回應；CLI/MCP/GUI 仍未切換到此 daemon，渲染工作也尚未背景化。

## 實際執行驗收

| 項目 | 證據 |
|---|---|
| CLI 工程與交易 | 不同程序建立、讀取、匯入真 PNG/WAV、dry-run 不加 revision、commit 加 revision、同鍵重試不重複、過期 revision 拒絕；undo/redo 回讀通過。 |
| 多軌實際輸出 | `target/acceptance-ec269004/crossfade-19.mp4`：ffprobe 量到 19.000 秒、30 fps、570 幀；0–19 秒音軌存在，9.5 秒抽幀可見紅／藍混合。 |
| 音訊起點 | 獨立 CLI 工程 `target/audio-acceptance-stdlib/run-_gqf1f5i/audio-offset.mp4`：5.000 秒、30 fps、150 幀。48 kHz PCM 解碼後，1.5–1.9 秒與 3.1–3.5 秒 RMS 均為 0；2.1–2.9 秒約 7998，符合指定在第 2 秒起播放 1 秒音訊。`tests/product_acceptance/audio_offset.py` 每輪建立全新工作區、素材、工程與報告，避免重播舊成功工作。 |
| 字幕實際輸出 | `target/acceptance-ec269004/burn-19-2.mp4`：ffprobe 量到 19 秒、570 幀；`burn-subtitle-frame.png` 的第 2 秒抽幀可見「StoryCut 字幕驗收」。SRT 匯出檔 hash 與回傳 artifact 相同。 |
| 並行與恢復 | 兩個獨立 CLI 程序以同一渲染鍵競爭，皆回同一 succeeded job/同一檔案；模擬只留下 initial record，`job_get` 回 `JOB_INTERRUPTED`。模擬 ready manifest 已落盤而最終檔尚未發布，以及最終檔已發布但 result 尚未落盤，兩種情境都由 `job_get` 核對 SHA-256 後恢復 succeeded result。 |
| 工作區限制 | 修改工程素材路徑指向授權工作區外時，`project_open` 與 `render_start` 都回 `PATH_DENIED`；`.storycut` Windows junction 指向外部目錄時 `job_get` 回 `PATH_DENIED`，外部目錄未寫入。 |
| MCP | 真 `storycut.exe mcp --stdio` 子程序完成 initialize、tools/list、tools/call、ping、EOF；stdout 僅 JSON-RPC，stderr 空白。對已完成 preview job 呼叫 `job_get(include_preview=true)`，收到一筆 `image/png` content；base64 解碼後與磁碟上 627-byte PNG 完全相同，text 與 structuredContent 一致。 |
| 短片預覽 | `storycut_preview_range` 由 CLI 在 `target/acceptance-ec269004/.storycut/previews/preview-range-8d1efbe5246fbc44a6af.mp4` 真實渲染 3 秒 H.264 MP4，ffprobe 量到 320×180、30 fps、90 幀、AAC 48 kHz；解碼音訊 mean volume -24.1 dB，抽幀可見燒錄 SRT。`job_get` 回同一 artifact；同鍵重送 SHA/ID 不變、異 payload 回 `IDEMPOTENCY_CONFLICT`。 |
| MCP 短片預覽 | release `storycut.exe mcp --stdio` 真子程序完成 initialize、tools/list、project_get、preview_range、job_get、ping、EOF；`target/acceptance-ec269004/mcp-range-report.json` 記錄 2 秒、30 fps、60 幀 MP4，回傳 artifact 的 54,762 bytes 與 SHA-256 均和磁碟吻合，stderr 空白。 |
| 動態關鍵影格 | `target/motion-acceptance-6c0f/run-whzuzljn/` 的 5 支真 MP4 各 640×360、30 fps、2 秒、60 幀；非對稱標記中心四方向實測符合設定。110%→118% 縮放的 bbox 寬 78→84 像素，比例 1.0769（設定比 1.0727）。分割／未分割版本逐幀比對 60 幀 MAE=0；`motion-contact-sheet.png`、`report.md` 與基線失敗紀錄均保留。 |
| 規格資料 | `python -X utf8 tools/validate_spec.py` 通過；`python -X utf8 -m unittest discover -s tests -v` 58/58 通過。這些不是產品渲染驗收。 |
| 原生視窗 | `npx tauri dev` 編譯並啟動原生 StoryCut 視窗；Win32 UI Automation 操作 Windows 選檔視窗，載入 `target/acceptance-ec269004/demo.storycut.json`。GUI 點「新增圖片軌」經共用核心提交後 revision 7→8、軌數 3→4；獨立 CLI `project_get`/`timeline_get` 讀到同一 revision 與新軌。GUI 再點「產生精確影格」，真核心 preview job succeeded，安全 bridge 顯示 1280×720 PNG；CLI `job_get` 驗得同一 4350-byte/SHA-256 artifact。畫面見 `target/storycut-native-before.png`、`target/storycut-native-preview.png`。 |
| 發行版 GUI 播放與修剪 | 最終 release EXE 用 Windows 原生選檔視窗開啟同一測試工程，顯示 v8、3 素材、4 軌、3 片段與完整 19 秒時間碼。GUI 產生的 6 秒 MP4 在 WebView 真播放：`readyState=4`、`duration=6`、`error=null`，按播放後 `paused=false` 且 `currentTime` 前進，畫面見 `desktop/acceptance/native-release-preview-playing.png`。在原生時間軸拖曳旁白片段右端 0.5 秒後，GUI v8→v9；獨立 CLI `timeline_get` 回讀該片段長度 3.5 秒（`duration_ticks=2469600000`）。這些操作只修改 `target/acceptance-ec269004` 測試工程。 |
| GUI 匯出 | Tauri 原生開發視窗送交 `render_start`，CLI `job_get` 獨立回讀 `render-6da12b1c1f90ced07f5f` 成功、revision 8、137,609-byte MP4。`ffprobe` 驗得 19 秒、640×360、30 fps、570 幀與 19 秒 AAC；成片 `target/acceptance-ec269004/實際程序驗收.mp4`。此項是在原生開發視窗驗收，未於最終 release EXE 重做。 |
| Windows 發行建置 | Tauri release `--no-bundle` 已產生 `desktop/src-tauri/target/release/storycut-desktop.exe`；Tauri bundle 已產生 MSI 與 NSIS 安裝器，位置在同目錄 `bundle/msi/`、`bundle/nsis/`。這只證明建置／打包成功，尚未證明安裝器在乾淨 Windows 上可啟動或解除安裝行為。 |
| 可攜版 | `dist/StoryCut-0.1.0-portable-windows-x64.zip` 含 9 個項目；解壓後的 GUI/CLI SHA-256 與各自 release 執行檔一致，文件 SHA-256 與工作區版本一致。解壓 CLI 在新工作區執行 `capabilities` 回 `ok=true`、29 項工具；解壓 GUI 在本機啟動，將最小化的視窗還原後，未重新載入頁面即顯示完整 StoryCut 編輯器，畫面見 `desktop/acceptance/native-portable-restored-without-reload.png`。這不是乾淨 Windows 安裝測試。 |
| Windows IPC 骨架 | 獨立 daemon/client 測試涵蓋 Unicode 工作區、單一 writer、18 個 client 競爭、16 深佇列滿載、`OVERLOADED`、重複 owner、程序結束後續接及 pipe-slot timeout；這些通過不代表 CLI/MCP 已使用 daemon 或背景工作可取消。 |
| 自動測試 | `cargo test --workspace --locked` 通過 63 項 Rust 測試（含真 FFmpeg integration 與 Windows IPC 程序測試）；`.venv/Scripts/python.exe -X utf8 tools/validate_spec.py` 與相同環境的 58 項 Python 規格測試通過。Python 測試只驗資料契約。 |

## 已知限制與未驗收

- GUI/CLI/MCP 的渲染與預覽目前由呼叫端同步等待；MCP 單一連線在長渲染時無法同時處理另一請求。daemon IPC 骨架尚未接入這三種入口；`job_cancel`、背景工作與實際中途取消尚未完成。已連線 daemon 的同步請求沒有讀寫期限，未完成訊框可能卡住 dispatcher。ready manifest 前中斷的工作會標示 `JOB_INTERRUPTED`，不自動重試渲染；ready 後可驗證既有產物並恢復發布。
- GPU/NVENC、所有轉場、ASS/VTT/SSA 樣式燒錄、高解析度長片效能尚未完成。來源影格或放大圖層超過 16,777,216 像素時在啟動渲染前回 `UNSUPPORTED_FEATURE`，避免巨大記憶體配置；其他未支援效果亦回明確錯誤。
- GUI 已在 native 對真工程完成開啟、加軌、精確影格與短片預覽、拖曳修剪及一次匯出；字幕編輯等其餘操作仍須逐項桌面驗收。瀏覽器 demo 不能當成這項證據。
- Windows MSI/NSIS 已建置、尚未實際安裝；可攜版已在目前 Windows 主機解壓並啟動，尚未在乾淨 Windows 驗收。媒體功能需 FFmpeg/ffprobe；開發建置另需 Rust、Node。
- RecordScreen 錄影未產生：當次環境沒有可用 RecordScreen MCP/loopback 服務，啟動程序被工具政策拒絕。原生視窗截圖不等於錄影。

`ACCEPTANCE.md` 的其他項目維持待驗，不因單元測試、schema 檢查、編譯或 demo 畫面而標記通過。
