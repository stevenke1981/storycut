# Agent 操作流程

目前 CLI/MCP 可操作真實工程與媒體。請以 `storycut_capabilities` / `tools/list` 決定可呼叫工具；以下含尚未實作的目標流程，失敗時以實際錯誤碼為準。渲染目前同步完成後才回傳，必須確認 `data.job.state == "succeeded"` 並檢查產物；`job_events`、`job_cancel` 尚未可用。

1. 使用 host 的 stdio MCP 設定啟動 `storycut mcp --stdio --workspace <已授權目錄>`。完成協商後呼叫 storycut_capabilities；只用真正宣告可用的工具。
2. project_create/open，取得真正 project_id；media_import 得到 asset ID、stream 及新 revision。不使用顯示名稱取代 ID。
3. project_get/timeline_get 讀最新工程；依影像／圖片／聲音建立軌道與 clip。影片原聲同批建立 audio clip 和 av_sync link。
4. timeline_apply(dry_run=true) 查看最終長度與 diff；再以未改變的 expected_revision 和相同語義 key 提交 dry_run=false。有衝突就重讀，不盲目換 key。
5. subtitle_import 進既有 subtitle 軌，保留原格式；需要移動字幕時明確指定 offset 或修改 cue。故事片頭只依使用者選定模板新增。
6. project_validate(check_media=true)；preview_frame/range；用 job_get 確認成功並看／聽實際產物。機器 QA 和人工檢查互補，不能只看資料 schema。
7. 固定 revision 分別 render_start(clean) 與 render_start(burn)。job_get 回 succeeded 且具有效產物後才能宣告完成；另執行 project_save。

## 範例對照

examples/seed-after-import.storycut.json 假設已匯入 7 個素材且 revision=7。timeline-apply 的 batch 新增 8 軌、9 段片段、2 轉場與 1 組 AV link，原子提交後 revision=8；subtitle_import 再變為 9。render-clean/subtitles 使用 revision=9。

這些 ID/版本是示例；真的每次媒體匯入若採 batch，修訂數不一定是 7。以 API 回傳的實際值為準。metadata 未探測、media 檔案不存在的本包 fixture 不能直接拿去聲稱 renderable。

## 失敗處理

REVISION_CONFLICT → 重新讀取並確認新意圖；LOCKED_TRACK → 回報，不擅自解鎖；MEDIA_OFFLINE → 請求重新連結／匯入有權素材；UNSUPPORTED_FEATURE → 回報不支援，不偷偷刪特效；RENDER_FAILED → 保留原工程，查看 job 的安全錯誤，不能回成功。

若輸出等待逾時，使用同一 idempotency_key 查詢或重試，不要立刻換 key 送新渲染。`job_cancel` 尚未實作；目前需要中止長渲染時須停止呼叫程序，並以 `job_get` 查詢其持久化狀態。所有外部副作用都依使用者指定範圍。
