# 多 Agent／GUI 一致性與交易

## 同一份真實狀態

每個 project 有 immutable project_id 和單調遞增 revision。GUI、CLI、各 MCP adapter 的 Agent ID 僅作可核對的記錄，不是授權憑證。daemon 依 OS 使用者／工作區政策識別來源。

持有專案的唯一 writer 在 daemon。每個客戶端讀 revision，再以 expected_revision 提交。非同版拒絕 REVISION_CONFLICT，回應目前修訂與可理解原因；不能 --force 覆蓋，不能把舊整份 JSON 存回最新工程。

## 交易 envelope

```json
{
  "project_id": "demo-multitrack",
  "expected_revision": 7,
  "idempotency_key": "demo-build-timeline-001",
  "dry_run": true,
  "operations": []
}
```

上面只是欄位示意；真實 operations 至少一項，完整有效範例在 examples/。

每批最多 500 操作。先檢查 caller、工作區、schema、物件 ID、權限與鎖定，再在副本上按序執行，最後驗證跨軌關係、來源 bounds、重疊、links 與時間精度。允許 batch 中途暫時存在尚未補上的 link/transition；只有最終合法狀態可提交。

任何失敗：全部不改；不得只成功前十個片段。成功：revision+1，一筆 undo 歷史，durable journal 與冪等結果一併原子寫入，再回應 changed_ids、removed_ids、duration 與 diff。

## 預檢與冪等

dry_run 不提交、不改 revision、不預留 idempotency key，也不預先鎖定工程。它回傳 planned diff／時長／錯誤；真提交仍需以相同 revision 重新驗證，不能把過期預檢當通行證。外部素材 probe 可產生有界非專案快取，但不得改專案或建立輸出檔。

提交前在同一安全作用域查 key：相同 canonical payload 的已成功結果，直接回原結果與原修訂（即使工程後來又變動），不再執行；不同 payload 用同 key 回 IDEMPOTENCY_CONFLICT。dry_run flag 不列入語義 payload。revision 檢查在這個「已成功重試」判定之後。

冪等記錄與 journal 一起持久化，不可只存程序記憶體。以新 key 提交代表新意圖，不能自動 deduplicate 合法的第二份相同圖片。

## 特殊操作

- track.update：只能更新列出的欄位，不能改 kind/id。非空 track.remove 必須拒絕；先明確移動／刪除內容。
- track.reorder：須提供完整、無重複的 track ID permutation。
- link.remove：解除同步且將視訊 audio_policy 改 muted，音訊保留。重連必須先對齊。
- transition.remove(reject_if_overlap)：必須同批另行修正重疊，否則整批拒絕。
- transition.remove(butt_cut_ripple)：將 incoming 與明列同軌後續片段右移原 overlap 長度以接成硬切；required linked clips 同步。若未列 target track 或碰鎖／其他片段則拒絕。名字保留 ripple 指後方位移，非刪除時的一律左移。
- clip.trim resample_visible：source 的頭部變化同步加到 effects sample_offset；若圖片 source_in 固定 0，頭部可見裁切依 start delta 計算。retime_full 明確重置 effects domain。
- clone clip：第一版可用新的 clip.add ID 和相同 asset reference，需自行包含合法 effects 與 links；不複製實際媒體。

字幕 cue 的 insert 必須使用新 ID，update 必須指向既有 ID，remove 也須明確 cue_id。分割／合併字幕以同批 remove＋insert/update 完成；保留不受影響的 raw 語法。操作會損壞未知 ASS/VTT 內容時拒絕，不能因 allow_lossy=false 而假裝完全保真。

所有去除片段只改工程，沒有刪除來源檔操作。任何刪除牽連 transition 需明確同批处理，不擅自移除陌生關聯。

## 鎖與 undo/redo

檢查 transaction 開始時的鎖定狀態；不能在同批暗中 unlock 後修改以繞過鎖。獨立解鎖交易需 allow_unlock 政策並有使用者明确授權。使用者 GUI 正常解鎖也會送到同一核心。

Undo/redo 都需 expected_revision 和冪等 key，只處理線性歷史頂端；復原後是新的 revision，不把數字倒退，避免 ABA 衝突。成功新編輯會清除 redo。再次 undo 另一 Agent 的修改，需 UI/Agent 先展示受影響交易，不提供任意選擇性覆蓋。

read/render 固定 revision；preview 與匯出期間可繼續編輯，但舊快照結果標示其 revision，不可覆蓋 GUI 正在顯示的新結果。
