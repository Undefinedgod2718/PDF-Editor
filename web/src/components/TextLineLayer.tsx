import { useEffect, useState } from 'react'
import {
  editPageLine,
  insertPageLine,
  listPageLines,
  shiftPageLine,
  type LineInfo,
} from '../api'

interface Props {
  docId: string
  page: number
  scale: number
  /** 該頁版本號，變更後重新抓行列表（行 index 每次變更後都會失效）。 */
  version: number
  onChanged: () => void
}

/**
 * P15 行編輯層（editLine 工具）：點選行開啟 popup，可改寫該行文字、
 * 在其下方插入複製樣式的新行（可選擇先把下方內容下移一行）、
 * 或把行上移/下移一個行距。無 reflow：一切都是就地改寫與平移。
 */
export default function TextLineLayer({ docId, page, scale, version, onChanged }: Props) {
  const [lines, setLines] = useState<LineInfo[]>([])
  const [selected, setSelected] = useState<LineInfo | null>(null)
  const [editValue, setEditValue] = useState('')
  const [insertValue, setInsertValue] = useState('')
  const [shiftDown, setShiftDown] = useState(true)
  const [andBelow, setAndBelow] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setSelected(null)
    listPageLines(docId, page)
      .then((res) => {
        if (!cancelled) setLines(res)
      })
      .catch((err) => console.error('listPageLines failed:', err))
    return () => {
      cancelled = true
    }
  }, [docId, page, version])

  const select = (line: LineInfo, e: React.MouseEvent) => {
    e.stopPropagation()
    setSelected(line)
    setEditValue(line.text)
    setInsertValue('')
    setError(null)
  }

  /** 送出一個變更；成功後由上層 bump version 觸發重抓，popup 關閉。 */
  const run = async (fn: () => Promise<unknown>) => {
    if (busy) return
    setBusy(true)
    setError(null)
    try {
      await fn()
      setSelected(null)
      onChanged()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  const submitEdit = () => {
    if (!selected) return
    void run(() => editPageLine(docId, page, selected.index, editValue))
  }

  const submitInsert = () => {
    if (!selected || !insertValue.trim()) return
    void run(() => insertPageLine(docId, page, selected.index, insertValue, shiftDown))
  }

  const submitShift = (dir: 1 | -1) => {
    if (!selected) return
    const delta = dir * Math.max(selected.h, selected.font_size) * 1.2
    void run(() => shiftPageLine(docId, page, selected.index, delta, andBelow))
  }

  return (
    <div className="line-layer">
      {lines.map((l) => (
        <div
          key={l.index}
          className={`text-line-box ${selected?.index === l.index ? 'active' : ''}`}
          style={{ left: l.x * scale, top: l.y * scale, width: l.w * scale, height: l.h * scale }}
          title={l.text}
          onClick={(e) => select(l, e)}
        />
      ))}

      {selected && (
        <div
          className="annot-popup line-popup"
          style={{
            left: selected.x * scale,
            top: (selected.y + selected.h) * scale + 4,
          }}
        >
          <textarea
            autoFocus
            value={editValue}
            onChange={(e) => setEditValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') setSelected(null)
            }}
          />
          <div className="annot-popup-actions">
            <button className="tb-btn" disabled={busy} onClick={submitEdit}>
              更新文字
            </button>
            <button className="tb-btn" disabled={busy} onClick={() => setSelected(null)}>
              取消
            </button>
          </div>

          <div className="line-popup-section">
            <input
              type="text"
              placeholder="新行文字（複製此行樣式）…"
              value={insertValue}
              onChange={(e) => setInsertValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') submitInsert()
                if (e.key === 'Escape') setSelected(null)
              }}
            />
            <div className="annot-popup-actions">
              <label className="line-popup-check">
                <input
                  type="checkbox"
                  checked={shiftDown}
                  onChange={(e) => setShiftDown(e.target.checked)}
                />
                下方內容下移
              </label>
              <button className="tb-btn" disabled={busy || !insertValue.trim()} onClick={submitInsert}>
                插入下一行
              </button>
            </div>
          </div>

          <div className="annot-popup-actions">
            <label className="line-popup-check">
              <input
                type="checkbox"
                checked={andBelow}
                onChange={(e) => setAndBelow(e.target.checked)}
              />
              連同下方行
            </label>
            <button className="tb-btn" disabled={busy} onClick={() => submitShift(-1)}>
              上移
            </button>
            <button className="tb-btn" disabled={busy} onClick={() => submitShift(1)}>
              下移
            </button>
          </div>

          {error && <div className="line-popup-error">{error}</div>}
        </div>
      )}
    </div>
  )
}
