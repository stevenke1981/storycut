---
name: storycut-edit
description: Use the installed StoryCut MCP or CLI to edit video, image, audio and subtitle timelines through revision-safe transactions. This specification package itself does not install a server.
---

# StoryCut Agent 剪輯

先確認實際 StoryCut 工具存在與 capabilities。缺少實作時不得假裝呼叫，也不要把規格 JSON 當成成功結果。

使用 MCP 或 CLI 公共契約：讀取 project/revision → 素材探測 → 規劃完整 typed operations → dry-run → 檢查 diff → 原子提交 → 精確預覽 → 固定 revision 輸出 → 核對終態與產物。

GUI/Agent 共用同一工程。聲音的 start 不得以圖片數量猜，來源入點与 timeline 起點分開；原聲用連結 audio clip，不能雙重混入。图片 duration 和 motion domain 分開，分割保留 sample_offset 以保持動作。

遇版本衝突先重讀；輸出逾時先查 job；鎖軌不擅自解鎖；複雜 ASS/VTT 不默默扁平化；新输出預設不覆蓋既有檔。調用工具不繞過 host 或工作區授權。

使用 artifacts 的實測訊息判定成功。schema pass、planned diff、queued job 或 exit=0 都不能單獨代表影片完成。

## 夜燈說書類旁白故事

旁白逐段合成的故事影片，用 `storycut_narration_assemble` 一次排好：旁白段落依序接續、每個畫面宣告涵蓋的段落範圍、原生生成影片自動加凍格把手、`centered_dissolve` 不縮短片長、配樂預設被旁白側鏈壓低、`overlays` 放 alpha 串場字卡。先 dry-run 看 `cuts`、`native_cores`、`segment_offsets`，再以相同請求提交；字幕依 `segment_offsets` 的 `start_tick` 當 offset 匯入（ASS 可燒錄且保留樣式）；輸出時用 `master_loudness: {"integrated_lufs": -16}`。規格與限制見 docs/NIGHT_LANTERN_PROFILE.md。
