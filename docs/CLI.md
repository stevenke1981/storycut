# CLI 契約與目前實作

`cargo build -p storycut-cli --locked` 會產生 `target\\debug\\storycut.exe`。標準入口與部分 alias 已可執行；`--jsonl`、`job events`、取消工作及背景 daemon 尚未實作。實際工具請先執行 `storycut --json --workspace <DIR> capabilities`。所有路徑均限於明確指定的工作區。

## 單一標準入口

```text
storycut --json call <MCP-tool-name> --args-file <request.json>
```

CLI 用與 MCP 完全相同的參數 schema 和回應 envelope，不必做 shell 的複雜 JSON 跳脫。`--args-file -` 從 stdin 讀一個 JSON object；schema 拒絕未知欄位、非有限數字與非法 ID。stdin 中字幕含引號／多行仍按 JSON 資料處理。

便利 alias 對應 `contracts/cli.json`：例如 storycut_timeline_apply 對應 `storycut --json timeline apply --request file.json`。--request 與各個便利欄位 flag 互斥。

```powershell
# 以下需要先建立真正工程並以回傳的 ID、revision 和素材路徑替換示例值。
storycut --json project open --file "D:\StoryCutWorkspace\episode.storycut.json"
storycut --json timeline get --project-id "demo-multitrack" --start-tick 0

# 精確 request 必須以剛讀到的 project_id、revision 及 asset ID 產生。
storycut --json timeline apply --request ".\examples\timeline-apply.dry-run.json"
storycut --json timeline apply --request ".\examples\timeline-apply.commit.json"
storycut --json call storycut_subtitle_import --args-file ".\examples\subtitle-import.json"
storycut --json render start --request ".\examples\render-clean.json"
storycut --json job get --job-id "<上一步回傳的job_id>"
```

範例檔中的 demo-multitrack、revision 7/8/9 是 fixture 數值，**不能當成通用真實專案 ID**。須先匯入實際素材再以回傳值替換，不要硬編碼 UI 顯示的 V1/素材檔名當作 API ID。

## 便利旗標的明確映射

| alias | flags 與缺省 |
|---|---|
| project open | --file → path |
| project get | --project-id |
| timeline get | --project-id；--start-tick 預設 0；--end-tick 預設 null |
| job get | --job-id；--include-preview 預設 false |
| capabilities | 空 object |
| 所有其他 alias | 第一版一律 --request JSON_FILE；不可用未定義 flags 宣稱可用 |

圖片／影片／聲音加入都走 clip add：其 --request 包含 typed clip；不靠不同 CLI 私自建立不同專案格式。

## stdout、stderr 與結束碼

--json 一次輸出一個 envelope object，不混進開機 banner、progress bar、FFmpeg 日誌。正常互動文字模式可以人類可讀，但 Agent 應用 --json。

`--jsonl`、`job events --follow` 尚未實作。`render_start` 與 `preview_frame` 目前同步執行，回傳時工作已經終止；可再用 `job_get` 查持久化結果。中斷後若已持久化 ready manifest，`job_get` 可核對 SHA-256 並完成發布；更早中斷會回 `JOB_INTERRUPTED`。實際中途取消尚未實作。

`storycut_preview_range` 可輸出含混音的短 MP4，亦為同步工作。請求需包含 `project_id`、固定 `revision`、`idempotency_key`、`width`（160–1920 的偶數）、`include_subtitles`、`start_tick` 與 `duration_ticks`。起點須對齊工程影格，範圍不得超出工程長度；成功後從 `data.job.artifacts` 取得工作區內 MP4 相對路徑、大小及 SHA-256。此工具不把整段影片嵌入 MCP 回應；可再用 `storycut_job_get` 回讀同一工作。

stderr 用於可安全公開的診斷；不輸出環境 token 或整個敏感路徑列表。MCP 子命令忽略人類輸出偏好，stdout 嚴格只輸出協議訊息。

結束碼：0 命令回應成功（渲染工作本身仍須檢查 `data.job.state`）；2 參數錯誤；3 找不到；4 衝突／鎖軌；5 權限／禁止覆蓋；6 不支援／缺媒體；7 渲染／I/O；8 明確取消；10 內部錯誤。完整目標碼見 contracts/errors.json。

`retryable=true` 只表示處理原因後可再試，不代表安全無限重試。revision conflict 必須重讀規劃；重試同一已提交命令必須用相同冪等 key。

## 安裝行為

Windows 安裝器與可攜版的發行驗收狀態見 [實作紀錄](IMPLEMENTATION_STATUS.md)。開發建置需 Rust、Node、FFmpeg/ffprobe；可攜版啟動不需 Rust 或 Node，但媒體功能仍需 FFmpeg/ffprobe，詳見 [可攜版說明](PORTABLE_WINDOWS.md)。
