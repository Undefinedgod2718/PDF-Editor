import { useState } from 'react'
import { applyWatermark, type DocInfo } from '../api'
import { parsePageSpec } from '../lib/pageSpec'

interface Props {
  doc: DocInfo
  onClose: () => void
  /** 套用成功後通知上層重畫（浮水印會改變每一頁的外觀）。 */
  onApplied: () => void
}

const PRESETS = ['機密', '草稿', '副本', '請勿外流', 'CONFIDENTIAL', 'DRAFT']

export default function WatermarkDialog({ doc, onClose, onApplied }: Props) {
  const [text, setText] = useState('機密')
  const [allPages, setAllPages] = useState(true)
  const [spec, setSpec] = useState('')
  const [fontSize, setFontSize] = useState(48)
  const [opacity, setOpacity] = useState(25)
  const [rotation, setRotation] = useState(45)
  const [color, setColor] = useState('#808080')
  const [behind, setBehind] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      const pages = allPages ? [] : parsePageSpec(spec, doc.pageCount)
      await applyWatermark(doc.id, {
        text,
        pages,
        fontSize,
        opacity: opacity / 100,
        color: hexToRgb(color),
        rotation,
        behind,
      })
      onApplied()
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
          <span>浮水印</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}

          <div className="modal-subtitle">文字</div>
          <input
            className="modal-input"
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void submit()
            }}
          />
          <div className="stamp-presets">
            {PRESETS.map((p) => (
              <button key={p} className="tb-btn" onClick={() => setText(p)}>
                {p}
              </button>
            ))}
          </div>

          <div className="modal-subtitle">套用範圍</div>
          <label className="stamp-check">
            <input
              type="checkbox"
              checked={allPages}
              onChange={(e) => setAllPages(e.target.checked)}
            />
            全部 {doc.pageCount} 頁
          </label>
          {!allPages && (
            <input
              className="modal-input"
              placeholder="例如 1,3-5"
              value={spec}
              onChange={(e) => setSpec(e.target.value)}
            />
          )}

          <div className="modal-subtitle">大小（{fontSize} pt）</div>
          <input
            type="range"
            min={8}
            max={200}
            value={fontSize}
            onChange={(e) => setFontSize(Number(e.target.value))}
          />

          <div className="modal-subtitle">不透明度（{opacity}%）</div>
          <input
            type="range"
            min={5}
            max={100}
            value={opacity}
            onChange={(e) => setOpacity(Number(e.target.value))}
          />

          <div className="modal-subtitle">旋轉（{rotation}°）</div>
          <input
            type="range"
            min={-90}
            max={90}
            value={rotation}
            onChange={(e) => setRotation(Number(e.target.value))}
          />

          <div className="modal-subtitle">顏色</div>
          <input
            type="color"
            className="modal-input"
            value={color}
            onChange={(e) => setColor(e.target.value)}
          />

          <label className="stamp-check">
            <input type="checkbox" checked={behind} onChange={(e) => setBehind(e.target.checked)} />
            壓在內容底下（文字才看得清楚）
          </label>

          {/* 這個動作沒有「取消浮水印」，只能靠存檔前放棄變更或另存新檔。 */}
          <div className="annot-hint">套用後無法單獨移除，請先確認再套用。</div>

          <div className="modal-footer">
            <button
              className="tb-btn btn-primary"
              disabled={busy || !text.trim()}
              onClick={() => void submit()}
            >
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

function hexToRgb(hex: string): { r: number; g: number; b: number } {
  const n = Number.parseInt(hex.slice(1), 16)
  return { r: (n >> 16) & 255, g: (n >> 8) & 255, b: n & 255 }
}
