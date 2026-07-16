import { useEffect, useRef, useState } from 'react'
import { deleteFormField, updateFormField, type FormField, type Rect } from '../api'
import type { BuilderFieldType } from './FormBuilderBar'

interface PxPoint {
  x: number
  y: number
}

interface Props {
  docId: string
  page: number
  /** 目前頁面渲染 scale（CSS px / pt），與 Viewer 傳給其他 layer 的 scale 一致。 */
  scale: number
  /** 已預先過濾成只屬於這一頁、且有 rect 的欄位。 */
  fields: FormField[]
  /** 目前選取的欄位型別（FormBuilderBar），僅影響新拖曳出的欄位要建立成什麼型別。 */
  selectedType: BuilderFieldType
  /** 拖曳畫出新欄位範圍後回呼（view-space points），由上層開啟 FieldDialog（create 模式）。 */
  onCreateRect: (rectPt: Rect) => void
  /** PATCH/DELETE 成功後呼叫，通知上層重新抓表單欄位＋bump 該頁渲染版本。 */
  onFieldsChanged: () => void
  /** 雙擊既有欄位框，由上層開啟 FieldDialog（edit 模式）。 */
  onEditField: (field: FormField) => void
}

/** 拖曳矩形雙軸皆小於這個像素門檻視為誤觸，忽略（不建立欄位）。 */
const MIN_PX = 8
/** 縮放結果最小尺寸（points），避免拖成負值/過小。 */
const MIN_RESIZE_PT = 8

const TYPE_LABELS: Record<string, string> = {
  Text: '文字',
  Checkbox: '核取方塊',
  RadioButton: '單選',
  ComboBox: '下拉選單',
  ListBox: '清單方塊',
  Signature: '簽名',
}

function pxRect(a: PxPoint, b: PxPoint): Rect {
  return {
    x: Math.min(a.x, b.x),
    y: Math.min(a.y, b.y),
    w: Math.abs(a.x - b.x),
    h: Math.abs(a.y - b.y),
  }
}

export default function FormBuilderLayer({
  docId,
  page,
  scale,
  fields,
  onCreateRect,
  onFieldsChanged,
  onEditField,
}: Props) {
  const overlayRef = useRef<HTMLDivElement>(null)

  // 空白區域拖曳畫新欄位範圍。
  const [draftStart, setDraftStart] = useState<PxPoint | null>(null)
  const [draftCur, setDraftCur] = useState<PxPoint | null>(null)

  // 既有欄位選取／移動／縮放。
  const [selectedIndex, setSelectedIndex] = useState<number | null>(null)
  const [dragMode, setDragMode] = useState<'move' | 'resize' | null>(null)
  const [dragStartPx, setDragStartPx] = useState<PxPoint | null>(null)
  const [dragCurPx, setDragCurPx] = useState<PxPoint | null>(null)
  const dragOrigRectRef = useRef<Rect | null>(null)
  const movedRef = useRef(false)

  // 欄位集合變動（換頁／增刪）後，若目前選取的 index 已不存在則清除選取。
  useEffect(() => {
    if (selectedIndex !== null && !fields.some((f) => f.index === selectedIndex)) {
      setSelectedIndex(null)
    }
  }, [fields, selectedIndex])

  // Delete/Backspace 刪除目前選取欄位；僅在有選取時掛上監聽，並避免打字輸入時誤觸。
  useEffect(() => {
    if (selectedIndex === null) return
    const onKeyDown = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null
      if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) return
      if (e.key !== 'Delete' && e.key !== 'Backspace') return
      e.preventDefault()
      const idx = selectedIndex
      setSelectedIndex(null)
      deleteFormField(docId, page, idx)
        .then(() => onFieldsChanged())
        .catch((err) => console.error('deleteFormField failed:', err))
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [selectedIndex, docId, page, onFieldsChanged])

  const localPoint = (e: { clientX: number; clientY: number }): PxPoint => {
    const r = overlayRef.current!.getBoundingClientRect()
    return { x: e.clientX - r.left, y: e.clientY - r.top }
  }

  const onOverlayPointerDown = (e: React.PointerEvent) => {
    if (e.target !== overlayRef.current) return // 點在欄位框/把手上，交給各自的 handler 處理
    overlayRef.current?.setPointerCapture(e.pointerId)
    setSelectedIndex(null)
    const p = localPoint(e)
    setDraftStart(p)
    setDraftCur(p)
  }

  const onOverlayPointerMove = (e: React.PointerEvent) => {
    if (draftStart) {
      setDraftCur(localPoint(e))
      return
    }
    if (dragMode && dragStartPx) {
      setDragCurPx(localPoint(e))
      movedRef.current = true
    }
  }

  const finishDraft = () => {
    if (!draftStart || !draftCur) return
    const rectPx = pxRect(draftStart, draftCur)
    setDraftStart(null)
    setDraftCur(null)
    if (rectPx.w < MIN_PX || rectPx.h < MIN_PX) return
    onCreateRect({
      x: rectPx.x / scale,
      y: rectPx.y / scale,
      w: rectPx.w / scale,
      h: rectPx.h / scale,
    })
  }

  const finishDrag = () => {
    const idx = selectedIndex
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

    if (!mode || !orig || !start || !cur || idx === null || !wasMoved) return // 純點擊：只選取，不送 PATCH

    const dx = (cur.x - start.x) / scale
    const dy = (cur.y - start.y) / scale
    const nextRect: Rect =
      mode === 'move'
        ? { x: orig.x + dx, y: orig.y + dy, w: orig.w, h: orig.h }
        : { x: orig.x, y: orig.y, w: Math.max(MIN_RESIZE_PT, orig.w + dx), h: Math.max(MIN_RESIZE_PT, orig.h + dy) }

    updateFormField(docId, page, idx, { rect: nextRect })
      .then(() => onFieldsChanged())
      .catch((err) => console.error('updateFormField (rect) failed:', err))
  }

  const onOverlayPointerUp = (e: React.PointerEvent) => {
    try {
      overlayRef.current?.releasePointerCapture(e.pointerId)
    } catch {
      /* pointer capture already released */
    }
    if (draftStart) {
      finishDraft()
      return
    }
    if (dragMode) finishDrag()
  }

  const startBoxDrag = (e: React.PointerEvent, field: FormField, mode: 'move' | 'resize') => {
    e.stopPropagation()
    if (!field.rect) return
    overlayRef.current?.setPointerCapture(e.pointerId)
    setSelectedIndex(field.index)
    setDragMode(mode)
    const p = localPoint(e)
    setDragStartPx(p)
    setDragCurPx(p)
    dragOrigRectRef.current = field.rect
    movedRef.current = false
  }

  const draftPx = draftStart && draftCur ? pxRect(draftStart, draftCur) : null

  return (
    <div
      ref={overlayRef}
      className="fb-layer"
      onPointerDown={onOverlayPointerDown}
      onPointerMove={onOverlayPointerMove}
      onPointerUp={onOverlayPointerUp}
    >
      {fields.map((field) => {
        if (!field.rect) return null
        const isSelected = field.index === selectedIndex

        let rect = field.rect
        if (isSelected && dragMode && dragStartPx && dragCurPx && dragOrigRectRef.current) {
          const orig = dragOrigRectRef.current
          const dx = (dragCurPx.x - dragStartPx.x) / scale
          const dy = (dragCurPx.y - dragStartPx.y) / scale
          rect =
            dragMode === 'move'
              ? { x: orig.x + dx, y: orig.y + dy, w: orig.w, h: orig.h }
              : {
                  x: orig.x,
                  y: orig.y,
                  w: Math.max(MIN_RESIZE_PT, orig.w + dx),
                  h: Math.max(MIN_RESIZE_PT, orig.h + dy),
                }
        }

        const style = {
          left: rect.x * scale,
          top: rect.y * scale,
          width: rect.w * scale,
          height: rect.h * scale,
        }

        return (
          <div
            key={field.index}
            className={`fb-box ${isSelected ? 'selected' : ''}`}
            style={style}
            onPointerDown={(e) => startBoxDrag(e, field, 'move')}
            onDoubleClick={(e) => {
              e.stopPropagation()
              onEditField(field)
            }}
          >
            <span className="fb-box-label">
              {field.name}（{TYPE_LABELS[field.fieldType] ?? field.fieldType}）
            </span>
            {isSelected && (
              <div className="fb-handle" onPointerDown={(e) => startBoxDrag(e, field, 'resize')} />
            )}
          </div>
        )
      })}

      {draftPx && (
        <div
          className="fb-draft"
          style={{ left: draftPx.x, top: draftPx.y, width: draftPx.w, height: draftPx.h }}
        />
      )}
    </div>
  )
}
