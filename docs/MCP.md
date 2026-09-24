# MCP Agent 介面契約

## 狀態與目標

本文件及 `contracts/mcp-tools.json` 定義 33 個目標工具；目前 `storycut mcp --stdio --workspace <DIR>` 是可執行的 JSON-RPC/MCP adapter，`tools/list` 僅列已實作工具。以 `storycut_capabilities` 查目前可用集合。以下提及 daemon、背景工作與取消的段落仍是目標契約，尚未落地。成功預覽的 `job_get(include_preview=true)` 已附 PNG image content，但超過 2 MiB 時回明確錯誤，尚未自動縮圖。

目標使用官方 Rust SDK rmcp；先以已核對的 `2025-11-25` 核心能力作相容測試基線，**不是宣稱這是最新協議**。SDK 提供其他日期版本不代表 StoryCut 自動驗收了該版本；發佈時固定版本、協商且只宣告通過測試的能力。[M1–M4]

## 啟動

```text
storycut mcp --stdio --workspace D:\StoryCutWorkspace
```

Agent host 把此命令作子程序啟動；目前 adapter 直接呼叫共用 Rust 指令核心，不需 GUI 開著。沒有私有 daemon。這只適用已授權的本機工作區；不是繞過 host 的工具核准。[M1]

`examples/mcp-launcher.json` 是通用 command/args/transport 描述，不是承諾各 Agent host 都接受同一份設定檔。安裝器可依使用者指定的 host 生成設定；不得未經授權改寫他們既有設定。

第一版不開 HTTP 端口、不支援遠端連線。未來如加入 Streamable HTTP，要依協商版本實作、限定 loopback、驗證 Origin 和認證，不用任意網頁可呼叫的裸 localhost API。[M1]

## 必要協議行為

- UTF-8 JSON-RPC；stdio 一行一個訊息，內容內換行用 JSON 跳脫；stdout 只能放 MCP 訊息，stderr 放 log。[M1]
- 在基線協議中：initialize → initialize response → notifications/initialized → tools/list / tools/call；處理 ping 與正常 EOF。[M2]
- tools capability 必須真的存在；只有真正支援時才宣告 listChanged、resources、prompts、tasks。第一版契約只要求 tools，不因有本機檔案就假裝提供 resources/read。
- tools/list 可分頁；tools/call 的 name 與 arguments 根據 catalog 驗證。回傳 structuredContent，並在 text content 序列化相同 JSON，供相容 client 讀取。[M4]
- 反序列化壞 JSON／未知方法／未知工具等使用協議錯誤。已知工具的 domain 驗證、鎖軌、revision conflict 等回 isError=true，包含公共錯誤 envelope。不得把失敗的字串裝在成功結果中。[M4]
- annotations 是提示不是授權機制。dry-run 工具仍以可能寫入的工具標註，不以 readOnlyHint 掩蓋實際編輯。

`examples/mcp-client-messages.jsonl` 只列 client 送出的訊息；必須等 server 初始化回應後才送 initialized，不能把整份檔案直接盲目 pipe 當成相容測試。

## 工具分組

| 分組 | 主要工具 |
|---|---|
| 狀態與專案 | capabilities、project_create/open/get/save/validate |
| 素材 | media_import、media_list |
| 時間軸 | timeline_get、timeline_apply、track_add/update |
| 片段 | clip_add/move/trim/split/remove、link_create |
| 效果 | motion_set、transition_set、audio_set |
| 字幕 | subtitle_import/update/export |
| 歷史 | history_undo、history_redo |
| 預覽與輸出 | preview_frame/range、render_start |
| 工作 | job_get/list/cancel/events |

所有實際工具名皆以 `storycut_` 開頭；完整參數／結果以 catalog 為準。track.remove/reorder、transition.remove、link.remove、subtitle.shift、subtitle.cue.insert/remove 等由 timeline_apply 的 typed operations 提供，沒有任意 JSON patch 或任意 shell 工具。

所有語義編輯捷徑都轉成 timeline_apply，不能各自寫一套位置或撤銷邏輯。project_validate/read 不會偷偷更新 probe 狀態；media_import 則是有 revision 的文件修改。

## 工作與進度

目前 preview_frame、preview_range 與 render_start 均同步執行，tools/call 回傳時工作已為終態。`job_get` 可回讀持久化工作；`job_events`、背景 daemon 與 `job_cancel` 仍是目標契約，不依賴特定 client 支援 MCP tasks 擴充；初始化不宣告未實作的 tasks。

協議 cancellation/progress 適用進行中的 request；已回傳 job_id 的工作要呼叫 job_cancel，不能拿過期 request_id 取消。取消 adapter 等待或斷線不等於取消 daemon 工作。[M5,M6]

成功的 preview_frame 可經 job_get(include_preview=true) 附 PNG image content，單邊最多 1920 像素，檔案超過 2 MiB 時回明確錯誤；不會假裝縮圖成功。preview_range 回工作區內 MP4 的 artifact 路徑、大小和 SHA-256，不把整部 MP4/base64 塞入工具結果。尚未完成／失敗不提供假產物。

## 安全

本機工作區由啟動 CLI/MCP 的人或桌面版明確指定；每次 tool 由共用核心檢查路徑與編輯條件。輸出目錄與覆蓋策略由工作區政策驗證，不由 Agent 填一個布林就繞過。字幕與檔名只當資料，不能執行其中指令。詳見 SECURITY.md。
