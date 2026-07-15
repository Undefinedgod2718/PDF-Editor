import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useRef,
} from 'react'
import { fetchPageText, renderUrl, type CharBox, type Color, type DocInfo, type FormField, type Rect, type SearchHit, type StampMeta } from '../api'
import AnnotLayer from './AnnotLayer'
import FormLayer from './FormLayer'
import CropLayer from './CropLayer'
import type { AnnotTool } from './AnnotToolbar'

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
  },
  ref,
) {
  const containerRef = useRef<HTMLDivElement>(null)
  const pageRefs = useRef<(HTMLDivElement | null)[]>([])
  const textCacheRef = useRef<Map<number, CharBox[]>>(new Map())

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
    <div className="viewer" ref={containerRef}>
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
            {tool === 'form' && (
              <FormLayer
                docId={doc.id}
                scale={scale}
                fields={formFields.filter((f) => f.page === page.index)}
                onFieldChanged={onFormFieldChanged}
              />
            )}
            {cropMode && page.index === currentPage && (
              <CropLayer scale={scale} onRectChange={onCropRectChange} />
            )}
          </div>
        )
      })}
    </div>
  )
})

export default Viewer
