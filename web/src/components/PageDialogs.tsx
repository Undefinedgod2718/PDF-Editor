import { useEffect, useState } from 'react'
import { extractPages, listDocuments, mergeDocuments, type DocInfo, type DocMeta } from '../api'
import { parsePageSpec } from '../lib/pageSpec'

interface DialogProps {
  doc: DocInfo
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}

function CreatedResult({
  created,
  onClose,
  onOpenDoc,
}: {
  created: DocMeta
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}) {
  return (
    <div className="modal-body">
      <p>新文件已建立：{created.filename}</p>
      <div className="modal-footer">
        <button className="tb-btn btn-primary" onClick={() => void onOpenDoc(created.id)}>
          切換開啟
        </button>
        <button className="tb-btn" onClick={onClose}>
          關閉
        </button>
      </div>
    </div>
  )
}

export function MergeDialog({ doc, onClose, onOpenDoc }: DialogProps) {
  const [docs, setDocs] = useState<DocMeta[]>([])
  const [selected, setSelected] = useState<string[]>([])
  const [filename, setFilename] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [created, setCreated] = useState<DocMeta | null>(null)

  useEffect(() => {
    listDocuments()
      .then(setDocs)
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
  }, [])

  const toggle = (id: string) => {
    setSelected((s) => (s.includes(id) ? s.filter((x) => x !== id) : [...s, id]))
  }

  const move = (id: string, dir: -1 | 1) => {
    setSelected((s) => {
      const i = s.indexOf(id)
      const j = i + dir
      if (i < 0 || j < 0 || j >= s.length) return s
      const next = [...s]
      ;[next[i], next[j]] = [next[j], next[i]]
      return next
    })
  }

  const submit = async () => {
    if (selected.length < 2) {
      setError('請至少勾選 2 份文件')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const meta = await mergeDocuments(selected, filename.trim() || undefined)
      setCreated(meta)
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
          <span>合併文件</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        {created ? (
          <CreatedResult created={created} onClose={onClose} onOpenDoc={onOpenDoc} />
        ) : (
          <div className="modal-body">
            {error && <div className="annot-hint">{error}</div>}
            <div className="modal-subtitle">勾選要合併的文件</div>
            <div className="doc-list">
              {docs.map((d) => (
                <label key={d.id} className="doc-list-item">
                  <input type="checkbox" checked={selected.includes(d.id)} onChange={() => toggle(d.id)} />
                  <span className="doc-list-name">{d.filename}</span>
                  {d.id === doc.id && <span className="doc-list-current">目前</span>}
                </label>
              ))}
            </div>
            {selected.length > 0 && (
              <>
                <div className="modal-subtitle">合併順序</div>
                <div className="doc-order-list">
                  {selected.map((id, i) => {
                    const d = docs.find((x) => x.id === id)
                    return (
                      <div key={id} className="doc-order-item">
                        <span>
                          {i + 1}. {d?.filename ?? id}
                        </span>
                        <div className="doc-order-actions">
                          <button className="tb-btn" disabled={i === 0} onClick={() => move(id, -1)}>
                            ↑
                          </button>
                          <button
                            className="tb-btn"
                            disabled={i === selected.length - 1}
                            onClick={() => move(id, 1)}
                          >
                            ↓
                          </button>
                          <button className="tb-btn" onClick={() => toggle(id)}>
                            移除
                          </button>
                        </div>
                      </div>
                    )
                  })}
                </div>
              </>
            )}
            <input
              className="modal-input"
              placeholder="檔名（可選）"
              value={filename}
              onChange={(e) => setFilename(e.target.value)}
            />
            <div className="modal-footer">
              <button className="tb-btn btn-primary" disabled={busy || selected.length < 2} onClick={() => void submit()}>
                {busy ? '合併中…' : '合併'}
              </button>
              <button className="tb-btn" onClick={onClose}>
                取消
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

export function ExtractDialog({ doc, onClose, onOpenDoc }: DialogProps) {
  const [spec, setSpec] = useState('')
  const [filename, setFilename] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [created, setCreated] = useState<DocMeta | null>(null)

  const submit = async () => {
    let pages: number[]
    try {
      pages = parsePageSpec(spec, doc.pageCount)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    setBusy(true)
    setError(null)
    try {
      const meta = await extractPages(doc.id, pages, filename.trim() || undefined)
      setCreated(meta)
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
          <span>擷取頁面</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        {created ? (
          <CreatedResult created={created} onClose={onClose} onOpenDoc={onOpenDoc} />
        ) : (
          <div className="modal-body">
            {error && <div className="annot-hint">{error}</div>}
            <div className="modal-subtitle">頁碼（1-based，如 1,3-5），共 {doc.pageCount} 頁</div>
            <input
              className="modal-input"
              placeholder="例：1,3-5"
              value={spec}
              onChange={(e) => setSpec(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void submit()
              }}
            />
            <input
              className="modal-input"
              placeholder="檔名（可選）"
              value={filename}
              onChange={(e) => setFilename(e.target.value)}
            />
            <div className="modal-footer">
              <button className="tb-btn btn-primary" disabled={busy} onClick={() => void submit()}>
                {busy ? '擷取中…' : '擷取'}
              </button>
              <button className="tb-btn" onClick={onClose}>
                取消
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}
