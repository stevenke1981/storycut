# 開發順序（規格 v0.2.0）

本版號是規格版號，不是已發布的 App。原本「MCP/CLI 以後再加」的分期已取消。

## M1：共用多軌核心與 headless CLI

Rust model/schema、19 操作、時間轉換、AV link、動畫 domain、journal/revision/undo、來源 bounds。實作 storycut --json call 入口及基本專案/素材/時間軸操作。用相同資料驗證 19 秒與 28 秒範例；此階段單獨不算完成。

## M2：真實渲染與聲音

FFmpeg/ffprobe、多軌合成、透明疊圖、圖片動態、轉場、聲音原位混合、包絡、sample 對齊、字幕保留及燒錄、真實輸出驗證。不能只回指令或假 artefact。

## M3：MCP 與同時操作

官方 rmcp、stdio lifecycle、tools/schema/result、daemon IPC、自動啟動、CLI/MCP 同一 state、衝突／冪等／dry-run、jobs/取消/重启。啟動兩個真實 client 跑同修訂競爭；不是只造兩份 JSON。

## M4：桌面時間軸與打包

Tauri GUI、多軌拖曳/修剪/縮放/吸附、縮圖/波形、鎖軌/音量/mute/solo、字幕編輯、連結標誌、預覽一致性與狀態事件。乾淨 Windows 上驗收 GUI＋CLI＋MCP 三入口；使用者不必安裝開發工具。

## 第一個可用版本門檻

M1–M4 全部核心閉環通過；影片、圖片、聲音及字幕多軌可由 GUI 和 Agent 互相接手。33 工具可分階段落地，但只有完整必備流程真正可用才可標可用版本；未完成項明列限制。

## 後續增強

更多任意關鍵幀、影片變速及保音高、ducking、進階混音、複雜遮罩/調色、更多 GPU 加速、4K 長片效能、Linux/macOS 實機驗收與遠端 Streamable HTTP。這些不得取代或延後本次使用者指定的 MCP/CLI／多軌。
