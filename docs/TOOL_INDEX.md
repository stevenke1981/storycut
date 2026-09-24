# MCP 完整工具索引（待實作）

全部名稱與參數以 contracts/mcp-tools.json 為準；此索引不是運行中 server 的能力聲明。

| 工具 | 功能 |
|---|---|
| `storycut_capabilities` | 取得已實作能力與依賴探測；不得以規格清單冒充可用功能。 |
| `storycut_project_create` | 在允許工作區建立空白專案；同 key 重試不得建立副本。 |
| `storycut_project_open` | 開啟或加入同一 daemon 專案；不覆蓋已開啟的最新狀態。 |
| `storycut_project_get` | 讀取最新版本與完整專案快照。 |
| `storycut_project_save` | 將指定最新版本原子寫回已開啟專案路徑；不另增 revision。 |
| `storycut_project_validate` | 驗證模型；check_media=true 另做素材／依賴讀取，不修改專案。 |
| `storycut_media_import` | 探測允許的本機素材並註冊，成功是一次修訂；不自動加入時間軸。 |
| `storycut_media_list` | 列出專案已註冊素材及可用 stream。 |
| `storycut_timeline_get` | 讀取時間範圍內片段、軌道與依賴；end_tick=null 表示到片尾。 |
| `storycut_timeline_apply` | 原子執行操作批次；版本不符或任一操作失敗時全部不提交。 |
| `storycut_track_add` | timeline_apply 單一 track.add 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_track_update` | timeline_apply 單一 track.update 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_clip_add` | timeline_apply 單一 clip.add 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_clip_move` | timeline_apply 單一 clip.move 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_clip_trim` | timeline_apply 單一 clip.trim 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_clip_split` | timeline_apply 單一 clip.split 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_clip_remove` | timeline_apply 單一 clip.remove 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_motion_set` | timeline_apply 單一 motion.set 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_audio_set` | timeline_apply 單一 audio.set 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_transition_set` | timeline_apply 單一 transition.set 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_link_create` | timeline_apply 單一 link.create 操作的型別化捷徑；共用授權、鎖軌、版本與復原。 |
| `storycut_subtitle_import` | 解析字幕並保留原文件，放到既有 subtitle 軌；格式支援以能力查詢為準。 |
| `storycut_subtitle_update` | 保留未知語法更新字幕；無法安全保存時拒絕，不能自動扁平化。 |
| `storycut_subtitle_export` | 以指定格式輸出字幕 sidecar；樣式降級必須明確授權。 |
| `storycut_history_undo` | 只操作目前線性歷史頂端，不可選擇性抹除另一 Agent 修改；修訂號仍遞增。 |
| `storycut_history_redo` | 只操作目前線性歷史頂端，不可選擇性抹除另一 Agent 修改；修訂號仍遞增。 |
| `storycut_preview_frame` | 提交精確單幀預覽工作；回 job_id；透過 job_get 查核產物。 |
| `storycut_preview_range` | 提交指定區間精確預覽；使用同一渲染計畫，不是假播放原素材。 |
| `storycut_render_start` | 依固定專案修訂提交輸出工作；未完成驗證前不可宣稱成功。 |
| `storycut_job_get` | 查詢工作狀態；include_preview=true 可附已完成的有界 PNG content，僅 preview_frame。 |
| `storycut_job_list` | 列出工作；回應上限 100，以 cursor 分頁。 |
| `storycut_job_cancel` | 要求取消此使用者有權管理的工作；cancelling 不等於 cancelled。 |
| `storycut_job_events` | 從事件序號增量讀取；不足保留區間時 reset_required=true 並重新 job_get。 |
