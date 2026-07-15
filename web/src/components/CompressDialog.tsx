import { useState } from 'react'
import { compressDocument, type CompressPreset, type CompressResult, type DocInfo } from '../api'

interface Props {
  doc: DocInfo
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}

const PRESET_LABELS: Record<CompressPreset, string> = {
  screen: '螢幕（72 DPI／品質 60）',
  ebook: '電子書（150 DPI／品質 75）',
  printer: '印刷（300 DPI／品質 85）',
  custom: '自訂',
}

const PRESET_ORDER: CompressPreset[] = ['screen', 'ebook', 'printer', 'custom']

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ['KB', 'MB', 'GB']
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[i]}`
}

function CompressedResult({
  result,
  onClose,
  onOpenDoc,
}: {
  result: CompressResult
  onClose: () => void
  onOpenDoc: (id: string) => void | Promise<void>
}) {
  const { before, after, stats } = result
  const savedPct = before > 0 ? Math.max(0, ((before - after) / before) * 100) : 0

  return (
    <div className="modal-body">
      <p>新文件已建立：{result.document.filename}</p>
      <div className="compress-result-sizes">
        <span>{formatBytes(before)}</span>
        <span>→</span>
        <span>{formatBytes(after)}</span>
        <span className="compress-result-pct">（省下 {savedPct.toFixed(1)}%）</span>
      </div>
      <div className="modal-subtitle">
        重壓 {stats.images_recompressed} 張圖片、略過 {stats.images_skipped} 張、去重 {stats.duplicates_merged}{' '}
        筆、清除物件 {stats.objects_pruned} 個
      </div>
      {after >= before && <div className="annot-hint">壓縮無明顯效果時仍會產生新文件。</div>}
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

export default function CompressDialog({ doc, onClose, onOpenDoc }: Props) {
  const [preset, setPreset] = useState<CompressPreset>('ebook')
  const [dpi, setDpi] = useState(150)
  const [quality, setQuality] = useState(75)
  const [filename, setFilename] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<CompressResult | null>(null)

  const defaultFilename = `compressed_${doc.filename}`

  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      const res = await compressDocument(doc.id, {
        preset,
        dpi: preset === 'custom' ? dpi : undefined,
        quality: preset === 'custom' ? quality : undefined,
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
          <span>壓縮文件</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        {result ? (
          <CompressedResult result={result} onClose={onClose} onOpenDoc={onOpenDoc} />
        ) : (
          <div className="modal-body">
            {error && <div className="annot-hint">{error}</div>}

            <div className="modal-subtitle">壓縮設定</div>
            <select
              className="modal-input"
              value={preset}
              onChange={(e) => setPreset(e.target.value as CompressPreset)}
            >
              {PRESET_ORDER.map((p) => (
                <option key={p} value={p}>
                  {PRESET_LABELS[p]}
                </option>
              ))}
            </select>

            {preset === 'custom' && (
              <>
                <div className="modal-subtitle">解析度 (DPI, 36–600)</div>
                <input
                  type="number"
                  className="modal-input"
                  min={36}
                  max={600}
                  value={dpi}
                  onChange={(e) => setDpi(Math.min(600, Math.max(36, Number(e.target.value) || 36)))}
                />
                <div className="modal-subtitle">畫質（{quality}，10–100）</div>
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

            <div className="modal-subtitle">新檔名（可選）</div>
            <input
              className="modal-input"
              placeholder={defaultFilename}
              value={filename}
              onChange={(e) => setFilename(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') void submit()
              }}
            />

            <div className="modal-footer">
              <button className="tb-btn btn-primary" disabled={busy} onClick={() => void submit()}>
                {busy ? '壓縮中…' : '壓縮'}
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
