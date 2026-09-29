# StoryCut｜影片剪輯器與 Agent 介面

目前提供 Rust 共用剪輯核心、Tauri 2 + React 桌面介面、JSON CLI、stdio MCP 與 FFmpeg 渲染。工作區內的工程、素材、時間軸和歷史由同一核心管理；桌面版和 Agent 指令共用 `storycut-command`。

這是開發中的可執行版本。實際可用工具以 `storycut_capabilities` 和 MCP `tools/list` 為準。尚未完成背景工作取消／完整重啟恢復、全部字幕樣式、GPU 編碼與發行安裝驗收；[實作與驗收紀錄](docs/IMPLEMENTATION_STATUS.md) 列出已跑過的項目與限制。`contracts/` 的工具包含目標契約，並非全部已實作。

## 夜燈說書製作

故事組裝支援依序加入大量圖片／影片，圖片預設各 10 秒，可搭配旁白、角色對話與循環配樂。圖片可點選主體焦點，套用推近、拉遠與上下左右平移；核心會限制取景邊界。桌面版、CLI 與 MCP 共用 `storycut_storyboard_assemble` 和 `storycut_focal_motion_apply`，詳細流程見 [夜燈說書操作指南](docs/STORY_WORKFLOW.md)。

## Windows 開發啟動

需要 Rust、Node.js、npm、FFmpeg/ffprobe（在 `PATH`，字幕燒錄需 FFmpeg 有 libass）。在本目錄執行：

```powershell
cargo build -p storycut-cli --locked
cd desktop
npm ci
npm run tauri dev
```

桌面版透過原生 Tauri bridge 呼叫共用 Rust 指令核心。命令列可獨立執行；先建立自行授權的工作區目錄，將要匯入的媒體放在該目錄內：

```powershell
.\target\debug\storycut.exe --json --workspace D:\StoryCutWorkspace capabilities
.\target\debug\storycut.exe --json --workspace D:\StoryCutWorkspace call storycut_project_create --args-file request.json
.\target\debug\storycut.exe mcp --stdio --workspace D:\StoryCutWorkspace
```

`--args-file` 的請求格式見 [CLI 說明](docs/CLI.md) 和 `contracts/mcp-tools.json`。CLI JSON stdout 只輸出一個回應；MCP stdout 只輸出協議訊息。輸出檔不可覆寫既有檔案，素材匯入只讀取原檔。`scripts/dev.ps1` 與 `scripts/build.ps1` 提供 Windows 入口。

## Windows 發行檔

`scripts/build.ps1 -Target all` 會建置 CLI、桌面程式、MSI 與 NSIS 安裝器；`scripts/build.ps1 -Target portable` 會產生含 GUI 與 Agent CLI/MCP 的 ZIP。發行檔的啟動與外部依賴見 [可攜版說明](docs/PORTABLE_WINDOWS.md)。媒體探測、預覽與輸出仍需另外安裝 FFmpeg/ffprobe；字幕燒錄需有 libass。建置成功不等於乾淨 Windows 安裝驗收，實測結果以 [實作與驗收紀錄](docs/IMPLEMENTATION_STATUS.md) 為準。

## 設計契約

1. MCP 與 CLI 改為「第一個可用版本的必備功能」，不再延後。
2. 加入真正多軌：影片／混合畫面、圖片疊加、聲音與字幕。圖片可放主影片軌，也可放獨立圖片軌。
3. GUI、CLI、MCP 必須共用同一 Rust 指令核心、工作區狀態、歷史紀錄與渲染計畫。
4. 影片原聲是連結的獨立音訊片段，支援同步移動／分割／裁切；不可同時再從影片路徑重播原聲。
5. 提供 MCP 工具契約、19 種原子時間軸操作，以及版本衝突／冪等／dry-run／批次回滾／工作取消規範；可用集合以 capabilities 為準。

保留：圖片設定秒數、上下左右平移與縮放、影片剪輯、SRT/ASS/SSA/VTT 編輯、基本轉場、旁白配樂、存檔復原與無字幕／燒錄字幕輸出。

## 建議閱讀順序

- SPEC.md：完整產品需求。
- docs/TIMELINE.md：軌道、時間精度、重疊、連結音訊、剪切語義。
- docs/MCP.md、docs/CLI.md：Agent 與程式介面契約。
- docs/TRANSACTIONS.md、docs/RENDER_JOBS.md、docs/SECURITY.md：併行編輯、工作與安全。
- ARCHITECTURE.md、PLAN.md：Rust 模組與開發順序。
- contracts/：JSON Schema、工具目錄、19 操作、錯誤碼與 CLI 對照。
- examples/：19 秒與 28 秒的時間軸、批次交易、MCP 訊息及輸出请求範例。
- AGENTS.md、skills/storycut-edit/SKILL.md：開發與剪輯 Agent 的操作規範。
- ACCEPTANCE.md：待實作／待實機測試的驗收項目，不是通過報告。
- VALIDATION.md：本包實際做過的規格資料測試與未測範圍。
- docs/IMPLEMENTATION_STATUS.md：目前實作、真實媒體與桌面驗收、限制。
- docs/LICENSE_INVENTORY.md：Rust 與前端依賴的宣告授權清單。

## 原規格資料檢查

Python 仍只用於規格檢查，**不是** App 的實作語言。開發環境安裝測試依賴後：

```powershell
python -m pip install -r requirements-validation.txt
python tools/validate_spec.py
python -m unittest discover -s tests -v
```

上述 Python 指令只驗證契約與合成測試資料；產品驗收另見 [實作與驗收紀錄](docs/IMPLEMENTATION_STATUS.md)。

## 第一個可用版本的交付門檻

同一個工程必須能由 GUI 手動剪輯、CLI 無介面操作，以及 MCP Agent 操作；三條路徑都要真的匯出相同的多軌合成影片。Windows 正式交付仍需 GUI 安裝包／可攜版、CLI/MCP 執行檔、鎖定依賴、完整原始碼、授權清單及實機報告。

所有 examples/media 路徑都是占位符；沒有附圖片、影片、音訊或字型。MCP response 範例是人工合成契約資料，並非成功連線的紀錄。
