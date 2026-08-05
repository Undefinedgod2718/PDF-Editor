import { useCallback, useEffect, useState } from 'react'
import {
  fetchOutline,
  saveOutline,
  type DocInfo,
  type NewOutlineItem,
  type OutlineItem,
} from '../api'

interface Props {
  doc: DocInfo
  currentPage: number
  gotoPage: (p: number) => void
  /** 書籤寫回後通知上層 bump revision（書籤不影響渲染，但文件已變髒）。 */
  onChanged: () => void
  onClose: () => void
}

/**
 * 面板內部把整棵樹攤平成一維陣列來編輯，`depth` 記層級。
 *
 * 巢狀結構直接編會很痛：改一個節點的層級要同時搬動它的整串子節點，插入要找到
 * 正確的兄弟位置。攤平之後「升/降一層」只是改自己的 depth（子節點跟著它連在
 * 後面，本來就相鄰），上下移動只是搬一段連續區間，存檔前再摺回巢狀即可。
 * Acrobat 的書籤面板實際上也是這個心智模型。
 */
interface FlatItem {
  /** 只在這個面板存活期間有效，用來當 React key 與選取目標。 */
  key: number
  title: string
  page: number
  depth: number
  open: boolean
}

let nextKey = 0

function flatten(items: OutlineItem[], depth: number, out: FlatItem[], pageCount: number) {
  for (const item of items) {
    out.push({
      key: nextKey++,
      title: item.title,
      // 後端解析不出目的地時給 null，這裡退回第一頁，使用者可以自己改。
      page: item.page !== null && item.page < pageCount ? item.page : 0,
      depth,
      open: item.open,
    })
    flatten(item.children, depth + 1, out, pageCount)
  }
}

/** 攤平陣列摺回巢狀；深度跳超過一層的（不該發生）當成只深一層處理。 */
function nest(flat: FlatItem[]): NewOutlineItem[] {
  const roots: NewOutlineItem[] = []
  const stack: NewOutlineItem[] = []
  for (const item of flat) {
    const node: NewOutlineItem = {
      title: item.title.trim(),
      page: item.page,
      open: item.open,
      children: [],
    }
    const depth = Math.min(item.depth, stack.length)
    stack.length = depth
    if (depth === 0) roots.push(node)
    else stack[depth - 1].children.push(node)
    stack.push(node)
  }
  return roots
}

export default function OutlinePanel({ doc, currentPage, gotoPage, onChanged, onClose }: Props) {
  const [items, setItems] = useState<FlatItem[]>([])
  const [selected, setSelected] = useState<number | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [dirty, setDirty] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const tree = await fetchOutline(doc.id)
      const flat: FlatItem[] = []
      flatten(tree, 0, flat, doc.pageCount)
      setItems(flat)
      setDirty(false)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [doc.id, doc.pageCount])

  useEffect(() => {
    void load()
  }, [load])

  const mutate = (next: FlatItem[]) => {
    setItems(next)
    setDirty(true)
  }

  const addHere = () => {
    const at = selected === null ? items.length : indexOfKey(selected) + 1
    const item: FlatItem = {
      key: nextKey++,
      title: `第 ${currentPage + 1} 頁`,
      page: currentPage,
      depth: selected === null ? 0 : items[indexOfKey(selected)].depth,
      open: true,
    }
    const next = [...items]
    next.splice(at, 0, item)
    mutate(next)
    setSelected(item.key)
  }

  const indexOfKey = (key: number) => items.findIndex((i) => i.key === key)

  /** 刪除一筆時，它底下的子節點（緊接在後、depth 更深的連續區間）一起刪掉——
   *  留下沒有父節點的孤兒只會在存檔時被 nest() 提升層級，不是使用者要的。 */
  const removeSelected = () => {
    if (selected === null) return
    const at = indexOfKey(selected)
    if (at < 0) return
    let end = at + 1
    while (end < items.length && items[end].depth > items[at].depth) end++
    mutate([...items.slice(0, at), ...items.slice(end)])
    setSelected(null)
  }

  const patch = (key: number, change: Partial<FlatItem>) => {
    mutate(items.map((i) => (i.key === key ? { ...i, ...change } : i)))
  }

  /** 縮排上限是「前一筆的層級 + 1」，否則會生出跳層的孤兒節點。 */
  const indent = (key: number, delta: number) => {
    const at = indexOfKey(key)
    if (at < 0) return
    const max = at === 0 ? 0 : items[at - 1].depth + 1
    const depth = Math.max(0, Math.min(items[at].depth + delta, max))
    if (depth === items[at].depth) return
    const shift = depth - items[at].depth
    // 子節點必須跟著一起移動，否則它們會被 nest() 掛到別人底下。
    let end = at + 1
    while (end < items.length && items[end].depth > items[at].depth) end++
    mutate(
      items.map((item, i) =>
        i >= at && i < end ? { ...item, depth: Math.max(0, item.depth + shift) } : item,
      ),
    )
  }

  /** 整段（含子節點）與相鄰的同層兄弟對調。 */
  const move = (key: number, dir: -1 | 1) => {
    const at = indexOfKey(key)
    if (at < 0) return
    let end = at + 1
    while (end < items.length && items[end].depth > items[at].depth) end++
    const block = items.slice(at, end)
    const rest = [...items.slice(0, at), ...items.slice(end)]
    if (dir === -1) {
      // 往前找同層或更淺的位置：跳過前一個兄弟的整段。
      let target = at - 1
      while (target > 0 && items[target].depth > items[at].depth) target--
      if (target < 0) return
      rest.splice(target, 0, ...block)
    } else {
      if (end >= items.length) return
      let after = end + 1
      while (after < items.length && items[after].depth > items[end].depth) after++
      rest.splice(after - block.length, 0, ...block)
    }
    mutate(rest)
  }

  const save = async () => {
    if (items.some((i) => !i.title.trim())) {
      setError('書籤標題不能空白')
      return
    }
    setSaving(true)
    setError(null)
    try {
      await saveOutline(doc.id, nest(items))
      setDirty(false)
      onChanged()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="outline-panel">
      <div className="outline-header">
        <span>書籤</span>
        <button className="tb-btn" title="在目前頁面新增書籤" onClick={addHere}>
          ＋
        </button>
        <button
          className="tb-btn"
          title="刪除選取的書籤（含其子項）"
          disabled={selected === null}
          onClick={removeSelected}
        >
          🗑
        </button>
        <button className="tb-btn" title="關閉" onClick={onClose}>
          ✕
        </button>
      </div>

      {error && <div className="outline-error">{error}</div>}

      <div className="outline-list">
        {loading && <div className="outline-empty">載入中…</div>}
        {!loading && items.length === 0 && (
          <div className="outline-empty">這份文件沒有書籤。按 ＋ 從目前頁面建一個。</div>
        )}
        {items.map((item) => (
          <div
            key={item.key}
            className={`outline-row ${selected === item.key ? 'active' : ''}`}
            style={{ paddingLeft: 8 + item.depth * 14 }}
            onClick={() => setSelected(item.key)}
          >
            <input
              className="outline-title"
              value={item.title}
              placeholder="書籤標題"
              onChange={(e) => patch(item.key, { title: e.target.value })}
              onFocus={() => setSelected(item.key)}
            />
            <input
              className="outline-page"
              type="number"
              min={1}
              max={doc.pageCount}
              value={item.page + 1}
              title="目標頁"
              onChange={(e) => {
                const p = Number(e.target.value) - 1
                if (p >= 0 && p < doc.pageCount) patch(item.key, { page: p })
              }}
            />
            <button className="tb-btn" title="跳到這一頁" onClick={() => gotoPage(item.page)}>
              ↗
            </button>
          </div>
        ))}
      </div>

      {selected !== null && (
        <div className="outline-actions">
          <button className="tb-btn" title="升一層" onClick={() => indent(selected, -1)}>
            ⇤
          </button>
          <button className="tb-btn" title="降一層（成為上一筆的子項）" onClick={() => indent(selected, 1)}>
            ⇥
          </button>
          <button className="tb-btn" title="上移" onClick={() => move(selected, -1)}>
            ▲
          </button>
          <button className="tb-btn" title="下移" onClick={() => move(selected, 1)}>
            ▼
          </button>
        </div>
      )}

      <div className="outline-footer">
        <button className="tb-btn btn-primary" disabled={!dirty || saving} onClick={() => void save()}>
          {saving ? '儲存中…' : '套用書籤'}
        </button>
        <button className="tb-btn" disabled={!dirty || saving} onClick={() => void load()}>
          復原
        </button>
      </div>
    </div>
  )
}
