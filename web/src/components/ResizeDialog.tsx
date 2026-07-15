import { useState } from 'react'
import { resizePages, type DocInfo, type ResizeMode } from '../api'
import { parsePageSpec } from '../lib/pageSpec'

interface Props {
  doc: DocInfo
  currentPage: number
  onClose: () => void
  onApplied: () => void | Promise<void>
}

interface Preset {
  label: string
  w: number
  h: number
}

const PRESETS: Preset[] = [
  { label: 'A4', w: 595, h: 842 },
  { label: 'Letter', w: 612, h: 792 },
  { label: 'Legal', w: 612, h: 1008 },
  { label: 'A3', w: 842, h: 1191 },
  { label: 'A5', w: 420, h: 595 },
  { label: 'Tabloid', w: 792, h: 1224 },
]

const CUSTOM = '自訂'
const MIN_PT = 36
const MAX_PT = 14400

type Scope = 'current' | 'all' | 'range'
type Orientation = 'portrait' | 'landscape'

export default function ResizeDialog({ doc, currentPage, onClose, onApplied }: Props) {
  const cur = doc.pages.find((p) => p.index === currentPage)
  const [presetLabel, setPresetLabel] = useState(CUSTOM)
  const [width, setWidth] = useState(cur?.width ?? 595)
  const [height, setHeight] = useState(cur?.height ?? 842)
  const [orientation, setOrientation] = useState<Orientation>('portrait')
  const [mode, setMode] = useState<ResizeMode>('scale')
  const [scope, setScope] = useState<Scope>('current')
  const [rangeSpec, setRangeSpec] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const applyPreset = (label: string) => {
    setPresetLabel(label)
    const p = PRESETS.find((x) => x.label === label)
    if (!p) return // 自訂：不變動目前寬高
    if (orientation === 'landscape') {
      setWidth(p.h)
      setHeight(p.w)
    } else {
      setWidth(p.w)
      setHeight(p.h)
    }
  }

  const toggleOrientation = () => {
    setOrientation((o) => (o === 'portrait' ? 'landscape' : 'portrait'))
    setWidth(height)
    setHeight(width)
  }

  const valid = width >= MIN_PT && width <= MAX_PT && height >= MIN_PT && height <= MAX_PT

  const submit = async () => {
    if (!valid) return
    let pages: number[]
    try {
      pages =
        scope === 'current'
          ? [currentPage]
          : scope === 'all'
            ? Array.from({ length: doc.pageCount }, (_, i) => i)
            : parsePageSpec(rangeSpec, doc.pageCount)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      return
    }
    setBusy(true)
    setError(null)
    try {
      await resizePages(doc.id, pages, width, height, mode)
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
          <span>調整頁面大小</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}

          <div className="modal-subtitle">紙張大小</div>
          <select className="modal-input" value={presetLabel} onChange={(e) => applyPreset(e.target.value)}>
            <option value={CUSTOM}>{CUSTOM}</option>
            {PRESETS.map((p) => (
              <option key={p.label} value={p.label}>
                {p.label}（{p.w}×{p.h} pt）
              </option>
            ))}
          </select>

          <div className="resize-dim-row">
            <label>
              寬 (pt)
              <input
                type="number"
                className="modal-input"
                value={width}
                onChange={(e) => {
                  setWidth(Number(e.target.value))
                  setPresetLabel(CUSTOM)
                }}
              />
            </label>
            <label>
              高 (pt)
              <input
                type="number"
                className="modal-input"
                value={height}
                onChange={(e) => {
                  setHeight(Number(e.target.value))
                  setPresetLabel(CUSTOM)
                }}
              />
            </label>
            <button className="tb-btn" onClick={toggleOrientation} title="切換直式/橫式">
              {orientation === 'portrait' ? '直式' : '橫式'} ⇄
            </button>
          </div>
          {!valid && (
            <div className="annot-hint">
              寬高需介於 {MIN_PT}–{MAX_PT} pt
            </div>
          )}

          <div className="modal-subtitle">調整模式</div>
          <div className="radio-group">
            <label>
              <input type="radio" checked={mode === 'scale'} onChange={() => setMode('scale')} /> 等比縮放內容
            </label>
            <label>
              <input type="radio" checked={mode === 'canvas'} onChange={() => setMode('canvas')} /> 僅改畫布（內容不縮放）
            </label>
          </div>

          <div className="modal-subtitle">套用範圍</div>
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

          <div className="modal-footer">
            <button className="tb-btn btn-primary" disabled={busy || !valid} onClick={() => void submit()}>
              {busy ? '套用中…' : '套用'}
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
