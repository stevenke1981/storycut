# StoryCut 實作與驗收紀錄

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
- GPU/NVENC、所有轉場、ASS/VTT/SSA 樣式燒錄、長片效能尚未完成。來源影格或放大圖層超過 16,777,216 像素時在啟動渲染前回 `UNSUPPORTED_FEATURE`，避免巨大記憶體配置；其他未支援效果亦回明確錯誤。
- GUI 已在 native 對真工程完成開啟、加軌、精確影格與短片預覽、拖曳修剪及一次匯出；字幕編輯等其餘操作仍須逐項桌面驗收。瀏覽器 demo 不能當成這項證據。
- Windows MSI/NSIS 已建置、尚未實際安裝；可攜版已在目前 Windows 主機解壓並啟動，尚未在乾淨 Windows 驗收。媒體功能需 FFmpeg/ffprobe；開發建置另需 Rust、Node。
- RecordScreen 錄影未產生：當次環境沒有可用 RecordScreen MCP/loopback 服務，啟動程序被工具政策拒絕。原生視窗截圖不等於錄影。

`ACCEPTANCE.md` 的其他項目維持待驗，不因單元測試、schema 檢查、編譯或 demo 畫面而標記通過。
