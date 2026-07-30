import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
} from 'react'
import { fetchPageText, renderUrl, type CharBox, type Color, type DocInfo, type FormField, type ImageInfo, type Rect, type RedactBox, type SearchHit, type StampMeta } from '../api'
import AnnotLayer from './AnnotLayer'
import FormLayer from './FormLayer'
import CropLayer from './CropLayer'
import ImageLayer from './ImageLayer'
import RedactLayer from './RedactLayer'
import FormBuilderLayer from './FormBuilderLayer'
import TextLineLayer from './TextLineLayer'
import type { AnnotTool } from './AnnotToolbar'
import type { BuilderFieldType } from './FormBuilderBar'

interface FlashTarget {
  page: number
  rect: Rect
  key: number
}

interface Props {
  doc: DocInfo
  scale: number
  hits: SearchHit[]
  activeHit: number
  onCurrentPageChange: (p: number) => void
  tool: AnnotTool
  color: Color
  inkWidth: number
  stamp: StampMeta | null
  pageVersions: Record<number, number>
  onAnnotationChanged: (page: number) => void
  flash: FlashTarget | null
  formFields: FormField[]
  onFormFieldChanged: (page: number) => void
  /** 目前中心頁（App.tsx 的 currentPage），裁切模式僅在此頁面顯示互動層。 */
  currentPage: number
  /** 裁切模式是否啟用（Toolbar「裁切」按鈕）。 */
  cropMode: boolean
  /** 裁切選取範圍變動（拖曳完成）回呼，帶出 view-space points 矩形。 */
  onCropRectChange: (rectPt: Rect) => void
  /** 影像模式是否啟用（Toolbar「影像」按鈕）。 */
  imageMode: boolean
  /** 目前選取的既有影像 index（取代影像用），未選取時為 null。 */
  selectedImageIndex: number | null
  onSelectImage: (img: ImageInfo) => void
  /** 是否已選好要插入的檔案，等待在頁面上拖曳/點擊放置。 */
  insertArmed: boolean
  /** 插入檔案的原始尺寸換算成 points（96dpi）。 */
  insertNaturalPt: { w: number; h: number } | null
  /** 插入影像拖曳／點擊放置完成後回呼，帶出 view-space points 矩形。 */
  onInsertRectChange: (rectPt: Rect) => void
  /** 表單建立模式是否啟用（Toolbar「建立表單」按鈕）。啟用時目前頁面用 FormBuilderLayer 取代 FormLayer。 */
  formBuilderMode: boolean
  /** FormBuilderBar 目前選取的欄位型別，僅供新拖曳出的欄位使用。 */
  builderFieldType: BuilderFieldType
  /** 拖曳畫出新欄位範圍完成回呼（view-space points）。 */
  onBuilderCreateRect: (rectPt: Rect) => void
  /** 表單欄位建立/修改/刪除成功後回呼，通知上層重新抓表單欄位＋bump 該頁渲染版本。 */
  onFormFieldsChanged: () => void
  /** 雙擊既有欄位框，開啟編輯 dialog。 */
  onEditFormField: (field: FormField) => void
  /** 密文模式是否啟用（Toolbar「密文」按鈕）——可跨頁，每頁都顯示互動層。 */
  redactMode: boolean
  /** 所有頁面目前暫存、尚未套用的密文方框。 */
  redactBoxes: RedactBox[]
  /** 在某頁拖曳新增一個密文方框（view-space points）。 */
  onRedactAddBox: (page: number, rectPt: Rect) => void
  /** 平移（手形）模式是否啟用（Toolbar 按鈕）——拖曳捲動檢視區，停用底下各層互動。 */
  panMode: boolean
}

export interface ViewerHandle {
  scrollToPage: (p: number) => void
  scrollToRect: (p: number, rect: Rect) => void
}

const Viewer = forwardRef<ViewerHandle, Props>(function Viewer(
  {
    doc,
    scale,
    hits,
    activeHit,
    onCurrentPageChange,
    tool,
    color,
    inkWidth,
    stamp,
    pageVersions,
    onAnnotationChanged,
    flash,
    formFields,
    onFormFieldChanged,
    currentPage,
    cropMode,
    onCropRectChange,
    imageMode,
    selectedImageIndex,
    onSelectImage,
    insertArmed,
    insertNaturalPt,
    onInsertRectChange,
    formBuilderMode,
    builderFieldType,
    onBuilderCreateRect,
    onFormFieldsChanged,
    onEditFormField,
    redactMode,
    redactBoxes,
    onRedactAddBox,
    panMode,
  },
  ref,
) {
  const containerRef = useRef<HTMLDivElement>(null)
  const pageRefs = useRef<(HTMLDivElement | null)[]>([])
  const textCacheRef = useRef<Map<number, CharBox[]>>(new Map())
  const panState = useRef<{ x: number; y: number; scrollLeft: number; scrollTop: number } | null>(null)

  const onPanPointerDown = useCallback(
    (e: React.PointerEvent) => {
      if (!panMode) return
      const container = containerRef.current
      if (!container) return
      container.setPointerCapture(e.pointerId)
      panState.current = { x: e.clientX, y: e.clientY, scrollLeft: container.scrollLeft, scrollTop: container.scrollTop }
    },
    [panMode],
  )
  const onPanPointerMove = useCallback((e: React.PointerEvent) => {
    const container = containerRef.current
    const start = panState.current
    if (!container || !start) return
    container.scrollLeft = start.scrollLeft - (e.clientX - start.x)
    container.scrollTop = start.scrollTop - (e.clientY - start.y)
  }, [])
  const onPanPointerUp = useCallback((e: React.PointerEvent) => {
    try {
      containerRef.current?.releasePointerCapture(e.pointerId)
    } catch {
      /* pointer capture already released */
    }
    panState.current = null
  }, [])

  useEffect(() => {
    textCacheRef.current.clear()
  }, [doc.id])

  const getPageChars = useCallback(
    async (page: number): Promise<CharBox[]> => {
      const cached = textCacheRef.current.get(page)
      if (cached) return cached
      const { chars } = await fetchPageText(doc.id, page)
      textCacheRef.current.set(page, chars)
      return chars
    },
    [doc.id],
  )

  useImperativeHandle(ref, () => ({
    scrollToPage: (p: number) => {
      pageRefs.current[p]?.scrollIntoView({ block: 'start', behavior: 'auto' })
    },
    scrollToRect: (p: number, rect: Rect) => {
      const container = containerRef.current
      const el = pageRefs.current[p]
      if (!container || !el) return
      const top = el.offsetTop + rect.y * scale - 96
      container.scrollTo({ top: Math.max(top, 0), behavior: 'smooth' })
    },
  }))

  // Track which page occupies the viewport centre.
  const onScroll = useCallback(() => {
    const container = containerRef.current
    if (!container) return
    const mid = container.scrollTop + container.clientHeight / 2
    let best = 0
    for (let i = 0; i < pageRefs.current.length; i++) {
      const el = pageRefs.current[i]
      if (!el) continue
      if (el.offsetTop <= mid) best = i
      else break
    }
    onCurrentPageChange(best)
  }, [onCurrentPageChange])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    container.addEventListener('scroll', onScroll, { passive: true })
    return () => container.removeEventListener('scroll', onScroll)
  }, [onScroll])

  const dpr = Math.min(window.devicePixelRatio || 1, 2)

  return (
    <div
      className={`viewer ${panMode ? 'viewer-panning' : ''}`}
      ref={containerRef}
      onPointerDown={onPanPointerDown}
      onPointerMove={onPanPointerMove}
      onPointerUp={onPanPointerUp}
      onPointerCancel={onPanPointerUp}
    >
      {doc.pages.map((page) => {
        const cssW = page.width * scale
        const cssH = page.height * scale
        const pageHits = hits
          .map((h, i) => ({ ...h, hitIndex: i }))
          .filter((h) => h.page === page.index)
        return (
          <div
            key={page.index}
            className="page"
            style={{ width: cssW, height: cssH }}
            ref={(el) => {
              pageRefs.current[page.index] = el
            }}
          >
            <img
              src={renderUrl(doc.id, page.index, scale * dpr, pageVersions[page.index])}
              width={cssW}
              height={cssH}
              loading="lazy"
              alt={`第 ${page.index + 1} 頁`}
              draggable={false}
            />
            {pageHits.flatMap((h) =>
              h.rects.map((r, ri) => (
                <div
                  key={`${h.hitIndex}-${ri}`}
                  className={`hl ${h.hitIndex === activeHit ? 'hl-active' : ''}`}
                  style={{
                    left: r.x * scale,
                    top: r.y * scale,
                    width: r.w * scale,
                    height: r.h * scale,
                  }}
                />
              )),
            )}
            <AnnotLayer
              docId={doc.id}
              page={page.index}
              scale={scale}
              tool={tool}
              color={color}
              inkWidth={inkWidth}
              stamp={stamp}
              version={pageVersions[page.index] ?? 0}
              getPageChars={getPageChars}
              onChanged={() => onAnnotationChanged(page.index)}
              flashRect={flash && flash.page === page.index ? flash.rect : null}
              flashKey={flash?.key ?? 0}
            />
            {tool === 'editLine' && (
              <TextLineLayer
                docId={doc.id}
                page={page.index}
                scale={scale}
                version={pageVersions[page.index] ?? 0}
                onChanged={() => onAnnotationChanged(page.index)}
              />
            )}
            {formBuilderMode && page.index === currentPage ? (
              <FormBuilderLayer
                docId={doc.id}
                page={page.index}
                scale={scale}
                fields={formFields.filter((f) => f.page === page.index)}
                selectedType={builderFieldType}
                onCreateRect={onBuilderCreateRect}
                onFieldsChanged={onFormFieldsChanged}
                onEditField={onEditFormField}
              />
            ) : (
              tool === 'form' && (
                <FormLayer
                  docId={doc.id}
                  scale={scale}
                  fields={formFields.filter((f) => f.page === page.index)}
                  onFieldChanged={onFormFieldChanged}
                />
              )
            )}
            {cropMode && page.index === currentPage && (
              <CropLayer scale={scale} onRectChange={onCropRectChange} />
            )}
            {imageMode && page.index === currentPage && (
              <ImageLayer
                docId={doc.id}
                page={page.index}
                scale={scale}
                version={pageVersions[page.index] ?? 0}
                pageWidth={page.width}
                pageHeight={page.height}
                selectedIndex={selectedImageIndex}
                onSelectImage={onSelectImage}
                insertArmed={insertArmed}
                insertNaturalPt={insertNaturalPt}
                onInsertRectChange={onInsertRectChange}
              />
            )}
            {redactMode && (
              <RedactLayer
                scale={scale}
                boxesPt={redactBoxes
                  .filter((b) => b.page === page.index)
                  .map((b) => ({ x: b.x, y: b.y, w: b.w, h: b.h }))}
                onAddBox={(rectPt) => onRedactAddBox(page.index, rectPt)}
              />
            )}
          </div>
        )
      })}
    </div>
  )
})

export default Viewer
