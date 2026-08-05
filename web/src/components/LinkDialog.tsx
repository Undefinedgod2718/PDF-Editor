import { useState } from 'react'
import { createLink, type DocInfo, type Rect } from '../api'

interface Props {
  doc: DocInfo
  page: number
  /** 使用者在頁面上拉出的範圍（view-space points）。 */
  rectPt: Rect
  onClose: () => void
  onCreated: () => void
}

export default function LinkDialog({ doc, page, rectPt, onClose, onCreated }: Props) {
  const [kind, setKind] = useState<'page' | 'uri'>('page')
  const [targetPage, setTargetPage] = useState(Math.min(page + 1, doc.pageCount - 1))
  const [url, setUrl] = useState('https://')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      await createLink(
        doc.id,
        page,
        rectPt,
        kind === 'page' ? { target: 'page', page: targetPage } : { target: 'uri', url },
      )
      onCreated()
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
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
          <span>建立連結</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}

          <div className="modal-subtitle">連結目標</div>
          <select
            className="modal-input"
            value={kind}
            onChange={(e) => setKind(e.target.value as 'page' | 'uri')}
          >
            <option value="page">跳到本文件的某一頁</option>
            <option value="uri">開啟網址</option>
          </select>

          {kind === 'page' ? (
            <>
              <div className="modal-subtitle">目標頁（1–{doc.pageCount}）</div>
              <input
                className="modal-input"
                type="number"
                min={1}
                max={doc.pageCount}
                value={targetPage + 1}
                onChange={(e) => {
                  const p = Number(e.target.value) - 1
                  if (p >= 0 && p < doc.pageCount) setTargetPage(p)
                }}
              />
            </>
          ) : (
            <>
              <div className="modal-subtitle">網址</div>
              <input
                className="modal-input"
                value={url}
                placeholder="https://example.com"
                onChange={(e) => setUrl(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') void submit()
                }}
              />
              {/* 後端只接受這三種 scheme，先在這裡講清楚，免得按了才吃 400。 */}
              <div className="modal-subtitle">只接受 http://、https:// 或 mailto:</div>
            </>
          )}

          <div className="modal-footer">
            <button className="tb-btn btn-primary" disabled={busy} onClick={() => void submit()}>
              {busy ? '建立中…' : '建立'}
            </button>
            <button className="tb-btn" onClick={onClose}>
              取消
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}
