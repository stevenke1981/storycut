# 官方資料（核對日期：2026-09-18）

本規格中的功能與版本分期是設計建議，官方文件僅支持技術能力和格式限制；它們不代表本 App 已經實作。

- [R1] Tauri 2：Rust 邏輯、Web 前端與跨平台架構。https://v2.tauri.app/
- [R2] Tauri 建置前提：Windows 開發與 WebView2。https://v2.tauri.app/start/prerequisites/
- [R3] Tauri 外部程式 sidecar。https://v2.tauri.app/develop/sidecar/
- [R4] FFmpeg filters：zoompan、xfade、acrossfade、subtitles 及各濾鏡要求。https://ffmpeg.org/ffmpeg-filters.html
- [R5] Wails：Go＋Web 技術建立桌面 App。https://wails.io/docs/introduction/
- [R6] libass：ASS／SSA 字幕渲染器。https://github.com/libass/libass
- [R7] Aegisub ASS Override Tags。https://aegisub.org/docs/latest/ass_tags/
- [R8] Library of Congress，SubRip Subtitle format (SRT)。https://www.loc.gov/preservation/digital/formats/fdd/fdd000569.shtml
- [R9] W3C WebVTT 規格：cue、position、line、align、region 等。https://www.w3.org/TR/webvtt1/
- [R10] FFmpeg License and Legal Considerations。https://ffmpeg.org/legal.html
- [R11] NVIDIA Using FFmpeg with NVIDIA GPU Hardware Acceleration。https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/ffmpeg-with-nvidia-gpu/index.html

發佈時需再次核對依賴版本與授權。不要因為框架可跨平台，就把未測試的平台寫成已驗收。


## v0.2.0 本次核對的官方資料

下列資料用於協議與技術依據，不代表本 App 已實作。查閱日期：2026-09-18。以上 R1–R11 沿用原包；本次特別重新核對 R3 sidecar 與 R4 filters，不將所有舊連結冒稱重新測試。

- [M1] MCP 2025-11-25 Transports：stdio／stdout 規則，與該版本 Streamable HTTP 安全。https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
- [M2] MCP 2025-11-25 Lifecycle：初始化、能力與生命週期。https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle
- [M3] 官方 Rust SDK rmcp；實作時需鎖定實際版本和通過協議測試。https://github.com/modelcontextprotocol/rust-sdk
- [M4] MCP 2025-11-25 Tools：inputSchema/outputSchema、structuredContent、錯誤與 annotations。https://modelcontextprotocol.io/specification/2025-11-25/server/tools
- [M5] MCP Cancellation：request 層取消。https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation
- [M6] MCP Progress：進行中的 request token 與進度。https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/progress

本案選 2025-11-25 作基本相容性基線，**不宣稱它是最新規範**。產品功能、33 工具與 tick 常數是 StoryCut 設計決策，不應歸因給 MCP/FFmpeg 官方。

## 2026-10-01 Rust × FFmpeg 專案調查（GitHub）

只作參考，未引入依賴或程式碼：

- [G1] nathanbabcock/ffmpeg-sidecar（MIT）：包裝 FFmpeg 可執行檔的 Rust 介面；其自動下載功能不符本案「不自動下載」原則，故只自建 PATH 探測。https://github.com/nathanbabcock/ffmpeg-sidecar
- [G2] larksuite/rsmpeg（MIT）、zmwangx/rust-ffmpeg（WTFPL）：連結 libav*；Windows 需 FFmpeg dev 函式庫，與現行 CLI 路線與可攜發行不合。
- [G3] jub0t/Concat（AGPL-3.0）、66HEX/frame（GPL-3.0）：授權與 MIT 不相容，不複製程式碼。

