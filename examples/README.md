# 範例不是已執行結果

素材全部缺席、probe_status=unprobed，source duration/streams 僅為合成宣告值。不可據此宣告渲染或 ffprobe 成功。

- two-images.storycut.json：v0.2 模型、兩圖 570 幀／19 秒。
- multitrack.storycut.json：独立 28 秒、8 軌、9 段、2 轉場、1 AV link 與 VTT 文件的模型案例；revision=0 不代表已依 Agent 流程提交。
- seed-after-import.storycut.json：另一個流程 fixture，假設素材已註冊於 revision=7。
- operations.json、timeline-apply.*：原子建立時間軸；dry-run 不提交，commit 7→8。
- after-apply.storycut.json：上述 batch 預期狀態，不是 App 真正執行結果。
- subtitle-import.json：8→9；render-clean/subtitles.json 針對 9 輸出。
- mcp-client-messages.jsonl：僅 client 訊息，必須按握手回應循序送出，不可盲目一次 pipe。
- *.response.json：合成工具回應，供 schema 驗證，不是真實 server log。
- mcp-launcher.json：通用啟動參數；各 host 的設定外殼需另外適配。
- legacy-v0.1.0-two-images.json：僅用於格式對照。

此目錄没有媒體、字型或軟體執行檔；captions.srt/ass/vtt 沿用原版樣本，不保證樣式相互無損。
