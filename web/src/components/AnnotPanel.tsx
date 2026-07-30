import { useEffect, useMemo, useState } from 'react'
import {
  deleteAnnotation,
  listAnnotations,
  replyToAnnotation,
  updateAnnotation,
  type AnnotationInfo,
  type Color,
  type DocInfo,
  type Rect,
} from '../api'
import { PALETTE, colorToHex, hexToColor } from './AnnotToolbar'

interface Props {
  doc: DocInfo
  currentPage: number
  version: number
  /** 刪除／編輯／回覆都會改到頁面內容，一律叫這個讓父層 bump 該頁的 render version。 */
  onChanged: (page: number) => void
  onSelect: (page: number, rect: Rect) => void
  onClose: () => void
}

// key 一律是 pdfium-render `PdfPageAnnotationType` 的 Debug 命名（清單 API 實際
// 帶的字串），不是 PDF 規範拼法。這裡原本寫 "StrikeOut"（規範拼法），結果刪除線
// 永遠對不到、直接顯示原始字串 "Strikeout"——跟 RECOLORABLE_TYPES 那則註解講的
// 是同一個陷阱，只是踩在不同地方。
const TYPE_LABEL: Record<string, string> = {
  Highlight: '螢光標記',
  Underline: '底線',
  Strikeout: '刪除線',
  Squiggly: '波浪線',
  Text: '便籤',
  Note: '便籤',
  Ink: '手繪',
  FreeText: '文字框',
  // 後端以 PDFium 的 subtype 命名回傳，文字框註解實際落在 "Stamp"（見 wiki 補充說明）
  Stamp: '文字框',
}

// 這個 app 沒有登入機制，作者名字純粹存在本機瀏覽器，跨份文件共用一個欄位。
const AUTHOR_STORAGE_KEY = 'pdf-editor:annotAuthor'

/**
 * 可換色的類型——必須用 `it.type` 實際帶的字串比對，也就是 pdfium-render
 * `PdfPageAnnotationType` 的 Debug 命名（"Strikeout"，小寫 o），不是後端
 * `annots::set_color` 內部比對的 PDF 規範拼法（"StrikeOut"，大寫 O）。用錯
 * 拼法不會報錯，只會讓刪除線的換色按鈕悄悄地永遠是 disabled。
 */
const RECOLORABLE_TYPES = new Set(['Highlight', 'Underline', 'Strikeout', 'Squiggly'])

/** 換色按鈕 disabled 時的說明；非標記類的外觀是「畫出來」而非由顏色欄位生成
 *  （見 `annots::set_color` 開頭註解），換色在這一版一律不支援。 */
function colorDisabledReason(type: string): string | null {
  if (RECOLORABLE_TYPES.has(type)) return null
  if (type === 'Ink') return '手繪筆畫的顏色已烘進外觀圖層，無法在此換色'
  if (type === 'Stamp') return '文字框／印章的顏色已烘進外觀圖層，無法在此換色'
  if (type === 'Text' || type === 'Note') return '便籤／回覆沒有可重新生成的外觀圖層，無法在此換色'
  return '此類型目前不支援換色'
}

/** 讀上次填的作者名。localStorage 不可用（無痕模式關閉／權限問題）時退回空字串，
 *  當作沒設定——呼叫端會因此在送出時省略 author 欄位，而不是送一個空字串。 */
function loadStoredAuthor(): string {
  try {
    return localStorage.getItem(AUTHOR_STORAGE_KEY) ?? ''
  } catch {
    return ''
  }
}

function storeAuthor(name: string): void {
  try {
    localStorage.setItem(AUTHOR_STORAGE_KEY, name)
  } catch {
    // 存不進去不影響當次使用，只是下次不會記得。
  }
}

export default function AnnotPanel({ doc, currentPage, version, onChanged, onSelect, onClose }: Props) {
  const [items, setItems] = useState<AnnotationInfo[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [author, setAuthor] = useState<string>(() => loadStoredAuthor())

  // 同時只會有一個編輯框／回覆框／換色框開著（用 nm 當 key）；null 表示都沒開。
  const [editingNm, setEditingNm] = useState<string | null>(null)
  const [editText, setEditText] = useState('')
  const [replyingNm, setReplyingNm] = useState<string | null>(null)
  const [replyText, setReplyText] = useState('')
  const [recoloringNm, setRecoloringNm] = useState<string | null>(null)
  // 換色請求進行中時鎖住色票，防止同一個請求被連點兩次疊發。
  const [savingColor, setSavingColor] = useState(false)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    listAnnotations(doc.id, currentPage)
      .then((res) => {
        if (!cancelled) setItems(res)
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [doc.id, currentPage, version])

  // 切頁時任何殘留的編輯／回覆／換色框、錯誤訊息都跟舊頁的清單綁在一起，沒意義了。
  useEffect(() => {
    setEditingNm(null)
    setReplyingNm(null)
    setRecoloringNm(null)
    setError(null)
  }, [currentPage])

  // 依 irt 建父 -> 子清單；串可以深過兩層（後端測過三層），所以渲染時遞迴查表，
  // 不假設只有「頂層/回覆」兩級。
  const childrenByParent = useMemo(() => {
    const map = new Map<string, AnnotationInfo[]>()
    for (const it of items) {
      if (!it.irt) continue
      const list = map.get(it.irt)
      if (list) list.push(it)
      else map.set(it.irt, [it])
    }
    return map
  }, [items])

  const topLevel = items.filter((it) => it.irt === null)

  // 後端刪除父節點時會連同底下所有回覆一起遞移刪除，這裡算出總數讓確認訊息說清楚。
  const countDescendants = (nm: string): number => {
    const kids = childrenByParent.get(nm) ?? []
    let n = kids.length
    for (const k of kids) {
      if (k.nm) n += countDescendants(k.nm)
    }
    return n
  }

  const changeAuthor = (name: string) => {
    setAuthor(name)
    storeAuthor(name)
  }

  const remove = async (it: AnnotationInfo) => {
    const descendants = it.nm ? countDescendants(it.nm) : 0
    const msg =
      descendants > 0
        ? `確定要刪除這則留言？其下 ${descendants} 則回覆也會一併刪除。`
        : '確定要刪除這則留言？'
    if (!window.confirm(msg)) return
    try {
      // nm 是穩定 ID；舊註解（導入 /NM 前建立）才退回 index。
      await deleteAnnotation(doc.id, currentPage, it.nm ?? String(it.index))
      setError(null)
      onChanged(currentPage)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const startEdit = (it: AnnotationInfo) => {
    if (!it.nm) return
    setReplyingNm(null)
    setRecoloringNm(null)
    setEditingNm(it.nm)
    setEditText(it.contents ?? '')
  }

  const cancelEdit = () => {
    setEditingNm(null)
    setEditText('')
  }

  const saveEdit = async (it: AnnotationInfo) => {
    if (!it.nm) return
    try {
      await updateAnnotation(doc.id, currentPage, it.nm, {
        contents: editText,
        author: author.trim() || undefined,
      })
      setError(null)
      setEditingNm(null)
      setEditText('')
      onChanged(currentPage)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const startReply = (it: AnnotationInfo) => {
    if (!it.nm) return
    setEditingNm(null)
    setRecoloringNm(null)
    setReplyingNm(it.nm)
    setReplyText('')
  }

  const cancelReply = () => {
    setReplyingNm(null)
    setReplyText('')
  }

  const startRecolor = (it: AnnotationInfo) => {
    if (!it.nm) return
    setEditingNm(null)
    setReplyingNm(null)
    setRecoloringNm(it.nm)
  }

  const cancelRecolor = () => {
    setRecoloringNm(null)
  }

  // 換色沒有草稿概念：選了色票就立刻送出，跟工具列的色票同一套互動。
  // savingColor guard 住整個 await 區間，防止使用者連點兩個色票疊發兩個請求。
  const applyColor = async (it: AnnotationInfo, color: Color) => {
    if (!it.nm || savingColor) return
    setSavingColor(true)
    try {
      await updateAnnotation(doc.id, currentPage, it.nm, { color })
      setError(null)
      setRecoloringNm(null)
      onChanged(currentPage)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSavingColor(false)
    }
  }

  const sendReply = async (it: AnnotationInfo) => {
    if (!it.nm || !replyText.trim()) return
    try {
      await replyToAnnotation(doc.id, currentPage, it.nm, {
        contents: replyText,
        author: author.trim() || undefined,
      })
      setError(null)
      setReplyingNm(null)
      setReplyText('')
      onChanged(currentPage)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const renderNode = (it: AnnotationInfo, depth: number) => {
    const key = it.nm ?? `idx-${it.index}`
    const kids = it.nm ? childrenByParent.get(it.nm) ?? [] : []
    const isReply = it.irt !== null
    const isStamp = it.type === 'Stamp'
    const isEditing = editingNm !== null && editingNm === it.nm
    const isReplying = replyingNm !== null && replyingNm === it.nm
    const isRecoloring = recoloringNm !== null && recoloringNm === it.nm
    const colorDisabled = colorDisabledReason(it.type)

    return (
      <div key={key} className="annot-item-wrap" style={{ marginLeft: depth * 16 }}>
        <div className="annot-item" onClick={() => it.rect && onSelect(currentPage, it.rect)}>
          <div className="annot-item-main">
            {/* 回覆在 PDF 裡是 Text（便籤）subtype，那是 /IRT + /RT 這種回覆結構的
                載體，不是使用者選了便籤工具。照 subtype 標成「便籤」會讓整串回覆
                看起來像一堆莫名其妙的便籤，所以有父節點就標「回覆」。 */}
            <span className="annot-item-type">
              {it.irt ? '回覆' : (TYPE_LABEL[it.type] ?? it.type)}
            </span>
            {it.author && <span className="annot-item-author">{it.author}</span>}
            {it.contents && <span className="annot-item-contents">{it.contents}</span>}
            {/* 回覆的 rect 只是複製父節點的，對使用者毫無意義，不顯示。 */}
            {!isReply && it.rect && (
              <span className="annot-item-rect">
                x:{Math.round(it.rect.x)} y:{Math.round(it.rect.y)} w:{Math.round(it.rect.w)} h:
                {Math.round(it.rect.h)}
              </span>
            )}
          </div>
          <div className="annot-item-actions" onClick={(e) => e.stopPropagation()}>
            <button
              className="tb-btn"
              title={isStamp ? '文字框內容已烘進外觀圖層，無法在此編輯' : '編輯'}
              disabled={!it.nm || isStamp}
              onClick={() => startEdit(it)}
            >
              編輯
            </button>
            <button className="tb-btn" title="回覆" disabled={!it.nm} onClick={() => startReply(it)}>
              回覆
            </button>
            <button
              className="tb-btn"
              title={colorDisabled ?? '換色'}
              disabled={!it.nm || colorDisabled !== null}
              onClick={() => (isRecoloring ? cancelRecolor() : startRecolor(it))}
            >
              🎨
            </button>
            <button
              className="tb-btn annot-item-del"
              title="刪除"
              onClick={() => {
                void remove(it)
              }}
            >
              🗑
            </button>
          </div>
        </div>
        {isEditing && (
          <div className="annot-edit-box" onClick={(e) => e.stopPropagation()}>
            <textarea value={editText} onChange={(e) => setEditText(e.target.value)} rows={3} />
            <div className="annot-edit-actions">
              <button
                className="tb-btn"
                onClick={() => {
                  void saveEdit(it)
                }}
              >
                儲存
              </button>
              <button className="tb-btn" onClick={cancelEdit}>
                取消
              </button>
            </div>
          </div>
        )}
        {isReplying && (
          <div className="annot-edit-box" onClick={(e) => e.stopPropagation()}>
            <textarea
              value={replyText}
              onChange={(e) => setReplyText(e.target.value)}
              rows={2}
              placeholder="輸入回覆內容…"
            />
            <div className="annot-edit-actions">
              <button
                className="tb-btn"
                disabled={!replyText.trim()}
                onClick={() => {
                  void sendReply(it)
                }}
              >
                送出
              </button>
              <button className="tb-btn" onClick={cancelReply}>
                取消
              </button>
            </div>
          </div>
        )}
        {isRecoloring && (
          <div className="annot-edit-box" onClick={(e) => e.stopPropagation()}>
            {/* 清單沒有目前顏色欄位，色票一律不預選——選了就是新顏色，不是「目前顏色」。 */}
            <div className="toolbar-group annot-palette">
              {PALETTE.map((c) => (
                <button
                  key={colorToHex(c)}
                  className="swatch"
                  style={{ background: `rgb(${c.r},${c.g},${c.b})` }}
                  title={colorToHex(c)}
                  disabled={savingColor}
                  onClick={() => {
                    void applyColor(it, c)
                  }}
                />
              ))}
              <input
                type="color"
                className="swatch-picker"
                title="自訂顏色"
                disabled={savingColor}
                onChange={(e) => {
                  void applyColor(it, hexToColor(e.target.value))
                }}
              />
            </div>
            <div className="annot-edit-actions">
              <button className="tb-btn" onClick={cancelRecolor}>
                取消
              </button>
            </div>
          </div>
        )}
        {kids.map((k) => renderNode(k, depth + 1))}
      </div>
    )
  }

  return (
    <div className="annot-panel">
      <div className="search-header">
        <span className="annot-panel-title">第 {currentPage + 1} 頁註解</span>
        <button className="tb-btn" title="關閉" onClick={onClose}>
          ✕
        </button>
      </div>
      <div className="annot-author-row">
        <label htmlFor="annot-author-input">作者</label>
        <input
          id="annot-author-input"
          type="text"
          value={author}
          placeholder="（未設定）"
          onChange={(e) => changeAuthor(e.target.value)}
        />
      </div>
      {error && (
        <div className="annot-error">
          <span>{error}</span>
          <button className="tb-btn" title="關閉" onClick={() => setError(null)}>
            ✕
          </button>
        </div>
      )}
      <div className="search-results">
        {loading && <div className="annot-empty">載入中…</div>}
        {!loading && topLevel.length === 0 && <div className="annot-empty">此頁尚無註解</div>}
        {topLevel.map((it) => renderNode(it, 0))}
      </div>
    </div>
  )
}
