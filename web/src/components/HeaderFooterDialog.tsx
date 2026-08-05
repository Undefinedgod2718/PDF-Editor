import { useState } from 'react'
import { applyHeaderFooter, type DocInfo } from '../api'
import { parsePageSpec } from '../lib/pageSpec'

interface Props {
  doc: DocInfo
  onClose: () => void
  onApplied: () => void
}

type SlotKey =
  | 'headerLeft'
  | 'headerCenter'
  | 'headerRight'
  | 'footerLeft'
  | 'footerCenter'
  | 'footerRight'

const SLOTS: { key: SlotKey; label: string }[] = [
  { key: 'headerLeft', label: '頁首左' },
  { key: 'headerCenter', label: '頁首中' },
  { key: 'headerRight', label: '頁首右' },
  { key: 'footerLeft', label: '頁尾左' },
  { key: 'footerCenter', label: '頁尾中' },
  { key: 'footerRight', label: '頁尾右' },
]

const EMPTY: Record<SlotKey, string> = {
  headerLeft: '',
  headerCenter: '',
  headerRight: '',
  footerLeft: '',
  footerCenter: '',
  footerRight: '',
}

/** {date} 由前端展開（時區與格式是瀏覽器才知道的事，見 api.ts 的說明）。 */
function today(): string {
  const now = new Date()
  const pad = (n: number) => String(n).padStart(2, '0')
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
}

export default function HeaderFooterDialog({ doc, onClose, onApplied }: Props) {
  const [slots, setSlots] = useState<Record<SlotKey, string>>({
    ...EMPTY,
    footerCenter: '第 {page} 頁，共 {pages} 頁',
  })
  const [allPages, setAllPages] = useState(true)
  const [spec, setSpec] = useState('')
  const [fontSize, setFontSize] = useState(10)
  const [margin, setMargin] = useState(36)
  const [startNumber, setStartNumber] = useState(1)
  const [color, setColor] = useState('#000000')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const anyText = SLOTS.some(({ key }) => slots[key].trim())

  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      const pages = allPages ? [] : parsePageSpec(spec, doc.pageCount)
      await applyHeaderFooter(doc.id, {
        ...slots,
        pages,
        fontSize,
        color: hexToRgb(color),
        margin,
        startNumber,
        date: today(),
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
          <span>頁首頁尾與頁碼</span>
          <button className="tb-btn" onClick={onClose}>
            ✕
          </button>
        </div>
        <div className="modal-body">
          {error && <div className="annot-hint">{error}</div>}

          <div className="modal-subtitle">
            可用代碼：{'{page}'} 目前頁碼、{'{pages}'} 總頁數、{'{date}'} 今天日期
          </div>
          {SLOTS.map(({ key, label }) => (
            <div className="hf-slot" key={key}>
              <span className="hf-slot-label">{label}</span>
              <input
                className="modal-input"
                value={slots[key]}
                onChange={(e) => setSlots({ ...slots, [key]: e.target.value })}
              />
            </div>
          ))}

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

          <div className="modal-subtitle">字級（pt）</div>
          <input
            className="modal-input"
            type="number"
            min={6}
            max={72}
            value={fontSize}
            onChange={(e) => setFontSize(Math.min(72, Math.max(6, Number(e.target.value) || 10)))}
          />

          <div className="modal-subtitle">距頁緣（pt）</div>
          <input
            className="modal-input"
            type="number"
            min={0}
            max={200}
            value={margin}
            onChange={(e) => setMargin(Math.min(200, Math.max(0, Number(e.target.value) || 0)))}
          />

          {/* 封面在別份檔案、正文要從 2 開始編號時用得到，對應 Acrobat 的「起始頁碼」。 */}
          <div className="modal-subtitle">第一頁的頁碼</div>
          <input
            className="modal-input"
            type="number"
            value={startNumber}
            onChange={(e) => setStartNumber(Number(e.target.value) || 1)}
          />

          <div className="modal-subtitle">顏色</div>
          <input
            type="color"
            className="modal-input"
            value={color}
            onChange={(e) => setColor(e.target.value)}
          />

          <div className="annot-hint">套用後無法單獨移除，請先確認再套用。</div>

          <div className="modal-footer">
            <button
              className="tb-btn btn-primary"
              disabled={busy || !anyText}
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
