# 工作、預览與輸出生命周期

render_start、preview_frame、preview_range 都提交應用層工作，回傳完整 job object，內含 job_id/source_revision。範例 response 是人工資料，不是工作已執行。

狀態：queued → running → succeeded/failed；queued/running → cancelling → cancelled；程序中斷可為 interrupted。若取消和完成競賽，依 daemon 原子終態決定；已 succeeded 不可再改 cancelled，已 cancelled 不再發布成片。

progress 可為 null，不虛構百分比；以計畫總時間及已編碼進度估算時需限制於 0..1。只有成片驗證與正式發布完成才是 succeeded。工作事件 seq 單調遞增，job_events 帶 after_seq 增量讀；保留窗過期回 reset_required。

## 生命週期

daemon 擁有 FFmpeg 子程序群，不隨啟動它的 CLI 返回或 MCP adapter EOF 消失。使用 Windows Job Objects/程序句柄及 Unix process group 等實際機制安全管理，不用 killall ffmpeg。

MCP request cancellation 只能取消尚在等待的工具呼叫；工具已回 job_id 後，必須用 job_cancel。新 cancel 請求成功只代表「已接受」，需 job_get 確認 cancelled。不得把 MCP adapter 連線關閉當作殺掉所有任務。[M5]

## 固定快照與輸出安全

snapshot 固定 revision／素材指紋／字幕原文件／字型環境／依賴；開始與結束檢查來源變動。變動回 MEDIA_CHANGED，避免中途素材被替換產生混合版本影片。

輸出前預檢 path allowlist、磁碟空間、重複工作與覆蓋政策。overwrite=false 以不覆蓋的原子發布方式保障競賽，不是只在開始時 exists 檢查一次。

在目標磁碟安全暫存檔寫入→驗證→平台適合的原子發布。覆蓋需明確授權；Windows rename/replace 差異必須實測，不能先刪舊檔再搬新檔。取消只清理本 job 暫存，不刪來源或先前產物。

## 媒體驗證

至少驗證視訊幀數、fps、有無需要的 audio stream、音訊裁切／延遲、duration 與主要 timestamp、影像實際可解碼、字幕模式正確。單看 FFmpeg exit=0 或容器 duration 不足。音訊壓縮 padding 必須與解碼後有效樣本／PTS 分開核對。

clean 輸出 = 不燒錄字幕，仍含明確配置的圖層與混音；burn = 另加字幕。VTT 燒錄不支援的效果需先報告，不能拿純文字替代所有定位而宣稱相同。

## 預覽產物

preview_frame 產出已合成的 PNG，不是來源縮圖；精確渲染字幕／疊圖／當下轉場。preview_range 產出包含混音的短片，使 Agent 或人類能檢查聲音。預覽和成片共用引擎及 snap-to-frame 契約。

job_get(include_preview=true) 只對已成功單幀預覽回有界 PNG content；其他結果回 artifact ID／相對路徑／sha256／尺寸。讀取 artifact 也要檢查工作所有權及工作區，不提供任意讀檔接口。

## 實作狀態

FFmpeg 真實 PNG 預覽、含混音的短 MP4 預覽、MP4 輸出及部分媒體 QA 已實作，並以 Windows 真素材驗證。Windows daemon/client IPC 骨架已有單一工作區 lease、具目前使用者 SID 限制的 named pipe 和有界佇列，但 GUI/CLI/MCP 尚未接入，工作仍同步執行；工作事件、背景渲染與中途取消仍待實作。詳見 [實作與驗收紀錄](IMPLEMENTATION_STATUS.md)。規格驗證器只能檢查資料關係，不足以證明工作生命周期已通過。
