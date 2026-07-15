import { useState } from 'react'
import { cropPages, type DocInfo, type Rect } from '../api'
import { parsePageSpec } from '../lib/pageSpec'

interface Props {
  doc: DocInfo
  currentPage: number
  /** 目前在頁面上拖曳出的選取範圍（view-space points），尚未拖曳時為 null。 */
  rect: Rect | null
  /** 裁切／重設成功後呼叫，重新抓文件資訊＋刷新渲染版本（同其他頁面結構操作）。 */
  onApplied: () => void | Promise<void>
  /** 關閉裁切模式（取消、Esc、成功後皆會呼叫）。 */
  onClose: () => void
}

type Scope = 'current' | 'all' | 'range'

export default function CropBar({ doc, currentPage, rect, onApplied, onClose }: Props) {
  const [scope, setScope] = useState<Scope>('current')
  const [rangeSpec, setRangeSpec] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const resolvePages = (): number[] => {
    if (scope === 'current') return [currentPage]
    if (scope === 'all') return Array.from({ length: doc.pageCount }, (_, i) => i)
    return parsePageSpec(rangeSpec, doc.pageCount)
  }

  const run = async (targetRect: Rect | null) => {
    let pages: number[]
    try {
      pages = resolvePages()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    setBusy(true)
    setError(null)
    try {
      await cropPages(doc.id, pages, targetRect)
      await onApplied()
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="crop-bar">
      <div className="crop-bar-header">
        <span>裁切頁面</span>
        <button className="tb-btn" onClick={onClose}>
          ✕
        </button>
      </div>

      {error && <div className="annot-hint">{error}</div>}

      <div className="crop-bar-status">
        {rect
          ? `已選取範圍：${rect.w.toFixed(0)} × ${rect.h.toFixed(0)} pt`
          : '請在頁面上拖曳選取裁切範圍'}
      </div>

      <div className="radio-group">
        <label>
          <input type="radio" checked={scope === 'current'} onChange={() => setScope('current')} /> 本頁
        </label>
        <label>
          <input type="radio" checked={scope === 'all'} onChange={() => setScope('all')} /> 全部頁面
        </label>
        <label>
          <input type="radio" checked={scope === 'range'} onChange={() => setScope('range')} /> 頁面範圍
        </label>
        {scope === 'range' && (
          <input
            className="modal-input"
            placeholder="例：1,3-5"
            value={rangeSpec}
            onChange={(e) => setRangeSpec(e.target.value)}
          />
        )}
      </div>

      <div className="crop-bar-actions">
        <button className="tb-btn btn-primary" disabled={busy || !rect} onClick={() => void run(rect)}>
          {busy ? '套用中…' : '確認裁切'}
        </button>
        <button className="tb-btn" disabled={busy} onClick={() => void run(null)}>
          重設裁切
        </button>
        <button className="tb-btn" disabled={busy} onClick={onClose}>
          取消
        </button>
      </div>
    </div>
  )
}
