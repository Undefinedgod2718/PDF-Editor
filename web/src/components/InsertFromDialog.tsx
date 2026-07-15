import { useEffect, useState } from 'react'
import { fetchDocInfo, insertPagesFrom, listDocuments, type DocInfo, type DocMeta } from '../api'
import { parsePageSpec } from '../lib/pageSpec'

interface Props {
  doc: DocInfo
  onClose: () => void
  onApplied: () => void | Promise<void>
}

type Position = 'before' | 'end'

export default function InsertFromDialog({ doc, onClose, onApplied }: Props) {
  const [docs, setDocs] = useState<DocMeta[]>([])
  const [sourceId, setSourceId] = useState('')
  const [sourceInfo, setSourceInfo] = useState<DocInfo | null>(null)
  const [spec, setSpec] = useState('全部')
  const [position, setPosition] = useState<Position>('end')
  const [beforePage, setBeforePage] = useState(1)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    listDocuments()
      .then((list) => {
        setDocs(list)
        setSourceId((id) => id || list[0]?.id || '')
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
  }, [])

  useEffect(() => {
    if (!sourceId) {
      setSourceInfo(null)
      return
    }
    let cancelled = false
    setSourceInfo(null)
    setError(null)
    fetchDocInfo(sourceId)
      .then((info) => {
        if (!cancelled) setSourceInfo(info)
      })
      .catch((err) => {
        if (!cancelled) {
          setSourceInfo(null)
          setError(err instanceof Error ? err.message : String(err))
        }
      })
    return () => {
      cancelled = true
    }
  }, [sourceId])

  const submit = async () => {
    if (!sourceInfo || sourceInfo.id !== sourceId) return
    let pages: number[]
    try {
      const trimmed = spec.trim()
      pages =
        trimmed === '' || trimmed === '全部'
          ? Array.from({ length: sourceInfo.pageCount }, (_, i) => i)
          : parsePageSpec(trimmed, sourceInfo.pageCount, { preserveOrder: true })
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    if (pages.length === 0) {
      setError('請選擇至少一頁')
      return
    }
    const at = position === 'end' ? doc.pageCount : Math.min(Math.max(beforePage - 1, 0), doc.pageCount)
    setBusy(true)
    setError(null)
    try {
      await insertPagesFrom(doc.id, sourceId, pages, at)
      await onApplied()
      onClose()
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
          <span>從其他文件插入頁面</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}

          <div className="modal-subtitle">來源文件</div>
          <select className="modal-input" value={sourceId} onChange={(e) => setSourceId(e.target.value)}>
            {docs.map((d) => (
              <option key={d.id} value={d.id}>
                {d.filename}
                {d.id === doc.id ? '（本文件）' : ''}
              </option>
            ))}
          </select>

          <div className="modal-subtitle">
            頁碼（1-based，如 1,4,2 依序插入，或「全部」）
            {sourceInfo ? `，共 ${sourceInfo.pageCount} 頁` : ''}
          </div>
          <input
            className="modal-input"
            placeholder="全部"
            value={spec}
            onChange={(e) => setSpec(e.target.value)}
          />

          <div className="modal-subtitle">插入位置</div>
          <div className="radio-group">
            <label>
              <input type="radio" checked={position === 'before'} onChange={() => setPosition('before')} />
              插入到第
              <input
                type="number"
                className="modal-input inline-num"
                min={1}
                max={doc.pageCount + 1}
                value={beforePage}
                onChange={(e) => {
                  setBeforePage(Number(e.target.value))
                  setPosition('before')
                }}
              />
              頁之前
            </label>
            <label>
              <input type="radio" checked={position === 'end'} onChange={() => setPosition('end')} /> 插入到最後
            </label>
          </div>

          <div className="modal-footer">
            <button
              className="tb-btn btn-primary"
              disabled={busy || !sourceId || !sourceInfo || sourceInfo.id !== sourceId}
              onClick={() => void submit()}
            >
              {busy ? '插入中…' : '插入'}
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
