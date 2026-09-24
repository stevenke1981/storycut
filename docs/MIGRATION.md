# 從 v0.1.0 草案移轉

本包保留 examples/legacy-v0.1.0-two-images.json 作原始 schema 參考，不把它當 v0.2 可用資料。examples/two-images.storycut.json 為手動建立、經資料驗證的 v0.2 對照；**沒有提供生產級 migration 程式**。

| 舊欄位 | 新欄位／行為 |
|---|---|
| video_track | tracks 中的一條 kind=video；片段移至 clips 並加 track_id |
| start_frame / duration_frames | frame × timebase × fps.den / fps.num → start_tick / duration_ticks |
| keyframe.frame | 同樣精確換 tick；增加 motion domain_duration、sample_offset=0 |
| audio_tracks | 轉 tracks(kind=audio) 及 clips(kind=audio)，來源 stream 明確化 |
| subtitle_document | subtitles 文件陣列，指向 subtitle track；保留 raw 文件及 cue |
| transitions from_clip/to_clip | from_clip_id/to_clip_id，加 track_id 與 global tick |
| export | 改成使用者可選的輸出 job request，不在開啟工程時自動執行 |

30 fps 的旧 300 幀 = 7056000000 ticks；270 幀 = 6350400000 ticks；兩段最大結束 = 13406400000 ticks = 570 幀 = 19 秒。

已知來源 PTS 以原 time_base 保存；非整數 tick 轉換若必須量化，回報精度損失並保留原表示，不宣稱位元無損。原始專案只讀備份，schema_version 不識別時拒绝寫回；新的資料版本必須經明確遷移。
