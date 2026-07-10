import { useCallback, useEffect, useRef, useState } from 'react'
import { fetchDocInfo, uploadPdf, type Color, type DocInfo, type Rect, type SearchHit } from './api'
import Toolbar from './components/Toolbar'
import ThumbnailPanel from './components/ThumbnailPanel'
import Viewer, { type ViewerHandle } from './components/Viewer'
import SearchPanel from './components/SearchPanel'
import AnnotToolbar, { type AnnotTool } from './components/AnnotToolbar'
import AnnotPanel from './components/AnnotPanel'

interface FlashTarget {
  page: number
  rect: Rect
  key: number
}

export default function App() {
  const [doc, setDoc] = useState<DocInfo | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [scale, setScale] = useState(1.25)
  const [currentPage, setCurrentPage] = useState(0)
  const [showThumbs, setShowThumbs] = useState(true)
  const [showSearch, setShowSearch] = useState(false)
  const [hits, setHits] = useState<SearchHit[]>([])
  const [activeHit, setActiveHit] = useState(-1)
  const viewerRef = useRef<ViewerHandle>(null)

  // ---- 註解相關狀態 ----
  const [tool, setTool] = useState<AnnotTool>('select')
  const [color, setColor] = useState<Color>({ r: 255, g: 214, b: 0 })
  const [inkWidth, setInkWidth] = useState(2)
  const [showAnnotPanel, setShowAnnotPanel] = useState(false)
  const [pageVersions, setPageVersions] = useState<Record<number, number>>({})
  const [flash, setFlash] = useState<FlashTarget | null>(null)
  const flashSeq = useRef(0)

  const openFile = useCallback(async (file: File) => {
    setBusy(true)
    setError(null)
    try {
      const { id } = await uploadPdf(file)
      const info = await fetchDocInfo(id)
      setDoc(info)
      setCurrentPage(0)
      setHits([])
      setActiveHit(-1)
      setPageVersions({})
      setFlash(null)
      setTool('select')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }, [])

  const gotoPage = useCallback((p: number) => {
    viewerRef.current?.scrollToPage(p)
  }, [])

  const gotoHit = useCallback(
    (index: number) => {
      setActiveHit(index)
      const hit = hits[index]
      if (hit) viewerRef.current?.scrollToPage(hit.page)
    },
    [hits],
  )

  // 註解建立/刪除後：該頁渲染圖已經在後端烙進新內容，需要 cache-bust 版本號讓 <img> 重新抓取。
  const bumpPageVersion = useCallback((page: number) => {
    setPageVersions((v) => ({ ...v, [page]: (v[page] ?? 0) + 1 }))
  }, [])

  const selectAnnotation = useCallback((page: number, rect: Rect) => {
    flashSeq.current += 1
    setFlash({ page, rect, key: flashSeq.current })
    viewerRef.current?.scrollToRect(page, rect)
  }, [])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'f' && doc) {
        e.preventDefault()
        setShowSearch(true)
      }
      if (e.key === 'Escape') {
        setTool('select')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [doc])

  if (!doc) {
    return (
      <div className="welcome">
        <div className="welcome-card">
          <h1>PDF Editor</h1>
          <p>開啟 PDF 檔案開始檢視與編輯</p>
          <label className="btn btn-primary">
            {busy ? '載入中…' : '開啟 PDF'}
            <input
              type="file"
              accept="application/pdf"
              hidden
              disabled={busy}
              onChange={(e) => {
                const f = e.target.files?.[0]
                if (f) void openFile(f)
              }}
            />
          </label>
          {error && <p className="error">{error}</p>}
        </div>
      </div>
    )
  }

  return (
    <div className="app">
      <Toolbar
        doc={doc}
        scale={scale}
        setScale={setScale}
        currentPage={currentPage}
        gotoPage={gotoPage}
        showThumbs={showThumbs}
        toggleThumbs={() => setShowThumbs((v) => !v)}
        showSearch={showSearch}
        toggleSearch={() => setShowSearch((v) => !v)}
        openFile={openFile}
      />
      <AnnotToolbar
        tool={tool}
        setTool={setTool}
        color={color}
        setColor={setColor}
        inkWidth={inkWidth}
        setInkWidth={setInkWidth}
        showAnnotPanel={showAnnotPanel}
        toggleAnnotPanel={() => setShowAnnotPanel((v) => !v)}
      />
      <div className="workspace">
        {showThumbs && (
          <ThumbnailPanel doc={doc} currentPage={currentPage} gotoPage={gotoPage} />
        )}
        <Viewer
          ref={viewerRef}
          doc={doc}
          scale={scale}
          hits={hits}
          activeHit={activeHit}
          onCurrentPageChange={setCurrentPage}
          tool={tool}
          color={color}
          inkWidth={inkWidth}
          pageVersions={pageVersions}
          onAnnotationChanged={bumpPageVersion}
          flash={flash}
        />
        {showSearch && (
          <SearchPanel
            doc={doc}
            hits={hits}
            setHits={setHits}
            activeHit={activeHit}
            gotoHit={gotoHit}
            onClose={() => {
              setShowSearch(false)
              setHits([])
              setActiveHit(-1)
            }}
          />
        )}
        {showAnnotPanel && (
          <AnnotPanel
            doc={doc}
            currentPage={currentPage}
            version={pageVersions[currentPage] ?? 0}
            onDeleted={bumpPageVersion}
            onSelect={selectAnnotation}
            onClose={() => setShowAnnotPanel(false)}
          />
        )}
      </div>
    </div>
  )
}
