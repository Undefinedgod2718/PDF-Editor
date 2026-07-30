import { useEffect, useRef, useState } from 'react'
import {
  fetchOcrLanguages,
  pollOcrJob,
  startOcrJob,
  type DocInfo,
  type OcrLanguage,
  type OcrResult,
} from '../api'

interface Props {
  doc: DocInfo
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}

// Used only if GET /api/ocr/languages fails — keeps the dialog usable even
// when the language list can't be fetched, matching the server's own
// OcrOptions default (eng+chi_tra).
const FALLBACK_LANGUAGES: OcrLanguage[] = [
  { code: 'eng', label: '英文' },
  { code: 'chi_tra', label: '繁體中文' },
]
const DEFAULT_SELECTED = new Set(['eng', 'chi_tra'])

const POLL_INTERVAL_MS = 700

function OcrResultView({
  result,
  onClose,
  onOpenDoc,
}: {
  result: OcrResult
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}) {
  const { stats } = result
  return (
    <div className="modal-body">
      <p>新文件已建立：{result.document.filename}</p>
      <div className="modal-subtitle">
        處理 {stats.pages_processed} 頁、略過已有文字層 {stats.pages_skipped_existing_text} 頁、加入
        {stats.words_added} 個文字、捨棄低信心 {stats.words_skipped_low_confidence} 個
        {stats.words_skipped_no_font > 0 && `、缺字型跳過 ${stats.words_skipped_no_font} 個`}
        {stats.pages_truncated > 0 && `、${stats.pages_truncated} 頁辨識提前中止`}
      </div>
      {stats.words_added === 0 && (
        <div className="annot-hint">沒有加入任何辨識文字——可能全部頁面已有文字層，或掃描內容辨識不到字。</div>
      )}
      <div className="modal-footer">
        <button className="tb-btn btn-primary" onClick={() => void onOpenDoc(result.document.id)}>
          開啟新文件
        </button>
        <button className="tb-btn" onClick={onClose}>
          關閉
        </button>
      </div>
    </div>
  )
}

export default function OcrDialog({ doc, onClose, onOpenDoc }: Props) {
  const [languages, setLanguages] = useState<OcrLanguage[]>(FALLBACK_LANGUAGES)
  const [selected, setSelected] = useState<Set<string>>(DEFAULT_SELECTED)
  const [force, setForce] = useState(false)
  const [filename, setFilename] = useState('')
  const [busy, setBusy] = useState(false)
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<OcrResult | null>(null)
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null)

  useEffect(() => {
    let cancelled = false
    fetchOcrLanguages()
      .then((langs) => {
        if (cancelled || langs.length === 0) return
        setLanguages(langs)
        const codes = new Set(langs.map((l) => l.code))
        const defaults = new Set([...DEFAULT_SELECTED].filter((c) => codes.has(c)))
        setSelected(defaults.size > 0 ? defaults : codes)
      })
      .catch(() => {
        // stays on FALLBACK_LANGUAGES / DEFAULT_SELECTED — dialog still works
      })
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    return () => {
      if (pollRef.current) clearInterval(pollRef.current)
    }
  }, [])

  const toggleLang = (code: string) => {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(code)) next.delete(code)
      else next.add(code)
      return next
    })
  }

  const defaultFilename = `ocr_${doc.filename}`

  const submit = async () => {
    if (selected.size === 0) {
      setError('請至少選一個語言')
      return
    }
    setBusy(true)
    setError(null)
    setProgress(null)
    try {
      const { job_id } = await startOcrJob(doc.id, {
        langs: [...selected].join('+'),
        force,
        filename: filename.trim() || undefined,
      })
      // 上一次輪詢還沒回來就跳過這一拍。間隔只有 700ms，一旦某次請求變慢就會有
      // 兩個請求同時在途；後端讀到終態會把 job 從 map 移除（見 ocr_job_status），
      // 於是先到的那個拿到 done、後到的拿到 404，.catch 會把已經顯示的成功結果
      // 蓋成「no such OCR job」——文件其實早就存好了。
      let inFlight = false
      pollRef.current = setInterval(() => {
        if (inFlight) return
        inFlight = true
        pollOcrJob(doc.id, job_id)
          .then((status) => {
            if (status.status === 'running') {
              setProgress({ done: status.pages_done, total: status.pages_total })
              return
            }
            if (pollRef.current) {
              clearInterval(pollRef.current)
              pollRef.current = null
            }
            if (status.status === 'done') {
              setResult({ document: status.document, stats: status.stats })
            } else {
              setError(status.message)
            }
            setBusy(false)
          })
          .catch((err) => {
            if (pollRef.current) {
              clearInterval(pollRef.current)
              pollRef.current = null
            }
            setError(err instanceof Error ? err.message : String(err))
            setBusy(false)
          })
          .finally(() => {
            inFlight = false
          })
      }, POLL_INTERVAL_MS)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
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
          <span>光學字元辨識 (OCR)</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        {result ? (
          <OcrResultView result={result} onClose={onClose} onOpenDoc={onOpenDoc} />
        ) : (
          <div className="modal-body">
            {error && <div className="annot-hint">{error}</div>}
            <div className="annot-hint">
              對沒有文字層的掃描頁套用辨識，加入看不見的可搜尋文字層——畫面不變，可搜尋、複製、醒目標示。
            </div>

            <div className="modal-subtitle">語言</div>
            {languages.map((l) => (
              <label key={l.code} className="doc-list-item">
                <input
                  type="checkbox"
                  checked={selected.has(l.code)}
                  onChange={() => toggleLang(l.code)}
                  disabled={busy}
                />
                <span>{l.label}</span>
              </label>
            ))}

            <label className="doc-list-item">
              <input
                type="checkbox"
                checked={force}
                onChange={(e) => setForce(e.target.checked)}
                disabled={busy}
              />
              <span>強制重新辨識（含已有文字層的頁面）</span>
            </label>

            <div className="modal-subtitle">新檔名（可選）</div>
            <input
              className="modal-input"
              placeholder={defaultFilename}
              value={filename}
              disabled={busy}
              onChange={(e) => setFilename(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void submit()
              }}
            />

            {busy && (
              <div className="modal-subtitle">
                {progress && progress.total > 0
                  ? `處理中 ${progress.done}/${progress.total} 頁…`
                  : '處理中…'}
              </div>
            )}

            <div className="modal-footer">
              <button className="tb-btn btn-primary" disabled={busy} onClick={() => void submit()}>
                {busy ? '辨識中…' : '開始辨識'}
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
