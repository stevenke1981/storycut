# 背景匯出實測紀錄｜2026-10-13

此輪由 main `d509bb791667026ac217630d524b1c7b83836e1b` 建立獨立 `feat/async-export-jobs`。遠端夜燈分支 `1c08d46` 與 main 各有 6 個獨有提交；没有合併或重建工程。本機工作目錄原為空，未找到 StoryCut checkout 或同範圍活動，使用公開來源完整 clone。GitHub connector 確認登入 `stevenke1981`、repository push 權限；本機 gh 舊憑證無效，未改認證或安全設定。

## 交付範圍

- Windows GUI/CLI/MCP 改走共享 workspace daemon；匯出在背景 worker 跑，其他指令可繼續使用。
- 真實進度、取消與 durable 終態／失敗原因；固定工程快照、同鍵 replay、兩匯出容量上限。
- 保留既有目標、編碼中出現的同名檔案與預先存在的私有 stage；取消只清理本 worker 的新 stage。
- GUI 自動輪詢、重開工程追蹤 ID、貼入 CLI/MCP 的工作 ID；預覽不蓋掉匯出卡片。
- daemon 加入 portable archive 與桌面 bundle resource；原有 SID pipe／lease／路徑授權不變。

## 實際環境與結果

本機 Windows x64、Rust 1.99.0、Node 24.21.0、Python 3.14.8、FFmpeg/ffprobe 9.0.1 Gyan essentials（有 libass），所有媒體均為自造素材。沒有讀取私人錄影／音訊，沒有使用付費模型。編譯均以一個 Cargo job 執行。

| 驗證 | 結果 |
|---|---|
| Rust workspace | 87 項通過，另有 1 個 subprocess fixture ignored；包含真 FFmpeg render、字幕、旁白、響度、交易、MCP 與 Windows IPC |
| Desktop bridge | 4 項通過；debug GUI 可建置，資源內含 matching daemon |
| 契約與範例 | 61 項通過；36 個目標工具、19 操作、3 個輸出範例；不是 App runtime 證據 |
| Frontend | TypeScript + Vite build 通過 |
| Lint | workspace 與 desktop 全 target Clippy 通過，新增 jobs/control/windows_spawn 模組無警告；既有 Rust 1.99 style／deprecated warnings、縮圖 borrow 與測試模組排列警告保留，`-D warnings` 並非全乾淨 |
| Format／diff | root Rust format、desktop Rust format、git diff whitespace 檢查通過 |
| CLI auto-start | stdout/stderr EOF 3.1123 秒；daemon 在 CLI 結束後存活；中文、空白、尾端 separator 通過 |
| Audio offset | 真實 MP4 解碼 240,000 samples，2 秒前／3 秒後 RMS=0，2–3 秒音訊 RMS 約 7,998 |
| 72 圖故事 | 完整 722 秒、21,660 幀成片；本次 render 125.223 秒，旁白／角色聲道／配樂、運鏡、來源只讀通過 |
| Night Lantern | 1,524 幀成片；原生凍格核心、置中淡化、alpha 卡片、ASS、ducking、主控 −16.0 LUFS 與來源只讀通過 |
| Async export | MCP 提交 49.7 ms，同連線 ping 0.1 ms；真進度 2.6311%；取消、固定 revision、60 幀成片、失敗舊檔保護、容量、獨立並行取消、EOF／重啟中斷及私有 stage 保留通過 |
| UI 實際畫面 | Edge 155 headless，built UI 連接真 CLI/daemon；running/cancelled/succeeded/failed、匯出中真 PNG 預覽、重開追蹤通過，0 page errors |

每個產物都由 ffprobe／解碼與 SHA-256 驗證；queued/running、CLI exit=0 或 schema pass 沒有當成成片成功。

## 回歸中修正的實際缺口

1. 自動啟動的 Windows daemon 原繼承 CLI 管線，CLI 已退出仍無 EOF。改用明確 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`，只繼承 NUL，不調整全域 handle／security。[Windows 繼承規則](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute)
2. 大型 IPC 回應寫完立即 disconnect 丟失未讀 bytes，76 個素材已提交但 CLI 收到 pipe error。斷線前 drain；55,000 字元回應及 72 圖成片驗證通過。[Windows named pipe 操作](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-operations)
3. MCP capabilities 新欄位須同步更新 success 分支 schema，已以真 stdio 回應驗證。
4. FFmpeg 9 移除 `-filter_complex_script`，改用 `-/filter_complex` 檔案載入語法。[FFmpeg 官方說明](https://ffmpeg.org/ffmpeg.html#Options)
5. FFmpeg 9 将合法 1000 Hz PCM WAV auto-probe 成 MPEG-TS；指定 WAV 可正常解碼。探測、編碼與響度量測均依實際 RIFF/RF64/RIFX/BW64 + WAVE 簽名指定 demuxer，不依副檔名猜測。
6. 真截圖發現 toast 遮住工作卡片及錯誤文字不換行，已移開 toast、補捲動、換行與按鈕尺寸。
7. 第一輪 CI 的 FFmpeg 8.1.1 取消停在 `cancelling` 超過 30 秒，72 圖與 Night Lantern 已通過。Windows 啟動器退出後，後代程序可能仍持有 stdout／stderr；現在在 join 讀取器前關閉 worker 自己的 Job Object，停止所屬程序群組。新增真啟動器／後代持管線測試，取消在 0.06 秒完成；維持原本 30 秒終態驗收門檻，推送後再次核對 CI。

## 可追溯證據

這些本機 report／媒體／PNG 位於 ignored `target`，來源原檔保留：

- `target/final-rust-tests.log`、`target/final-desktop-tests.log`、`target/final-desktop-build.log`、`target/final-clippy.log`、`target/final-desktop-clippy.log`、`target/final-ipc-tests.log`、`target/ci-cancel-fix-test.log`。
- `target/auto-start-acceptance/run-ls37s_vm/自動啟動 with spaces/report.json`。
- `target/audio-acceptance-stdlib/run-7ystvihr/report.json`。
- `target/story-workflow-acceptance/run-q4boa4mg/夜燈說書/report.json`。
- `target/night-lantern-acceptance/run-9tiq6o9r/夜燈說書/report.json`。
- `target/async-export-acceptance/run-u0eml5ye/非同步 export/report.json`。
- CI 取消修正後重跑：`target/async-export-acceptance/run-0ra25vq0/非同步 export/report.json`（PASS），`target/ci-cancel-fix-build.log` 與 `target/ci-cancel-fix-acceptance.log`。
- `target/ui-export-evidence/{report.json,bridge-calls.jsonl,running.png,cancelled.png,succeeded.png,failed.png}`。

CI 已加入 auto-start／async lifecycle gate 與 report 上傳；推送 SHA 與 CI 執行 URL 以交付訊息為準。

## 明確未驗／限制

原生 AutoGui session 回報 cursor-unavailable，不能擷取／操作桌面；內建瀏覽器隔離啟動器也缺平台目錄。沒有解鎖、轉移 session、調整安全設定或繞過拒絕。UI 測試替換的是 Tauri transport 與檔案 picker，回應和媒體都由真 CLI/daemon 產生；不能算原生 WebView2、dialog 或實體鍵盤／滑鼠驗收。

未做乾淨機器 MSI/NSIS 安裝、GPU、其他 OS、disk-full／突然斷電、完整重啟續算或增量 job_events 驗收。preview_frame/range 仍同步；post-connect IPC read 尚無 deadline。中斷可標示 interrupted／核對既有 ready，沒有自動續算；強制停止可能保留工作私有暫存。只停止本輪明確建立的測試 owner，不終止其他專案程序。沒有 merge、release 或部署。
