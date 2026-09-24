# v0.2.0 規格包驗證紀錄

日期：2026-09-18。**本紀錄只驗證本包的契約與合成資料，不是 App 驗收報告。**

## 實際執行

- Python 規格驗證器完成；33 個 MCP 工具的 inputSchema/outputSchema 與專案／19 種操作 schema 通過結構檢查。
- 4 份 v0.2 專案 fixture 通過選定的跨欄位檢查：軌道種類、來源引用、時間對齊、重疊／轉場、AV link、字幕時段、動畫與聲音包絡時間域。
- 5 份工具輸入範例、2 份工具輸出範例通過對應 schema；structuredContent 與 text JSON 相同，isError 與結果一致。
- 19 秒示例為 570 幀；28 秒、8 軌示例為 840 幀；批次組装結果與預期狀態完全相同。這些是資料計算，不是 FFmpeg 輸出量測。
- `python -m unittest discover -s tests -v`：**58 項通過**。包含負時間／零長度／不對齊／缺素材引用／軌型不符／來源越界／重疊未宣告／AV 不同步／錯誤 envelope 等負向案例。
- 以有理數驗證分割圖片保留原曲線採樣；以精確絕對樣本邊界驗證 NTSC 幀邊界音訊取整不逐段累積。這是數學／資料模型測試，不是 Rust 剪輯核心測試。
- 原有 SRT、ASS、VTT 樣本與 v0.1 legacy JSON 保留；本次只檢查字幕標頭與事件數，未完整驗證所有字幕語法或排版。
- ZIP 檔案 CRC、JSON 可解析性與包內 SHA256 manifest 檢查通過。

## 未執行，不能宣稱完成

Rust/Tauri 編譯、GUI 時間軸操作、真實 MCP client/server 協商、CLI 執行、daemon/IPC、多程序交易、OS 路徑沙盒、冪等持久化、FFmpeg 探測／渲染、字幕排版、音畫同步實測、工作取消／恢復、Windows 安裝、GPU、長片效能及產品安全測試。

`ACCEPTANCE.md` 的 A01–A60 全部是產品實作後要跑的驗收條件，不因本次 58 項規格資料測試就被視為通過。

本包沒有 App 原始碼、可執行 MCP/CLI、Windows 安裝器、媒體或字型。範例媒體 metadata 是未探測的宣告值，工具 response 是人工合成資料。

## 重現

```powershell
python -m pip install -r requirements-validation.txt
python tools/validate_spec.py
python -m unittest discover -s tests -v
```

實際輸出位於 tests/spec-validation-result.json 與 tests/contract-test-output.txt。
