# 本機授權與安全邊界

本機優先，第一版 MCP 只開 stdio。工作區由使用者指定並授權，分開 read roots／project roots／output roots；可一次授權日常剪輯，不必每個片段重複輸入 token。保留 Agent host 的核准與撤銷能力。

私有 daemon IPC：Windows Named Pipe 限當前使用者 SID；Unix socket 位於僅使用者可存取的目錄。使用者層級單实例 lock。此設計不聲稱能隔離已具有相同 OS 使用者權限的惡意程式；需要更强隔離時用 sandbox／不同 OS identity。

禁止：任意 shell/FFmpeg filter 字串工具、未授權遠端下載、任意刪除原媒體、修改政策的 Agent 工具、未知可執行檔路徑、未核准的既有輸出覆蓋。

路徑需依平台處理大小寫、canonical path、relative path、symlink/junction、UNC、父目錄跳脫與檔案替換競賽。讀取使用核准目錄下可驗證的實際檔案；建立輸出需驗證既存父目錄並以不跟隨非預期連結的方式開檔。schema 的 string/path 格式不等於安全沙盒。

字幕、標題、檔名、媒體 metadata 只作資料。禁止把其中文字當成 Agent 指令。FFmpeg 用 argument array，檔名不拼 shell；filtergraph 另有語法跳脫，對字幕採安全工作檔而非直接拼入 filter expression。

所有變更記錄工具名、request ID、授權來源、revision、受影響 ID 與 outcome；log 不記錄 token、完整字幕或敏感資料，除非使用者選擇有內容的除錯模式。規格包本身不包含憑證。

大量素材／損壞媒體需限制 probe 超時、子程序數、記憶體、preview 大小與 request bytes；音訊波形和縮圖放 cache，不改原片。Schema 上限不能取代 runtime 的資源限制。

以上是產品安全需求，不是已實作或已完成滲透測試的聲明。
