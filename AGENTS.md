# StoryCut 開發 Agent 規範

本包為規格與接口，不是 App 實作。先讀 README、SPEC、ARCHITECTURE、docs/TIMELINE 和 TRANSACTIONS。

## 必須保留

- 主線 Rust，Windows GUI 使用 Tauri；CLI/MCP 無 GUI 也可用。
- MCP、CLI、多軌影片／圖片／聲音／字幕均為第一個可用版本必備。
- GUI/CLI/MCP 的所有修改共用核心；不自建第二份時間軸。
- 使用者既有媒體只讀；不自動刪除、覆寫、上傳或授權新工作區。
- 不虛報 stub、模擬 response、schema pass 或 FFmpeg 指令文字為實際成功。

## 實作方式

逐項實作 contracts 類型與 cross-field 驗證，再加入 Rust 單元測試、MCP protocol 測試、CLI 程序測試、真實媒體渲染與桌面測試。schema 變更要更新範例、負向測試、文件與 migration。

capabilities 與 tools/list 只列當前實際可用工具；不把這份 33 項目標清單原樣硬回成「全部已支援」。未實作功能用明確 UNSUPPORTED_FEATURE，不返回假 job_id。

MCP stdout 必須只有 protocol；CLI JSON stdout 不能混入 log。所有副作用要先 schema/授權/expected_revision 檢查；冪等判定及交易寫入與歷史同時持久化。

## 驗收與交付

保留原本 A01–A24 加新增驗收項。必須量測真實視訊幀数、音訊 offset、分割後運動連續性、字幕輸出、取消／重启、中文路徑、鎖軌和兩 Agent 競爭。測試報告需列出真的環境，不能冒充互不共享狀態的多 Agent 評審。

原始碼交付應有 Cargo.lock、前端 lockfile、Windows scripts、CI、license inventory、GUI/CLI/MCP 啟動說明、可攜版／安裝包與發行驗收紀錄。沒測過的 OS 不寫已驗收。
