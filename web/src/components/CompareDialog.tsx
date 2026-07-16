import { useEffect, useState } from 'react'
import {
  compareDocuments,
  listDocuments,
  type CompareResult,
  type DocInfo,
  type DocMeta,
} from '../api'

interface Props {
  doc: DocInfo
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}

function CompareResultView({
  result,
  onClose,
  onOpenDoc,
}: {
  result: CompareResult
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}) {
  const { report } = result
  const { stats } = report

  return (
    <div className="modal-body">
      {report.summary ? (
        <div className="modal-subtitle">
          <strong>AI 摘要</strong>
          <p>{report.summary}</p>
        </div>
      ) : (
        <div className="annot-hint">未設定 API 金鑰或摘要呼叫失敗，略過 AI 摘要（其餘比對結果不受影響）。</div>
      )}

      <div className="modal-subtitle">
        新增 {stats.pagesAdded} 頁、刪除 {stats.pagesDeleted} 頁、內容變動 {stats.pagesModified} 頁、
        文字差異片段 {stats.textChangesTotal} 處
      </div>

      <p>新文件已建立：{result.document.filename}</p>
      <div className="modal-footer">
        <button className="tb-btn btn-primary" onClick={() => void onOpenDoc(result.document.id)}>
          切換開啟
        </button>
        <button className="tb-btn" onClick={onClose}>
          關閉
        </button>
      </div>
    </div>
  )
}

export default function CompareDialog({ doc, onClose, onOpenDoc }: Props) {
  const [docs, setDocs] = useState<DocMeta[]>([])
  const [oldId, setOldId] = useState(doc.id)
  const [newId, setNewId] = useState('')
  const [visualDiff, setVisualDiff] = useState(true)
  const [llmSummary, setLlmSummary] = useState(true)
  const [filename, setFilename] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<CompareResult | null>(null)

  useEffect(() => {
    listDocuments()
      .then((list) => {
        setDocs(list)
        // 預設「修改後文件」選第一個非目前文件的項目，方便直接送出。
        const other = list.find((d) => d.id !== doc.id)
        if (other) setNewId(other.id)
      })
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
  }, [doc.id])

  const swap = () => {
    setOldId(newId)
    setNewId(oldId)
  }

  const submit = async () => {
    if (!oldId || !newId) {
      setError('請分別選擇原始文件與修改後文件')
      return
    }
    if (oldId === newId) {
      setError('原始文件與修改後文件不可相同')
      return
    }
    setBusy(true)
    setError(null)
    try {
      const res = await compareDocuments(oldId, newId, {
        visualDiff,
        llmSummary,
        filename: filename.trim() || undefined,
      })
      setResult(res)
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
          <span>比較文件</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        {result ? (
          <CompareResultView result={result} onClose={onClose} onOpenDoc={onOpenDoc} />
        ) : (
          <div className="modal-body">
            {error && <div className="annot-hint">{error}</div>}

            <div className="modal-subtitle">原始文件</div>
            <select className="modal-input" value={oldId} onChange={(e) => setOldId(e.target.value)}>
              <option value="" disabled>
                請選擇
              </option>
              {docs.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.filename}
                  {d.id === doc.id ? '（目前）' : ''}
                </option>
              ))}
            </select>

            <div style={{ textAlign: 'center', margin: '4px 0' }}>
              <button className="tb-btn" title="交換" onClick={swap}>
                ⇅ 交換
              </button>
            </div>

            <div className="modal-subtitle">修改後文件</div>
            <select className="modal-input" value={newId} onChange={(e) => setNewId(e.target.value)}>
              <option value="" disabled>
                請選擇
              </option>
              {docs.map((d) => (
                <option key={d.id} value={d.id}>
                  {d.filename}
                  {d.id === doc.id ? '（目前）' : ''}
                </option>
              ))}
            </select>

            <label className="doc-list-item">
              <input type="checkbox" checked={visualDiff} onChange={(e) => setVisualDiff(e.target.checked)} />
              <span>包含視覺（像素）差異比對</span>
            </label>
            <label className="doc-list-item">
              <input type="checkbox" checked={llmSummary} onChange={(e) => setLlmSummary(e.target.checked)} />
              <span>產生 AI 摘要（未設定 API 金鑰時會自動略過）</span>
            </label>

            <div className="modal-subtitle">新檔名（可選）</div>
            <input
              className="modal-input"
              placeholder={`compare_${oldId ? docs.find((d) => d.id === oldId)?.filename ?? '' : ''}`}
              value={filename}
              onChange={(e) => setFilename(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void submit()
              }}
            />

            <div className="modal-footer">
              <button className="tb-btn btn-primary" disabled={busy} onClick={() => void submit()}>
                {busy ? '比較中…' : '比較'}
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
