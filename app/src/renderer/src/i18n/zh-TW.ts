import type { Messages } from './en'

export const zhTW: Messages = {
  toolbar: {
    rectangle: '矩形',
    ellipse: '橢圓',
    emojiSticker: '表情貼圖',
    arrow: '箭頭',
    pen: '畫筆',
    highlight: '螢光筆',
    mosaic: '馬賽克',
    text: '文字',
    color: '顏色',
    translate: '翻譯',
    ocr: '提取文字 (OCR)',
    search: '搜尋',
    scrollCapture: '捲動截圖',
    undo: '復原',
    save: '儲存',
    pin: '貼圖',
    cancel: '取消',
    done: '完成（複製）'
  },
  hints: {
    dragToSelect: '拖曳選取區域 · Esc 取消',
    loading: '正在載入截圖…',
    capturing: '正在擷取螢幕…',
    working: '處理中…',
    adjustRegion: '拖曳控制點可調整大小 · 拖曳內部可移動 · 選擇工具開始標註'
  },
  scrollCapture: {
    capture: '擷取下一幀',
    cancel: '取消',
    done: '完成',
    capturing: '擷取中…',
    failed: '捲動截圖失敗',
    preview: '即時預覽',
    previewEmpty: '捲動頁面以產生長圖…',
    scrollPreviewHint: '在此區域捲動可預覽完整長圖'
  },
  ocr: {
    title: '提取的文字',
    copy: '複製文字',
    close: '關閉',
    noTextFound: '未找到文字。',
    noTextDetected: '（未偵測到文字）'
  },
  errors: {
    decodeFailed: '截圖解碼失敗',
    captureFailed: '螢幕擷取失敗',
    loadFailed: '截圖載入失敗'
  },
  settings: {
    title: '設定',
    language: '語言',
    shortcut: '截圖快捷鍵',
    shortcutHint: '點選下方輸入框，按下想要的組合鍵，然後儲存。',
    changeShortcut: '變更快捷鍵',
    pressShortcut: '請按下快捷鍵…',
    save: '儲存',
    cancel: '取消',
    saved: '快捷鍵已儲存。',
    shortcutInvalid: '快捷鍵無效。請至少使用一個修飾鍵（⌥、⌘、Ctrl、⇧）加一個按鍵。',
    shortcutInUse: '該快捷鍵已被其他 App 占用。',
    shortcutPressNow: '請按下新的組合鍵，然後點選儲存。',
    saving: '儲存中…'
  },
  textEditor: {
    drag: '拖曳移動',
    placeholder: '輸入文字…',
    done: '完成',
    cancel: '取消',
    moveHint: '拖曳文字可移動 · 連按兩下可再編輯',
    emojiMoveHint: '拖曳可移動 · 拖曳右下角可調整大小 · Backspace 可刪除'
  },
  clipboardHistory: {
    title: '剪貼簿記錄',
    searchPlaceholder: '搜尋剪貼簿',
    emptyTitle: '暫無剪貼簿記錄',
    emptyDescription: '複製文字、圖片或檔案後會顯示在這裡。',
    noResults: '找不到符合的內容',
    unavailable: '剪貼簿記錄暫時無法使用',
    operationError: '無法載入剪貼簿記錄',
    pasteFailed: '恢復剪貼簿內容失敗',
    pasteCopiedOnly: '已複製，請手動按 {pasteShortcut}',
    pasteAccessibilityRequired: '已複製。開啟輔助功能權限後可自動貼上。',
    imageUnavailable: '圖片無法使用',
    preview: '預覽',
    favorite: '加入收藏',
    unfavorite: '取消收藏',
    delete: '刪除',
    text: '文字',
    html: 'HTML',
    image: '截圖',
    files: '檔案',
    sourceApplication: '來源應用程式',
    themePickerLabel: '剪貼簿記錄主題',
    creamHanddrawn: '奶油手繪',
    bunnyCloud: '兔兔雲朵',
    link: '連結',
    screenshot: '截圖'
  }
}
