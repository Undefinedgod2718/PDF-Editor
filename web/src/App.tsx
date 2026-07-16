import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  fetchDocForm,
  fetchDocInfo,
  getProtectionStatus,
  uploadPdf,
  type Color,
  type DocInfo,
  type FormField,
  type ImageInfo,
  type Rect,
  type SearchHit,
  type StampMeta,
} from './api'
import Toolbar from './components/Toolbar'
import ThumbnailPanel from './components/ThumbnailPanel'
import Viewer, { type ViewerHandle } from './components/Viewer'
import SearchPanel from './components/SearchPanel'
import AnnotToolbar, { type AnnotTool } from './components/AnnotToolbar'
import AnnotPanel from './components/AnnotPanel'
import StampDrawer from './components/StampDrawer'
import DrawingModal from './components/DrawingModal'
import SignaturePad from './components/SignaturePad'
import CropBar from './components/CropBar'
import ImageBar from './components/ImageBar'
import ExportDialog from './components/ExportDialog'
import CompressDialog from './components/CompressDialog'
import ProtectDialog from './components/ProtectDialog'
import EncryptDialog from './components/EncryptDialog'
import DecryptPrompt from './components/DecryptPrompt'
import CompareDialog from './components/CompareDialog'
import FormBuilderBar, { type BuilderFieldType } from './components/FormBuilderBar'
import FieldDialog from './components/FieldDialog'

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
  const [selectedStamp, setSelectedStamp] = useState<StampMeta | null>(null)

  // ---- 表單填寫相關狀態（Phase 4）----
  const [formFields, setFormFields] = useState<FormField[]>([])
  const [formFieldsLoaded, setFormFieldsLoaded] = useState(false)

  // ---- 頁面裁切相關狀態（Phase 6）----
  const [cropMode, setCropMode] = useState(false)
  const [cropRect, setCropRect] = useState<Rect | null>(null)

  // ---- 影像插入／取代相關狀態（Phase 7）----
  const [imageMode, setImageMode] = useState(false)
  const [selectedImage, setSelectedImage] = useState<ImageInfo | null>(null)
  const [insertArmed, setInsertArmed] = useState(false)
  const [insertNaturalPt, setInsertNaturalPt] = useState<{ w: number; h: number } | null>(null)
  const [insertRect, setInsertRect] = useState<Rect | null>(null)

  // ---- 表單建立相關狀態（Phase 14）----
  const [formBuilderMode, setFormBuilderMode] = useState(false)
  const [builderFieldType, setBuilderFieldType] = useState<BuilderFieldType>('text')
  /** 拖曳畫出的新欄位範圍，非 null 時開啟 FieldDialog（create 模式）。 */
  const [pendingField, setPendingField] = useState<{ page: number; rect: Rect } | null>(null)
  /** 雙擊選取的既有欄位，非 null 時開啟 FieldDialog（edit 模式）。 */
  const [editingField, setEditingField] = useState<FormField | null>(null)

  // ---- 匯出對話框相關狀態（Phase 8）----
  const [showExport, setShowExport] = useState(false)

  // ---- 壓縮對話框相關狀態（Phase 9）----
  const [showCompress, setShowCompress] = useState(false)

  // ---- 保護對話框相關狀態（Phase 11）----
  const [showProtect, setShowProtect] = useState(false)

  // ---- 密文對話框相關狀態（Phase 12）----
  const [showEncrypt, setShowEncrypt] = useState(false)
  // 上傳／開啟文件時 GET /info 失敗，且偵測到是開檔密碼加密的文件：記錄 id／檔名，
  // 顯示「輸入密碼解密下載」提示，取代原本的死錯誤訊息。
  const [lockedDoc, setLockedDoc] = useState<{ id: string; filename?: string } | null>(null)

  // ---- 比較對話框相關狀態（Phase 13）----
  const [showCompare, setShowCompare] = useState(false)

  const resetImageInteraction = useCallback(() => {
    setSelectedImage(null)
    setInsertArmed(false)
    setInsertNaturalPt(null)
    setInsertRect(null)
  }, [])

  // 換頁時丟掉上一頁的選取，避免把 A 頁 view-space rect／影像選取套到 B 頁。
  useEffect(() => {
    setCropRect(null)
    resetImageInteraction()
    setPendingField(null)
    setEditingField(null)
  }, [currentPage, resetImageInteraction])

  // 載入文件（開啟本地檔案／合併或擷取後切換開啟）共用的重置邏輯。
  const loadDoc = useCallback(async (id: string) => {
    const info = await fetchDocInfo(id)
    setDoc(info)
    setCurrentPage(0)
    setHits([])
    setActiveHit(-1)
    setPageVersions({})
    setFlash(null)
    setTool('select')
    setSelectedStamp(null)
    setFormFields([])
    setFormFieldsLoaded(false)
    setCropMode(false)
    setCropRect(null)
    setImageMode(false)
    setSelectedImage(null)
    setInsertArmed(false)
    setInsertNaturalPt(null)
    setInsertRect(null)
    setFormBuilderMode(false)
    setBuilderFieldType('text')
    setPendingField(null)
    setEditingField(null)
    setShowExport(false)
    setShowCompress(false)
    setShowProtect(false)
    setShowEncrypt(false)
    setShowCompare(false)
  }, [])

  // fetchDocInfo／render 對開檔密碼加密的 PDF 一律 500（PDFium 打不開）。GET /protection
  // 則讀得到（權限位元不受加密影響），protected=true 是「這份文件需要解密」的訊號。
  // 偵測到就顯示解密提示，取代原本的死錯誤訊息；回傳 true 代表已處理（呼叫端不必再 setError）。
  const tryHandleEncrypted = useCallback(async (id: string, filename?: string): Promise<boolean> => {
    try {
      const status = await getProtectionStatus(id)
      if (status.protected) {
        setLockedDoc({ id, filename })
        return true
      }
    } catch {
      // 連 /protection 都失敗：不是加密造成的已知情境，交給原本的錯誤訊息處理。
    }
    return false
  }, [])

  const openFile = useCallback(
    async (file: File) => {
      setBusy(true)
      setError(null)
      setLockedDoc(null)
      let uploadedId: string | undefined
      try {
        const { id } = await uploadPdf(file)
        uploadedId = id
        await loadDoc(id)
      } catch (e) {
        if (uploadedId && (await tryHandleEncrypted(uploadedId, file.name))) return
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [loadDoc, tryHandleEncrypted],
  )

  const openDocById = useCallback(
    async (id: string) => {
      setBusy(true)
      setError(null)
      setLockedDoc(null)
      try {
        await loadDoc(id)
      } catch (e) {
        if (await tryHandleEncrypted(id)) return
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusy(false)
      }
    },
    [loadDoc, tryHandleEncrypted],
  )

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

  // 送進渲染 URL 的版本 = 伺服器持久 revision（每次寫入 +1，重啟不歸零）+ 本
  // session 的本地 bump。後端對帶 ?v= 的渲染回應標 immutable，所以這個值一旦
  // 對應過某個內容狀態就不能再指向別的內容：mountRev' ≥ mountRev + 本 session
  // 全部 bump 數，因此同一頁的 v 只會在內容完全沒變時重複（單一寫入者前提）。
  const renderVersions = useMemo(() => {
    const out: Record<number, number> = {}
    if (!doc) return out
    for (let i = 0; i < doc.pageCount; i++) out[i] = doc.revision + (pageVersions[i] ?? 0)
    return out
  }, [doc, pageVersions])

  // 該頁內容版本變了（刪物件／寫入／revision bump）→ 全物件集合可能重編 index。
  // 沒有穩定影像 ID，只能清選取，逼使用者重點；否則 replace 會打到錯物件。
  const currentImageListVersion = renderVersions[currentPage] ?? 0
  useEffect(() => {
    setSelectedImage(null)
  }, [currentImageListVersion])

  // 頁面結構操作（旋轉/刪除/插入/重排）成功後：重新抓 doc info，並清空全部頁面的
  // pageVersions 快取（全部 +1），確保縮圖與內文渲染都重新抓取最新內容。
  const refreshDocStructure = useCallback(async () => {
    if (!doc) return
    const info = await fetchDocInfo(doc.id)
    setDoc(info)
    setCurrentPage((p) => Math.min(p, info.pageCount - 1))
    setPageVersions((v) => {
      const maxV = Math.max(0, ...Object.values(v))
      const next = maxV + 1
      const nv: Record<number, number> = {}
      for (let i = 0; i < info.pageCount; i++) nv[i] = next
      return nv
    })
  }, [doc])

  const selectAnnotation = useCallback((page: number, rect: Rect) => {
    flashSeq.current += 1
    setFlash({ page, rect, key: flashSeq.current })
    viewerRef.current?.scrollToRect(page, rect)
  }, [])

  // 表單工具選中、或表單建立模式啟用時，抓一次全文件欄位（含每頁 rect）。
  useEffect(() => {
    if ((tool !== 'form' && !formBuilderMode) || !doc) return
    let cancelled = false
    fetchDocForm(doc.id)
      .then((fields) => {
        if (cancelled) return
        setFormFields(fields)
        setFormFieldsLoaded(true)
      })
      .catch((err) => console.error('fetchDocForm failed:', err))
    return () => {
      cancelled = true
    }
  }, [tool, formBuilderMode, doc])

  // 表單欄位寫入成功後：重新抓整份文件欄位（radio 群組等連動狀態才會同步），並 bump 該頁版本讓渲染圖重新烙值。
  const onFormFieldChanged = useCallback(
    (page: number) => {
      bumpPageVersion(page)
      if (!doc) return
      fetchDocForm(doc.id)
        .then((fields) => setFormFields(fields))
        .catch((err) => console.error('fetchDocForm failed:', err))
    },
    [doc, bumpPageVersion],
  )

  // 表單建立模式：建立/修改/刪除欄位皆發生在目前頁面，重用 onFormFieldChanged 的邏輯即可。
  const onBuilderFieldsChanged = useCallback(() => {
    onFormFieldChanged(currentPage)
  }, [onFormFieldChanged, currentPage])

  const onBuilderCreateRect = useCallback(
    (rectPt: Rect) => {
      setPendingField({ page: currentPage, rect: rectPt })
    },
    [currentPage],
  )

  const toggleCrop = useCallback(() => {
    setCropMode((v) => {
      const next = !v
      if (next) {
        setTool('select') // 裁切時停用其他註解工具，避免 AnnotLayer 搶走指標事件
        setImageMode(false)
        resetImageInteraction()
        setFormBuilderMode(false)
        setPendingField(null)
        setEditingField(null)
      } else {
        setCropRect(null)
      }
      return next
    })
  }, [resetImageInteraction])

  const toggleImageMode = useCallback(() => {
    setImageMode((v) => {
      const next = !v
      if (next) {
        setTool('select') // 影像模式時停用其他註解工具，避免 AnnotLayer 搶走指標事件
        setCropMode(false)
        setCropRect(null)
        setFormBuilderMode(false)
        setPendingField(null)
        setEditingField(null)
      }
      resetImageInteraction()
      return next
    })
  }, [resetImageInteraction])

  const toggleFormBuilder = useCallback(() => {
    setFormBuilderMode((v) => {
      const next = !v
      if (next) {
        setTool('select') // 表單建立模式時停用其他註解工具，避免 AnnotLayer 搶走指標事件
        setCropMode(false)
        setCropRect(null)
        setImageMode(false)
        resetImageInteraction()
      } else {
        setPendingField(null)
        setEditingField(null)
      }
      return next
    })
  }, [resetImageInteraction])

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'f' && doc) {
        e.preventDefault()
        setShowSearch(true)
      }
      if (e.key === 'Escape') {
        // 繪圖模式／簽名板開啟時由各自的 modal 處理 Escape（stopPropagation 後關閉），避免搶先把 tool 切走。
        if (tool === 'draw' || tool === 'sign') return
        if (cropMode) {
          setCropMode(false)
          setCropRect(null)
          return
        }
        if (imageMode) {
          setImageMode(false)
          resetImageInteraction()
          return
        }
        if (formBuilderMode) {
          if (pendingField || editingField) {
            setPendingField(null)
            setEditingField(null)
            return
          }
          setFormBuilderMode(false)
          return
        }
        setTool('select')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [doc, tool, cropMode, imageMode, formBuilderMode, pendingField, editingField, resetImageInteraction])

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
        {lockedDoc && (
          <DecryptPrompt
            id={lockedDoc.id}
            filename={lockedDoc.filename}
            onClose={() => setLockedDoc(null)}
          />
        )}
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
        cropMode={cropMode}
        toggleCrop={toggleCrop}
        imageMode={imageMode}
        toggleImageMode={toggleImageMode}
        formBuilderMode={formBuilderMode}
        toggleFormBuilder={toggleFormBuilder}
        showExport={showExport}
        toggleExport={() => setShowExport((v) => !v)}
        showCompress={showCompress}
        toggleCompress={() => setShowCompress((v) => !v)}
        showProtect={showProtect}
        toggleProtect={() => setShowProtect((v) => !v)}
        showEncrypt={showEncrypt}
        toggleEncrypt={() => setShowEncrypt((v) => !v)}
        showCompare={showCompare}
        toggleCompare={() => setShowCompare((v) => !v)}
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
        noFormFields={formFieldsLoaded && formFields.length === 0}
      />
      <div className="workspace">
        {showThumbs && (
          <ThumbnailPanel
            doc={doc}
            currentPage={currentPage}
            gotoPage={gotoPage}
            pageVersions={renderVersions}
            onStructureChanged={refreshDocStructure}
            onOpenDoc={openDocById}
          />
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
          stamp={selectedStamp}
          pageVersions={renderVersions}
          onAnnotationChanged={bumpPageVersion}
          flash={flash}
          formFields={formFields}
          onFormFieldChanged={onFormFieldChanged}
          currentPage={currentPage}
          cropMode={cropMode}
          onCropRectChange={setCropRect}
          imageMode={imageMode}
          selectedImageIndex={selectedImage?.index ?? null}
          onSelectImage={setSelectedImage}
          insertArmed={insertArmed}
          insertNaturalPt={insertNaturalPt}
          onInsertRectChange={setInsertRect}
          formBuilderMode={formBuilderMode}
          builderFieldType={builderFieldType}
          onBuilderCreateRect={onBuilderCreateRect}
          onFormFieldsChanged={onBuilderFieldsChanged}
          onEditFormField={setEditingField}
        />
        {cropMode && (
          <CropBar
            doc={doc}
            currentPage={currentPage}
            rect={cropRect}
            onApplied={refreshDocStructure}
            onClose={() => {
              setCropMode(false)
              setCropRect(null)
            }}
          />
        )}
        {imageMode && (
          <ImageBar
            doc={doc}
            currentPage={currentPage}
            selectedImage={selectedImage}
            insertArmed={insertArmed}
            insertRect={insertRect}
            onArmInsert={(naturalPt) => {
              setInsertArmed(true)
              setInsertNaturalPt(naturalPt)
              setInsertRect(null)
              setSelectedImage(null)
            }}
            onApplied={() => bumpPageVersion(currentPage)}
            onReset={resetImageInteraction}
            onClose={() => {
              setImageMode(false)
              resetImageInteraction()
            }}
          />
        )}
        {formBuilderMode && (
          <FormBuilderBar
            selectedType={builderFieldType}
            onSelectType={setBuilderFieldType}
            onDone={() => {
              setFormBuilderMode(false)
              setPendingField(null)
              setEditingField(null)
            }}
          />
        )}
        {pendingField && (
          <FieldDialog
            mode="create"
            docId={doc.id}
            page={pendingField.page}
            pageHeight={doc.pages[pendingField.page]?.height ?? 792}
            fieldType={builderFieldType}
            rectPt={pendingField.rect}
            onClose={() => setPendingField(null)}
            onCreated={onBuilderFieldsChanged}
          />
        )}
        {editingField && (
          <FieldDialog
            mode="edit"
            docId={doc.id}
            page={editingField.page}
            field={editingField}
            onClose={() => setEditingField(null)}
            onUpdated={onBuilderFieldsChanged}
          />
        )}
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
            version={renderVersions[currentPage] ?? 0}
            onDeleted={bumpPageVersion}
            onSelect={selectAnnotation}
            onClose={() => setShowAnnotPanel(false)}
          />
        )}
        {tool === 'stamp' && (
          <StampDrawer
            selected={selectedStamp}
            onSelect={setSelectedStamp}
            onClose={() => setTool('select')}
          />
        )}
        {tool === 'draw' && (
          <DrawingModal
            docId={doc.id}
            page={currentPage}
            onDone={(stamp) => {
              setSelectedStamp(stamp)
              setTool('stamp')
            }}
            onCancel={() => setTool('select')}
          />
        )}
        {tool === 'sign' && (
          <SignaturePad
            onDone={(stamp) => {
              setSelectedStamp(stamp)
              setTool('stamp')
            }}
            onCancel={() => setTool('select')}
          />
        )}
        {showExport && <ExportDialog doc={doc} onClose={() => setShowExport(false)} />}
        {showCompress && (
          <CompressDialog
            doc={doc}
            onClose={() => setShowCompress(false)}
            onOpenDoc={async (id) => {
              setShowCompress(false)
              await openDocById(id)
            }}
          />
        )}
        {showProtect && (
          <ProtectDialog
            doc={doc}
            onClose={() => setShowProtect(false)}
            onOpenDoc={async (id) => {
              setShowProtect(false)
              await openDocById(id)
            }}
          />
        )}
        {showEncrypt && <EncryptDialog doc={doc} onClose={() => setShowEncrypt(false)} />}
        {showCompare && (
          <CompareDialog
            doc={doc}
            onClose={() => setShowCompare(false)}
            onOpenDoc={async (id) => {
              setShowCompare(false)
              await openDocById(id)
            }}
          />
        )}
        {lockedDoc && (
          <DecryptPrompt
            id={lockedDoc.id}
            filename={lockedDoc.filename}
            onClose={() => setLockedDoc(null)}
          />
        )}
      </div>
    </div>
  )
}
