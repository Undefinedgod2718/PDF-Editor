import { useMemo, useState } from 'react'
import { printLocal, type AppMode, type DocInfo } from '../api'
import { parsePageSpec } from '../lib/pageSpec'

interface Props {
  doc: DocInfo
  mode: AppMode
  onClose: () => void
}

/** 列印對話框。桌面版打 POST /api/local/print（見 desktop/src/print.rs），跳系統列印
 *  對話框讓使用者選印表機／份數；網頁版瀏覽器碰不到印表機 device context，只能走
 *  window.print()——頁碼範圍與「列印註解」選項對網頁版沒有意義，所以在 web 模式下
 *  直接禁用這兩個控制項並說明原因，而不是留著讓使用者以為設定會生效。 */
/** 單張圖最多等這麼久。列印寧可少一頁，也不該整個按鈕卡在「列印中…」。 */
const IMAGE_WAIT_MS = 5000

/**
 * 捲軸外的頁面圖是 `loading="lazy"`（見 Viewer.tsx），還沒進視窗的那些根本沒下載
 * ——實測開啟 5 頁文件時 `complete` 是 0/5。規範說列印時不該套用 lazy loading、
 * Chromium 也有實作，但賭錯的下場是印出一疊空白頁，所以自己先拉下來。
 *
 * **不要用 `img.decode()` 當就緒訊號**：在不合成畫面的環境（無頭瀏覽器、背景分頁）
 * 它可能永遠不 resolve，即使圖片早就 `complete` 且有 naturalWidth。實測過：五張圖
 * 全部載入完成，五個 decode() 全部逾時，按鈕就此卡死。改用 load/error 事件，
 * 並且一律加逾時上限。
 */
async function forceLoadPageImages(): Promise<void> {
  const imgs = [...document.querySelectorAll<HTMLImageElement>('.page img')]
  await Promise.all(
    imgs.map((img) => {
      img.loading = 'eager'
      if (img.complete && img.naturalWidth > 0) return Promise.resolve()
      return new Promise<void>((resolve) => {
        const done = () => resolve()
        img.addEventListener('load', done, { once: true })
        // 載入失敗也要放行：那一頁會是空的，但其餘頁面沒理由跟著不印。
        img.addEventListener('error', done, { once: true })
        setTimeout(done, IMAGE_WAIT_MS)
      })
    }),
  )
}

export default function PrintDialog({ doc, mode, onClose }: Props) {
  const isWeb = mode === 'web'
  const [spec, setSpec] = useState('')
  const [annotations, setAnnotations] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // 三種結果各自不同意義（送出 n 頁／使用者取消／失敗），故意跟 error 分開存，
  // 「取消」不能套用跟失敗一樣的警示樣式。
  const [status, setStatus] = useState<string | null>(null)

  // 空字串＝全部頁面（省略 pages 欄位）；非空則交給 parsePageSpec 解析並驗證範圍，
  // 跟 ExportDialog 同一套規則，不另外寫一份解析邏輯。
  const { pages, specError } = useMemo(() => {
    const trimmed = spec.trim()
    if (trimmed === '') {
      return { pages: undefined as number[] | undefined, specError: null as string | null }
    }
    try {
      return { pages: parsePageSpec(trimmed, doc.pageCount), specError: null as string | null }
    } catch (err) {
      return { pages: undefined as number[] | undefined, specError: err instanceof Error ? err.message : String(err) }
    }
  }, [spec, doc.pageCount])

  const valid = isWeb || specError === null

  const submit = async () => {
    if (!valid || busy) return
    setBusy(true)
    setError(null)
    setStatus(null)
    try {
      if (isWeb) {
        await forceLoadPageImages()
        window.print()
        setStatus('已呼叫瀏覽器的列印功能，請在跳出的列印視窗中繼續。')
      } else {
        const { printed } = await printLocal(doc.id, { pages, annotations })
        setStatus(printed === 0 ? '已取消列印。' : `已送出 ${printed} 頁到印表機。`)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div
      className="modal-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div className="modal">
        <div className="modal-header">
          <span>列印</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}
          {status && <div className="print-status">{status}</div>}

          {isWeb && (
            <div className="annot-hint">
              網頁版透過瀏覽器列印，頁碼範圍與「列印註解」由瀏覽器的列印對話框決定，下面兩項設定不會套用。
            </div>
          )}

          <div className="modal-subtitle">
            頁碼範圍（1-based，如 1,3,5-9），共 {doc.pageCount} 頁；留空＝全部頁面
          </div>
          <input
            className="modal-input"
            placeholder="全部"
            value={spec}
            disabled={isWeb}
            onChange={(e) => setSpec(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submit()
            }}
          />
          {!isWeb && specError && <div className="annot-hint">{specError}</div>}

          <label className="doc-list-item">
            <input
              type="checkbox"
              checked={annotations}
              disabled={isWeb}
              onChange={(e) => setAnnotations(e.target.checked)}
            />
            <span>列印註解</span>
          </label>

          <div className="modal-footer">
            <button className="tb-btn btn-primary" disabled={busy || !valid} onClick={() => void submit()}>
              {busy ? '列印中…' : '列印'}
            </button>
            <button className="tb-btn" onClick={onClose}>
              關閉
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
