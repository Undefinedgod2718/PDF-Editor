import { useEffect, useRef, useState } from 'react'
import {
  createAnnotation,
  deletePageObject,
  editPageObject,
  listAnnotations,
  listPageObjects,
  updateAnnotation,
  type AnnotationInfo,
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

/** 這些工具不使用 AnnotLayer 的拖曳/繪製互動（表單填寫、簽名皆有自己的 UI）。
 *  'select' 仍在其中——它不走這裡的「拖曳畫新東西」路徑，但下面另外處理它
 *  「拖曳既有註解」的路徑，兩者是獨立的。 */
const PASSIVE_TOOLS: AnnotTool[] = ['select', 'editText', 'editLine', 'form', 'sign']

/** 後端 annots::set_rect 只接受這三種類型（見該函式註解）；其餘一律回 400，
 *  所以這裡也只對這三種畫控制點。字串要跟 pdfium-render 的 enum Debug 名稱
 *  完全一致（含大小寫），跟 set_color 用的 lopdf /Subtype 名稱是兩套命名，
 *  不要混用——`Strikeout` 是這一套裡的拼法。 */
const MOVABLE_KINDS = ['Ink', 'Stamp', 'Text']

type Corner = 'nw' | 'ne' | 'sw' | 'se'
const CORNERS: Corner[] = ['nw', 'ne', 'sw', 'se']

/** 縮放後最小寬高（points），避免拖成負值/過小，跟 FormBuilderLayer 的
 *  MIN_RESIZE_PT 同一慣例。 */
const MIN_ANNOT_PT = 8

/** 以某個角為錨點縮放：該角的兩個邊移動，對角固定不動。左/上邊會位移時
 *  夾住 dx/dy，讓寬高不會被拖出 MIN_ANNOT_PT 之外（而不是縮到負值再被
 *  Math.max 拉回，那樣位置會跳一下）。 */
function resizeRect(orig: Rect, corner: Corner, dx: number, dy: number): Rect {
  let x = orig.x
  let y = orig.y
  let w = orig.w
  let h = orig.h
  if (corner === 'nw' || corner === 'sw') {
    const clampedDx = Math.min(dx, orig.w - MIN_ANNOT_PT)
    x = orig.x + clampedDx
    w = orig.w - clampedDx
  } else {
    w = Math.max(MIN_ANNOT_PT, orig.w + dx)
  }
  if (corner === 'nw' || corner === 'ne') {
    const clampedDy = Math.min(dy, orig.h - MIN_ANNOT_PT)
    y = orig.y + clampedDy
    h = orig.h - clampedDy
  } else {
    h = Math.max(MIN_ANNOT_PT, orig.h + dy)
  }
  return { x, y, w, h }
}

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

  // ---- 「選取」工具：拖曳既有註解移動/縮放 ----
  const [selectAnnots, setSelectAnnots] = useState<AnnotationInfo[]>([])
  const [selectedNm, setSelectedNm] = useState<string | null>(null)
  const [dragMode, setDragMode] = useState<'move' | Corner | null>(null)
  const [dragStartPx, setDragStartPx] = useState<PxPoint | null>(null)
  const [dragCurPx, setDragCurPx] = useState<PxPoint | null>(null)
  const dragOrigRectRef = useRef<Rect | null>(null)
  const movedRef = useRef(false)
  const [dragError, setDragError] = useState<string | null>(null)

  // 「選取」工具啟用時（或該頁版本變動，如其他地方編輯/刪除了註解）重新抓該頁註解清單。
  useEffect(() => {
    if (tool !== 'select') {
      setSelectAnnots([])
      return
    }
    let cancelled = false
    listAnnotations(docId, page)
      .then((res) => {
        if (!cancelled) setSelectAnnots(res)
      })
      .catch((err) => console.error('listAnnotations failed:', err))
    return () => {
      cancelled = true
    }
  }, [tool, docId, page, version])

  // 離開選取工具時清掉選取/拖曳/錯誤狀態，避免切工具後殘留一個看不到的選取框。
  useEffect(() => {
    if (tool !== 'select') {
      setSelectedNm(null)
      setDragMode(null)
      setDragStartPx(null)
      setDragCurPx(null)
      dragOrigRectRef.current = null
      setDragError(null)
    }
  }, [tool])

  // 清單重新抓回來後，若目前選取的 nm 已不在其中（被刪除/類型不再可拖），清掉選取。
  useEffect(() => {
    if (selectedNm !== null && !selectAnnots.some((a) => a.nm === selectedNm)) {
      setSelectedNm(null)
    }
  }, [selectAnnots, selectedNm])

  /** 只有 Ink/Stamp/Text 且有穩定 /NM 的才可拖曳——沒有 /NM 的舊註解，
   *  後端 set_rect 沒有 index 退路（見 pdf-core find 邏輯），給了控制點也會
   *  一路 400，不如不給。回覆（irt != null）是面板專用的 Text 註解，
   *  不該在頁面上拖出把手。 */
  const movableAnnots = selectAnnots.filter(
    (a): a is AnnotationInfo & { nm: string; rect: Rect } =>
      a.nm !== null &&
      a.rect !== null &&
      a.irt === null &&
      MOVABLE_KINDS.includes(a.type),
  )

  const startAnnotDrag = (e: React.PointerEvent, nm: string, rect: Rect, mode: 'move' | Corner) => {
    e.stopPropagation()
    overlayRef.current?.setPointerCapture(e.pointerId)
    setSelectedNm(nm)
    setDragMode(mode)
    const p = localPoint(e)
    setDragStartPx(p)
    setDragCurPx(p)
    dragOrigRectRef.current = rect
    movedRef.current = false
    setDragError(null)
  }

  /** 拖曳中的即時預覽 rect；沒在拖就是伺服器原本的 rect。 */
  const displayRect = (a: AnnotationInfo & { rect: Rect; nm: string }): Rect => {
    if (dragMode && selectedNm === a.nm && dragStartPx && dragCurPx && dragOrigRectRef.current) {
      const orig = dragOrigRectRef.current
      const dx = (dragCurPx.x - dragStartPx.x) / scale
      const dy = (dragCurPx.y - dragStartPx.y) / scale
      return dragMode === 'move'
        ? { x: orig.x + dx, y: orig.y + dy, w: orig.w, h: orig.h }
        : resizeRect(orig, dragMode, dx, dy)
    }
    return a.rect
  }

  // 失敗時的還原靠一個隱性前提：拖曳過程中**沒有**樂觀改動 selectAnnots，畫面上的
  // 位移純粹來自 drag state，所以清掉 drag state 就自動退回伺服器上那份 rect。
  // 若日後有人為了效能改成邊拖邊改 selectAnnots，這裡的還原會靜默失效——那時要改成
  // 重新 listAnnotations，別只是把 drag state 清掉。
  const finishAnnotDrag = async () => {
    const nm = selectedNm
    const orig = dragOrigRectRef.current
    const start = dragStartPx
    const cur = dragCurPx
    const mode = dragMode
    const wasMoved = movedRef.current

    setDragMode(null)
    setDragStartPx(null)
    setDragCurPx(null)
    dragOrigRectRef.current = null
    movedRef.current = false

    if (!mode || !orig || !start || !cur || !nm || !wasMoved) return // 純點擊：只選取，不送 PATCH

    const dx = (cur.x - start.x) / scale
    const dy = (cur.y - start.y) / scale
    const nextRect: Rect = mode === 'move' ? { x: orig.x + dx, y: orig.y + dy, w: orig.w, h: orig.h } : resizeRect(orig, mode, dx, dy)

    try {
      await updateAnnotation(docId, page, nm, { rect: nextRect })
      onChanged()
    } catch (err) {
      // 沒有另外做「樂觀更新」——selectAnnots 裡的 rect 全程沒被拖曳改過，
      // 上面清掉 dragMode 等狀態後畫面自然掉回 a.rect（伺服器原值），
      // 不會停在一個檔案裡其實沒有的位置。
      console.error('updateAnnotation (rect) failed:', err)
      setDragError(err instanceof Error ? err.message : String(err))
    }
  }

  // Escape：先取消進行中的拖曳，其次清掉選取；兩者之一發生就 preventDefault，
  // 讓 DocumentWorkspace 的 Escape 分支（工具退回/退出全螢幕）看到
  // e.defaultPrevented 而提早 return。
  //
  // 必須用 capture：React effect 是父先子後 mount，bubble 監聽會讓 DocumentWorkspace
  // 先跑——拖曳中按 Esc 就會先退全螢幕，再才取消拖曳（checklist A4／C3-42）。
  // capture 把本 handler 放到 DocumentWorkspace 的 bubble listener 前面。
  useEffect(() => {
    if (tool !== 'select') return
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      if (dragMode) {
        setDragMode(null)
        setDragStartPx(null)
        setDragCurPx(null)
        dragOrigRectRef.current = null
        movedRef.current = false
        e.preventDefault()
        return
      }
      if (selectedNm !== null) {
        setSelectedNm(null)
        e.preventDefault()
      }
    }
    window.addEventListener('keydown', onKeyDown, true)
    return () => window.removeEventListener('keydown', onKeyDown, true)
  }, [tool, dragMode, selectedNm])

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
    if (tool === 'select') {
      // 空白處點擊（沒點到任何 annot-select-box，它們自己的 handler 會
      // stopPropagation）：清掉選取。真正開始拖曳走的是各框自己的
      // onPointerDown（見下方 startAnnotDrag），不經過這裡。
      if (e.target === overlayRef.current) setSelectedNm(null)
      return
    }
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
    if (tool === 'select') {
      if (dragMode) {
        setDragCurPx(localPoint(e))
        movedRef.current = true
      }
      return
    }
    if (PASSIVE_TOOLS.includes(tool) || fromPopup(e)) return
    const p = localPoint(e)
    if ((isTextTool || tool === 'freeText' || (tool === 'stamp' && stamp)) && dragStart) {
      setDragCur(p)
    } else if (tool === 'ink' && inkPts.length > 0) {
      setInkPts((pts) => [...pts, p])
    }
  }

  const onPointerUp = async (e: React.PointerEvent) => {
    if (tool === 'select') {
      try {
        overlayRef.current?.releasePointerCapture(e.pointerId)
      } catch {
        /* pointer capture already released */
      }
      if (dragMode) await finishAnnotDrag()
      return
    }
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
      className={`annot-layer ${!PASSIVE_TOOLS.includes(tool) ? 'annot-layer-active' : ''} ${tool === 'select' ? 'annot-layer-select' : ''}`}
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

      {tool === 'select' &&
        movableAnnots.map((a) => {
          const rect = displayRect(a)
          const isSelected = a.nm === selectedNm
          return (
            <div
              key={a.nm}
              className={`annot-select-box ${isSelected ? 'selected' : ''}`}
              style={{ left: rect.x * scale, top: rect.y * scale, width: rect.w * scale, height: rect.h * scale }}
              onPointerDown={(e) => startAnnotDrag(e, a.nm, a.rect, 'move')}
            >
              {isSelected &&
                CORNERS.map((c) => (
                  <div
                    key={c}
                    className={`annot-select-handle annot-select-handle-${c}`}
                    onPointerDown={(e) => startAnnotDrag(e, a.nm, a.rect, c)}
                  />
                ))}
            </div>
          )
        })}

      {tool === 'select' && dragError && (
        <div className="annot-drag-error">
          <span>{dragError}</span>
          <button className="tb-btn" onClick={() => setDragError(null)}>
            ✕
          </button>
        </div>
      )}

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
              // preventDefault：見 SearchPanel 同名處的說明——關掉自己的 popup 後不下
              // 這行，同一下 Escape 會繼續冒泡到 DocumentWorkspace 而退出全螢幕。
              if (e.key === 'Escape') {
                e.preventDefault()
                setNotePopup(null)
              }
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
              if (e.key === 'Escape') {
                e.preventDefault()
                setFreeTextPopup(null)
              }
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
              if (e.key === 'Escape') {
                e.preventDefault()
                setEditPopup(null)
              }
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
