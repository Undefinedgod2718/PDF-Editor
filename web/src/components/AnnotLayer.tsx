import { useEffect, useRef, useState } from 'react'
import {
  createAnnotation,
  deletePageObject,
  editPageObject,
  listPageObjects,
  type CharBox,
  type Color,
  type Rect,
  type StampMeta,
  type TextObjectInfo,
} from '../api'
import { selectionToLineRects } from '../lib/annotGeom'
import type { AnnotTool } from './AnnotToolbar'

interface PxPoint {
  x: number
  y: number
}

/** 建立失敗只記 console（Phase 2 尚無全域 toast），避免 unhandled rejection。 */
async function tryCreate(fn: () => Promise<unknown>, onChanged: () => void) {
  try {
    await fn()
    onChanged()
  } catch (err) {
    console.error('createAnnotation failed:', err)
  }
}

interface Props {
  docId: string
  page: number
  scale: number
  tool: AnnotTool
  color: Color
  inkWidth: number
  stamp: StampMeta | null
  /** 該頁目前的版本號（每次註解/結構變更會 +1），用來觸發文字物件重新 fetch。 */
  version: number
  getPageChars: (page: number) => Promise<CharBox[]>
  onChanged: () => void
  flashRect: Rect | null
  flashKey: number
}

const TEXT_TOOLS: AnnotTool[] = ['highlight', 'underline', 'strikeout', 'squiggly']

/** 這些工具不使用 AnnotLayer 的拖曳/繪製互動（表單填寫、簽名皆有自己的 UI）。 */
const PASSIVE_TOOLS: AnnotTool[] = ['select', 'editText', 'editLine', 'form', 'sign']

function pxRect(a: PxPoint, b: PxPoint) {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  }
}

function rgba(c: Color, a: number) {
  return `rgba(${c.r},${c.g},${c.b},${a})`
}

/** highlight 預設半透明（避免蓋住文字），底線/刪除線/波浪線/便籤/手繪/文字框沿用選色不加透明度。 */
function colorForCreate(tool: AnnotTool, c: Color): Color {
  return tool === 'highlight' ? { ...c, a: 150 } : c
}

/** 在拖曳矩形內，依 aspect（寬/高）取最大等比尺寸，靠拖曳矩形左上角對齊。 */
function fitAspect(box: Rect, aspect: number): Rect {
  const boxAspect = box.w / box.h
  let w: number
  let h: number
  if (boxAspect > aspect) {
    h = box.h
    w = h * aspect
  } else {
    w = box.w
    h = w / aspect
  }
  return { x: box.x, y: box.y, w, h }
}

export default function AnnotLayer({
  docId,
  page,
  scale,
  tool,
  color,
  inkWidth,
  stamp,
  version,
  getPageChars,
  onChanged,
  flashRect,
  flashKey,
}: Props) {
  const overlayRef = useRef<HTMLDivElement>(null)
  const [dragStart, setDragStart] = useState<PxPoint | null>(null)
  const [dragCur, setDragCur] = useState<PxPoint | null>(null)
  const [inkPts, setInkPts] = useState<PxPoint[]>([])
  const [notePopup, setNotePopup] = useState<{ xPx: number; yPx: number; xPt: number; yPt: number } | null>(
    null,
  )
  const [noteText, setNoteText] = useState('')
  const [freeTextPopup, setFreeTextPopup] = useState<{ rectPx: Rect; rectPt: Rect } | null>(null)
  const [freeTextValue, setFreeTextValue] = useState('')
  const [freeTextSize, setFreeTextSize] = useState(14)

  const [objects, setObjects] = useState<TextObjectInfo[]>([])
  const [editPopup, setEditPopup] = useState<{ index: number; rectPx: Rect } | null>(null)
  const [editValue, setEditValue] = useState('')

  // 「編輯文字」工具啟用時（或該頁版本變動，如編輯/刪除物件後）重新抓該頁文字物件。
  useEffect(() => {
    if (tool !== 'editText') {
      setObjects([])
      return
    }
    let cancelled = false
    listPageObjects(docId, page)
      .then((res) => {
        if (!cancelled) setObjects(res)
      })
      .catch((err) => console.error('listPageObjects failed:', err))
    return () => {
      cancelled = true
    }
  }, [tool, docId, page, version])

  const isTextTool = TEXT_TOOLS.includes(tool)

  const localPoint = (e: { clientX: number; clientY: number }): PxPoint => {
    const r = overlayRef.current!.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }

  /** 來自 popup 內部（textarea、確定/取消鈕）的 pointer 事件會冒泡回 overlay，
   *  必須忽略，否則會清掉輸入狀態或用 pointer capture 劫持按鈕的 click。 */
  const fromPopup = (e: React.PointerEvent) =>
    (e.target as HTMLElement).closest('.annot-popup') !== null

  const onPointerDown = (e: React.PointerEvent) => {
    if (PASSIVE_TOOLS.includes(tool) || fromPopup(e)) return
    const p = localPoint(e)
    if (tool === 'note') {
      // Popup opens on pointer-up: opening on pointer-down lets the
      // subsequent mouseup land on the overlay and steal focus from the
      // auto-focused textarea, sending keystrokes to <body>.
      return
    }
    overlayRef.current?.setPointerCapture(e.pointerId)
    if (isTextTool || tool === 'freeText' || (tool === 'stamp' && stamp)) {
      setDragStart(p)
      setDragCur(p)
    } else if (tool === 'ink') {
      setInkPts([p])
    }
  }

  const onPointerMove = (e: React.PointerEvent) => {
    if (PASSIVE_TOOLS.includes(tool) || fromPopup(e)) return
    const p = localPoint(e)
    if ((isTextTool || tool === 'freeText' || (tool === 'stamp' && stamp)) && dragStart) {
      setDragCur(p)
    } else if (tool === 'ink' && inkPts.length > 0) {
      setInkPts((pts) => [...pts, p])
    }
  }

  const onPointerUp = async (e: React.PointerEvent) => {
    if (PASSIVE_TOOLS.includes(tool) || fromPopup(e)) return
    try {
      overlayRef.current?.releasePointerCapture(e.pointerId)
    } catch {
      /* pointer capture already released */
    }

    if (tool === 'note') {
      const p = localPoint(e)
      setNotePopup({ xPx: p.x, yPx: p.y, xPt: p.x / scale, yPt: p.y / scale })
      setNoteText('')
      return
    }

    if (tool === 'stamp' && stamp && dragStart && dragCur) {
      const selPx = pxRect(dragStart, dragCur)
      setDragStart(null)
      setDragCur(null)
      if (selPx.w < 5 || selPx.h < 5) return
      const fittedPx = fitAspect(selPx, stamp.width / stamp.height)
      const rectPt: Rect = {
        x: fittedPx.x / scale,
        y: fittedPx.y / scale,
        w: fittedPx.w / scale,
        h: fittedPx.h / scale,
      }
      await tryCreate(
        () => createAnnotation(docId, page, { type: 'stamp', rect: rectPt, stampId: stamp.id }),
        onChanged,
      )
      return
    }

    if (isTextTool && dragStart && dragCur) {
      const selPx = pxRect(dragStart, dragCur)
      setDragStart(null)
      setDragCur(null)
      if (selPx.w < 3 || selPx.h < 3) return
      const selPt: Rect = { x: selPx.x / scale, y: selPx.y / scale, w: selPx.w / scale, h: selPx.h / scale }
      const chars = await getPageChars(page)
      const rects = selectionToLineRects(chars, selPt)
      if (rects.length === 0) return
      await tryCreate(
        () =>
          createAnnotation(docId, page, {
            type: tool as 'highlight' | 'underline' | 'strikeout' | 'squiggly',
            rects,
            color: colorForCreate(tool, color),
          }),
        onChanged,
      )
      return
    }

    if (tool === 'freeText' && dragStart && dragCur) {
      const selPx = pxRect(dragStart, dragCur)
      setDragStart(null)
      setDragCur(null)
      if (selPx.w < 10 || selPx.h < 10) return
      const rectPt: Rect = { x: selPx.x / scale, y: selPx.y / scale, w: selPx.w / scale, h: selPx.h / scale }
      setFreeTextPopup({ rectPx: selPx, rectPt })
      setFreeTextValue('')
      return
    }

    if (tool === 'ink') {
      const pts = inkPts
      setInkPts([])
      if (pts.length < 2) return
      const strokePt = pts.map((p) => ({ x: p.x / scale, y: p.y / scale }))
      await tryCreate(
        () =>
          createAnnotation(docId, page, {
            type: 'ink',
            strokes: [strokePt],
            color,
            width: inkWidth,
          }),
        onChanged,
      )
    }
  }

  const submitNote = async () => {
    if (!notePopup) return
    const text = noteText.trim()
    setNotePopup(null)
    if (!text) return
    await tryCreate(
      () =>
        createAnnotation(docId, page, {
          type: 'note',
          x: notePopup.xPt,
          y: notePopup.yPt,
          contents: text,
          color,
        }),
      onChanged,
    )
  }

  const submitFreeText = async () => {
    if (!freeTextPopup) return
    const text = freeTextValue.trim()
    const popup = freeTextPopup
    setFreeTextPopup(null)
    if (!text) return
    await tryCreate(
      () =>
        createAnnotation(docId, page, {
          type: 'freeText',
          rect: popup.rectPt,
          contents: text,
          color,
          fontSize: freeTextSize,
        }),
      onChanged,
    )
  }

  const openEditPopup = (obj: TextObjectInfo, e: React.MouseEvent) => {
    e.stopPropagation()
    setEditPopup({
      index: obj.index,
      rectPx: { x: obj.x * scale, y: obj.y * scale, w: obj.w * scale, h: obj.h * scale },
    })
    setEditValue(obj.text)
  }

  const submitEditText = async () => {
    if (!editPopup) return
    const index = editPopup.index
    const text = editValue
    setEditPopup(null)
    try {
      await editPageObject(docId, page, index, text)
      onChanged()
    } catch (err) {
      console.error('editPageObject failed:', err)
    }
  }

  const deleteEditText = async () => {
    if (!editPopup) return
    const index = editPopup.index
    setEditPopup(null)
    try {
      await deletePageObject(docId, page, index)
      onChanged()
    } catch (err) {
      console.error('deletePageObject failed:', err)
    }
  }

  const dragPreviewPx = dragStart && dragCur ? pxRect(dragStart, dragCur) : null
  const stampPreviewPx =
    tool === 'stamp' && stamp && dragPreviewPx ? fitAspect(dragPreviewPx, stamp.width / stamp.height) : null

  return (
    <div
      ref={overlayRef}
      className={`annot-layer ${!PASSIVE_TOOLS.includes(tool) ? 'annot-layer-active' : ''}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={(e) => void onPointerUp(e)}
    >
      {dragPreviewPx && (isTextTool || tool === 'freeText') && (
        <div
          className="annot-drag-preview"
          style={{
            left: dragPreviewPx.x,
            top: dragPreviewPx.y,
            width: dragPreviewPx.w,
            height: dragPreviewPx.h,
            background: isTextTool ? rgba(color, 0.35) : 'transparent',
            borderColor: rgba(color, 0.9),
          }}
        />
      )}

      {stampPreviewPx && (
        <div
          className="annot-drag-preview"
          style={{
            left: stampPreviewPx.x,
            top: stampPreviewPx.y,
            width: stampPreviewPx.w,
            height: stampPreviewPx.h,
            borderColor: 'rgba(76,141,255,0.9)',
          }}
        />
      )}

      {tool === 'editText' &&
        objects.map((o) => (
          <div
            key={o.index}
            className="text-obj-box"
            style={{ left: o.x * scale, top: o.y * scale, width: o.w * scale, height: o.h * scale }}
            title={o.text}
            onClick={(e) => openEditPopup(o, e)}
          />
        ))}

      {inkPts.length > 1 && (
        <svg className="annot-ink-svg">
          <polyline
            points={inkPts.map((p) => `${p.x},${p.y}`).join(' ')}
            fill="none"
            stroke={rgba(color, 0.85)}
            strokeWidth={inkWidth * scale}
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      )}

      {notePopup && (
        <div className="annot-popup" style={{ left: notePopup.xPx, top: notePopup.yPx }}>
          <textarea
            autoFocus
            placeholder="輸入便籤內容…"
            value={noteText}
            onChange={(e) => setNoteText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') setNotePopup(null)
            }}
          />
          <div className="annot-popup-actions">
            <button className="tb-btn" onClick={() => void submitNote()}>
              確定
            </button>
            <button className="tb-btn" onClick={() => setNotePopup(null)}>
              取消
            </button>
          </div>
        </div>
      )}

      {freeTextPopup && (
        <div
          className="annot-popup"
          style={{ left: freeTextPopup.rectPx.x, top: freeTextPopup.rectPx.y + freeTextPopup.rectPx.h + 4 }}
        >
          <textarea
            autoFocus
            placeholder="輸入文字…"
            value={freeTextValue}
            onChange={(e) => setFreeTextValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') setFreeTextPopup(null)
            }}
          />
          <div className="annot-popup-actions">
            <select value={freeTextSize} onChange={(e) => setFreeTextSize(Number(e.target.value))}>
              <option value={12}>12pt</option>
              <option value={14}>14pt</option>
              <option value={18}>18pt</option>
              <option value={24}>24pt</option>
            </select>
            <button className="tb-btn" onClick={() => void submitFreeText()}>
              確定
            </button>
            <button className="tb-btn" onClick={() => setFreeTextPopup(null)}>
              取消
            </button>
          </div>
        </div>
      )}

      {editPopup && (
        <div
          className="annot-popup"
          style={{ left: editPopup.rectPx.x, top: editPopup.rectPx.y + editPopup.rectPx.h + 4 }}
        >
          <textarea
            autoFocus
            placeholder="輸入文字…"
            value={editValue}
            onChange={(e) => setEditValue(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') setEditPopup(null)
            }}
          />
          <div className="annot-popup-actions">
            <button className="tb-btn" onClick={() => void submitEditText()}>
              確定
            </button>
            <button className="tb-btn" onClick={() => void deleteEditText()}>
              刪除
            </button>
            <button className="tb-btn" onClick={() => setEditPopup(null)}>
              取消
            </button>
          </div>
        </div>
      )}

      {flashRect && (
        <div
          key={flashKey}
          className="annot-flash"
          style={{
            left: flashRect.x * scale,
            top: flashRect.y * scale,
            width: flashRect.w * scale,
            height: flashRect.h * scale,
          }}
        />
      )}
    </div>
  )
}
