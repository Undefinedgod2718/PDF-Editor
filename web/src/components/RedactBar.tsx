import { useState } from 'react'
import { redactDocument, type DocInfo, type RedactBox, type RedactStats } from '../api'

interface Props {
  doc: DocInfo
  /** 目前所有頁面暫存的密文方框（跨頁累積）。 */
  boxes: RedactBox[]
  /** 清除全部暫存方框（不送出）。 */
  onClear: () => void
  /** 套用成功：新文件已建立、原始文件已在後端刪除，帶新文件 id 開啟並關閉密文模式。 */
  onApplied: (newDocId: string) => void | Promise<void>
  /** 關閉密文模式（取消、Esc 皆會呼叫）——暫存方框一併捨棄。 */
  onClose: () => void
}

export default function RedactBar({ doc, boxes, onClear, onApplied, onClose }: Props) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [stats, setStats] = useState<RedactStats | null>(null)

  const pageCount = new Set(boxes.map((b) => b.page)).size

  const apply = async () => {
    setBusy(true)
    setError(null)
    try {
      const res = await redactDocument(doc.id, boxes)
      setStats(res.stats)
      await onApplied(res.document.id)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="redact-bar">
      <div className="redact-bar-header">
        <span>區域密文（光柵化）</span>
        <button className="tb-btn" onClick={onClose}>
          ✕
        </button>
      </div>

      {error && <div className="annot-hint">{error}</div>}

      {stats ? (
        <div className="redact-bar-status">
          已套用：{stats.pages_rasterized} 頁光柵化、燒入 {stats.boxes_burned} 個方框
          {stats.struct_elements_removed > 0 && `、清除 ${stats.struct_elements_removed} 個無障礙標記`}
        </div>
      ) : (
        <>
          <div className="annot-hint">
            拖曳畫出要塗黑的區域，可跨頁累積多個，畫完按「套用」。套用後該頁會整頁轉為圖片（不可復原、失去文字層），且
            <strong>原始（未密文）文件會被刪除</strong>，只保留密文後的新文件。
          </div>
          <div className="redact-bar-status">
            {boxes.length === 0
              ? '尚未選取任何區域'
              : `已選取 ${boxes.length} 個方框，共 ${pageCount} 頁`}
          </div>
        </>
      )}

      <div className="redact-bar-actions">
        {!stats && (
          <>
            <button
              className="tb-btn btn-primary"
              disabled={busy || boxes.length === 0}
              onClick={() => void apply()}
            >
              {busy ? '套用中…' : `套用密文 (${boxes.length})`}
            </button>
            <button className="tb-btn" disabled={busy || boxes.length === 0} onClick={onClear}>
              清除全部
            </button>
          </>
        )}
        <button className="tb-btn" disabled={busy} onClick={onClose}>
          {stats ? '關閉' : '取消'}
        </button>
      </div>
    </div>
  )
}
