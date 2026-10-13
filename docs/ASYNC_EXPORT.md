# 背景匯出（Windows，2026-10-13）

GUI、JSON CLI、stdio MCP 現在經由同一工作區 daemon 呼叫共用 Rust 核心。`storycut_render_start` 固定工程 revision 的完整快照，驗證參數與工作區後回傳 `running` 工作，不等待 FFmpeg。啟動端或 MCP EOF 不會取消工作。每個工作區最多同時執行兩個匯出；第三個請求回 `JOB_BUSY`，待工作結束後用新的冪等鍵重試。

## 建置與呼叫

```powershell
cargo build -p storycut-cli -p storycut-daemon --locked -j 1
.\target\debug\storycut.exe --json --workspace D:\StoryCutWorkspace capabilities
.\target\debug\storycut.exe --json --workspace D:\StoryCutWorkspace call storycut_render_start --args-file render.json
.\target\debug\storycut.exe --json --workspace D:\StoryCutWorkspace job get --job-id render-...
.\target\debug\storycut.exe --json --workspace D:\StoryCutWorkspace call storycut_job_cancel --args-file cancel.json
.\target\debug\storycut.exe mcp --stdio --workspace D:\StoryCutWorkspace
```

`render.json` 使用已讀取的 project_id/revision、工作區內的新路徑：

```json
{
  "project_id": "your-project-id",
  "revision": 2,
  "path": "exports/new-output.mp4",
  "overwrite": false,
  "idempotency_key": "export-20261013-001",
  "subtitle_mode": "none",
  "encoder": "h264_cpu",
  "range_start_tick": 0,
  "range_end_tick": null
}
```

輸出父目錄須已存在。`cancel.json` 是 `{"job_id":"render-...","idempotency_key":"cancel-001"}`。MCP 使用相同工具及 JSON。`job_get` 的參數是 `{"job_id":"render-...","include_preview":false}`。

`render_start` 回應或 CLI exit=0 只表示請求已受理。持續查詢到 `succeeded`，再核對 `artifacts` 的路徑、SHA-256 和大小，才是成片完成。失敗有 `failure_code` 與可選的 `failure_message`。`job_get` 查詢成功也可能傳回 `failed`/`cancelled`/`interrupted`；呼叫端須檢查工作終態。

## 狀態、進度與檔案

- `running → succeeded/failed`；取消先回 `cancelling`，worker 完成停止及清理後才回 `cancelled`。已完成工作的重複取消維持原終態。
- 進度來自 FFmpeg `out_time_us`，準備階段可為 null；編碼進度最高 95%，剩餘驗證完成且檔案發佈後才是 100%。不推算假百分比。
- 同一冪等鍵及相同請求立即回同一工作；同鍵不同參數回 `IDEMPOTENCY_CONFLICT`，不阻塞等待第一個 render。
- 既有檔案拒絕覆寫。新檔先寫入工作私有暫存、核對來源雜湊、解碼幀數與音訊，再以不覆寫 hard link 發佈。編碼中出現同名目標會失敗，保留該檔案。
- 取消包含探測、响度量測、編碼、音訊/視訊驗證及雜湊階段；只終止該 worker 擁有的子程序。Windows Job Object 在 daemon 退出時關閉其子程序。
- daemon 重啟後可讀取持久化終態；缺少完成記錄的工作標為 `interrupted`，已有 ready 記錄沿用既有恢復驗證。沒有自動續算。強制中斷可能留下工作私有暫存，不能把它當成完成影片；本輪不清空整個工作區。

GUI 自動輪詢匯出工作；預覽另存狀態，不會蓋掉匯出卡片。重新開啟工程會追蹤此 GUI 上次匯出的 ID，也可貼入 CLI/MCP 回傳的 ID 使用「追蹤工作」。匯出期間仍可編輯；該匯出持續使用開始時的 revision。

## 邊界與回歸

背景匯出只在 Windows daemon 路徑實作；直接可攜 Rust command API 及其他平台維持同步。frame/range preview 仍同步，不能以匯出取消工具取消它。`job_events`、完整重啟續算、GPU 編碼、安裝器的乾淨機器驗收尚未完成。`last_event_seq` 目前只表示狀態轉換，沒有增量事件串流。

CLI 與 daemon 執行檔須相鄰。開發腳本會一起建置它們；portable archive 包含 daemon。自行只建置桌面時，先建置 workspace 的 debug daemon，或把相符版本的 daemon 放在 GUI 執行檔旁。

```powershell
python -X utf8 tests/product_acceptance/async_export.py --cli target/debug/storycut.exe
```

此測試僅自造素材，驗證 MCP 即時回應、CLI/MCP 共用工作、進度、重複提交、取消、終態 schema、固定 revision、60 幀成片、目標競爭、容量上限、EOF 與重啟中斷。測試只停止自己明確建立的 daemon，不終止既有工作。完整結果見 `docs/ASYNC_EXPORT_VALIDATION.md`。

FFmpeg 9 移除了本專案原使用的 `-filter_complex_script`，現在使用官方支援的 `-/filter_complex` 讀取大型 filter graph 檔案：[FFmpeg 命令列檔案載入語法](https://ffmpeg.org/ffmpeg.html#Options)。
