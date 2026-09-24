# 規格測試環境

以下是 2026-09-18 原規格包的歷史環境說明。2026-09-24 Windows 產品實作與執行驗收請看 [實作與驗收紀錄](../docs/IMPLEMENTATION_STATUS.md)。規格測試通過不能代表產品操作通過。

Python 3.13.5；jsonschema 4.26.0；Linux 容器。
本環境非使用者 Windows，未執行 Rust、Tauri、MCP 或 FFmpeg 渲染。
58 項 unittest 通過；結果見 contract-test-output.txt。
