# 夜燈說書：圖片故事工作流程

本工作流程用於依序排列數十張圖片／影片，搭配旁白、角色對話和配樂。GUI、CLI、MCP 都呼叫同一個 Rust 核心；工程保存的是原素材引用與剪輯參數，不會改寫來源檔案。

## 素材準備

把專案和素材放在同一個授權工作區內。建議將畫面命名為 `001-開場.png`、`002-山路.png`；自然排序會讓 `2` 排在 `10` 前，也可手動調整順序。音訊依旁白、角色和配樂分別命名。不要將唯一的原始素材搬入測試目錄。

## 故事組裝

1. 開啟或建立工程，匯入圖片、影片與音訊。
2. 在故事組裝面板選擇畫面素材並確認順序。圖片預設 **10 秒**，可逐張修改；影片預設採用所選來源起點之後的可用長度。
3. 選擇直切或交叉淡化。淡化會讓相鄰片段重疊，因此總片長會縮短；面板的核心試算結果才是實際長度。
4. 分別加入旁白、角色對話及配樂，設定各段的時間軸起點、音量和淡入淡出。角色可使用自己的軌道名稱。
5. 短配樂可選擇循環至畫面結束；首段淡入、末段淡出。旁白和對話不會自動循環，也不會自動對齊語意或壓低配樂。
6. 先試算，再提交整批組裝。核心會檢查版本、素材長度、軌道鎖定和交易上限，失敗時不會留下半套時間軸。

每次組裝是一次可復原的交易。再次組裝會建立／加入片段，不會把既有故事當作可丟棄內容覆蓋。若只想調整現有片段，請使用時間軸和屬性面板。

## 圖片移動與對焦

選取圖片片段，在來源縮圖上點選人物或主體；用 Tab 聚焦縮圖後，可用方向鍵微調，按住 Shift 可加大步幅。面板會顯示 X/Y 座標，範圍為 0–1，左上角是 `(0,0)`，中央是 `(0.5,0.5)`；Agent 可直接傳入精確座標。

- **焦點推近**：從填滿畫布的取景開始，逐漸放大並將所選主體移向畫面中央；原圖與畫布比例不同時，起始取景就可能裁去部分邊緣。
- **平移**：上下左右移動畫面內容；先放大保留移動空間。
- **推近／拉遠**：以中央為目標的縮放。
- **靜止**：維持固定取景。

核心依素材與畫布比例採 cover 取景，避免露出空白邊緣。靠近原圖邊緣的主體不一定能在不露邊的情況下完全置中；核心回傳 `FOCUS_CLAMPED` 時，表示取景已受邊界限制。這是手動選定焦點，沒有自動辨識人物功能。

可將同一運鏡批次套用到圖片；不同構圖的圖片仍建議逐張確認主體位置。重新套用預設會用目前片段的可見長度重建曲線；單純分割既有運鏡則沿用原曲線，保留分割點的連續性。

來源縮圖只用於選焦點。請用「精確影格」或短片預覽檢查實際構圖、動作、混音和字幕；最後再匯出完整 MP4。

## Agent 操作

先讀 `storycut_capabilities` 和 `storycut_project_get`。新增兩個高階工具：

| 工具 | 用途 |
|---|---|
| `storycut_storyboard_assemble` | 按明確順序批次加入圖片／影片、連結影片原聲，以及旁白／對話／循環配樂 |
| `storycut_focal_motion_apply` | 按片段 ID 與來源焦點座標批次設定運鏡 |

兩者都需要 `project_id`、`expected_revision`、`idempotency_key`、`dry_run`。先用 `dry_run: true` 檢查，再以相同內容、修訂版與冪等鍵改為 `false` 提交。冪等鍵只可對應一份邏輯請求；重試不會重複新增片段。工程被其他 Agent 修改時，先回讀新版本，再用新的冪等鍵重新規劃；不把舊試算直接換上新修訂版提交。桌面在閒置時同步外部修訂，也可按「同步工程」。

完整欄位以 `contracts/mcp-tools.json` 為準。CLI 以 UTF-8 JSON 檔案傳遞，避免命令列轉義或長度問題；可攜版的 CLI 名稱為 `storycut-cli.exe`，請將下列 `storycut.exe` 換成該名稱：

```powershell
.\storycut.exe --json --workspace D:\MyStory call storycut_storyboard_assemble --args-file assemble.json
.\storycut.exe --json --workspace D:\MyStory call storycut_focal_motion_apply --args-file focus.json
.\storycut.exe mcp --stdio --workspace D:\MyStory
```

原始碼中的 `examples/storyboard-assemble.dry-run.json` 和 `examples/focal-motion-apply.dry-run.json` 對應 `after-apply.storycut.json` 的示範 ID 與 revision 8。使用時須換成目前工程回傳的 ID／revision；範例素材不是隨附的真實媒體，也不是可直接套用到任意工程的腳本。

## 驗收與限制

`tests/product_acceptance/story_workflow.py` 會建立隔離的 72 張圖片、2 秒影片和三種聲音，經真 CLI/MCP 組裝，再量測輸出影格數、焦點位置、放大比例及音訊起點。實際執行紀錄與桌面測試見 [IMPLEMENTATION_STATUS.md](IMPLEMENTATION_STATUS.md)。合成素材驗收不是使用者的正式 YouTube 成片。

目前渲染仍是同步工作，背景取消與 GPU 編碼尚未完成。長故事先產生短片預覽檢查，再輸出全片；大量 4K 圖片的資源需求不等同低解析度合成驗收。
