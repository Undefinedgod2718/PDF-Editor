import { useMemo, useState } from 'react'
import { exportDocument, type DocInfo, type ExportFormat } from '../api'
import { parsePageSpec } from '../lib/pageSpec'

interface Props {
  doc: DocInfo
  onClose: () => void
}

const DPI_OPTIONS = [72, 150, 300, 600]

const FORMAT_HINTS: Record<ExportFormat, string> = {
  png: 'PNG：每頁一張圖（多頁會打包成 zip）',
  jpg: 'JPG：每頁一張圖（多頁會打包成 zip）',
  tiff: 'TIFF：單一多頁檔',
  pptx: 'PPTX：每頁一張投影片',
  docx: 'Word：文字轉換，轉檔可能需要較長時間，請耐心等候',
  xlsx: 'Excel：表格轉換，轉檔可能需要較長時間，請耐心等候',
}

const RASTER_FORMATS: ExportFormat[] = ['png', 'jpg', 'tiff', 'pptx']

export default function ExportDialog({ doc, onClose }: Props) {
  const [format, setFormat] = useState<ExportFormat>('png')
  const [spec, setSpec] = useState('')
  const [dpi, setDpi] = useState(150)
  const [quality, setQuality] = useState(85)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // 空字串＝全部頁面（省略 pages 欄位）；非空則交給 parsePageSpec 解析並驗證範圍。
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

  const valid = specError === null
  const isRaster = RASTER_FORMATS.includes(format)

  const submit = async () => {
    if (!valid) return
    setBusy(true)
    setError(null)
    try {
      await exportDocument(doc.id, {
        format,
        pages,
        dpi: isRaster ? dpi : undefined,
        quality: format === 'jpg' ? quality : undefined,
      })
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
          <span>匯出文件</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}

          <div className="modal-subtitle">格式</div>
          <select
            className="modal-input"
            value={format}
            onChange={(e) => setFormat(e.target.value as ExportFormat)}
          >
            <option value="png">PNG</option>
            <option value="jpg">JPG</option>
            <option value="tiff">TIFF</option>
            <option value="pptx">PPTX</option>
            <option value="docx">Word (.docx)</option>
            <option value="xlsx">Excel (.xlsx)</option>
          </select>
          <div className="export-format-hint">{FORMAT_HINTS[format]}</div>

          <div className="modal-subtitle">
            頁碼範圍（1-based，如 1,3,5-9），共 {doc.pageCount} 頁；留空＝全部頁面
          </div>
          <input
            className="modal-input"
            placeholder="全部"
            value={spec}
            onChange={(e) => setSpec(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submit()
            }}
          />
          {specError && <div className="annot-hint">{specError}</div>}

          {isRaster && (
            <>
              <div className="modal-subtitle">解析度 (DPI)</div>
              <select className="modal-input" value={dpi} onChange={(e) => setDpi(Number(e.target.value))}>
                {DPI_OPTIONS.map((d) => (
                  <option key={d} value={d}>
                    {d}
                  </option>
                ))}
              </select>
            </>
          )}

          {format === 'jpg' && (
            <>
              <div className="modal-subtitle">畫質（{quality}）</div>
              <div className="export-quality-row">
                <input
                  type="range"
                  min={10}
                  max={100}
                  value={quality}
                  onChange={(e) => setQuality(Number(e.target.value))}
                />
                <input
                  type="number"
                  className="modal-input inline-num"
                  min={10}
                  max={100}
                  value={quality}
                  onChange={(e) => setQuality(Math.min(100, Math.max(10, Number(e.target.value) || 10)))}
                />
              </div>
            </>
          )}

          <div className="modal-footer">
            <button className="tb-btn btn-primary" disabled={busy || !valid} onClick={() => void submit()}>
              {busy ? '匯出中…' : '匯出'}
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
