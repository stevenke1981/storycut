# StoryCut Windows 可攜版

解壓縮後執行 `StoryCut.exe`，或以 PowerShell 執行 `Start-StoryCut.ps1`。編輯器與 Agent CLI/MCP 均已編譯；不需要 Rust 或 Node.js。請把工程與媒體放在您自行建立、授權的工作區。程式不會自動搬動或刪除原始媒體。

預覽、素材探測和輸出需要 `ffmpeg.exe` 與 `ffprobe.exe` 位於 `PATH`，燒錄字幕需要 FFmpeg 含 libass。本包沒有內嵌 FFmpeg；安裝或設定後，先以 `ffmpeg -version`、`ffprobe -version` 確認。原生視窗需要 Windows WebView2 執行環境。

Agent 可在同一工作區使用 `storycut-cli.exe`：

```powershell
.\storycut-cli.exe --json --workspace C:\StoryCutWorkspace capabilities
.\storycut-cli.exe mcp --stdio --workspace C:\StoryCutWorkspace
```

介面詳見 `docs/CLI.md`、`docs/MCP.md`。目前功能與真實驗收範圍詳見 `docs/IMPLEMENTATION_STATUS.md`。CLI/MCP 與桌面版須指向相同的工作區才能共用工程。
