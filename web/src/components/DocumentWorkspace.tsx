import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  checkDirty,
  deletePage,
  fetchDocForm,
  fetchDocInfo,
  getProtectionStatus,
  insertPage,
  rotateAllPages,
  rotatePage,
  saveAsDialog,
  saveDoc,
  SaveConflictError,
  type AppMode,
  type Color,
  type DocInfo,
  type FormField,
  type ImageInfo,
  type Rect,
  type RedactBox,
  type SearchHit,
  type StampMeta,
} from '../api'
import Toolbar, { type DialogKind } from './Toolbar'
import ToolTabs, { loadStoredToolTab, storeToolTab, type ToolTabId } from './ToolTabs'
import ThumbnailPanel from './ThumbnailPanel'
import { MergeDialog, ExtractDialog } from './PageDialogs'
import Viewer, { type ViewerHandle } from './Viewer'
import SearchPanel from './SearchPanel'
import AnnotToolbar, { type AnnotTool } from './AnnotToolbar'
import ToolRail from './ToolRail'
import AnnotPanel from './AnnotPanel'
import StampDrawer from './StampDrawer'
import DrawingModal from './DrawingModal'
import SignaturePad from './SignaturePad'
import CropBar from './CropBar'
import ImageBar from './ImageBar'
import RedactBar from './RedactBar'
import ExportDialog from './ExportDialog'
import PrintDialog from './PrintDialog'
import CompressDialog from './CompressDialog'
import OcrDialog from './OcrDialog'
import ProtectDialog from './ProtectDialog'
import EncryptDialog from './EncryptDialog'
import DecryptPrompt from './DecryptPrompt'
import CompareDialog from './CompareDialog'
import FormBuilderBar, { type BuilderFieldType } from './FormBuilderBar'
import FieldDialog from './FieldDialog'
import ActionWizardDialog from './ActionWizardDialog'
import OutlinePanel from './OutlinePanel'
import LinkDialog from './LinkDialog'
import WatermarkDialog from './WatermarkDialog'
import HeaderFooterDialog from './HeaderFooterDialog'

interface FlashTarget {
  page: number
  rect: Rect
  key: number
}

export interface DocumentWorkspaceProps {
  initialDoc: DocInfo
  mode: AppMode
  /** false 時分頁在背景（display:none 由外層套），鍵盤快捷鍵/視窗關閉檢查一律不回應。 */
  active: boolean
  onDirtyChange: (dirty: boolean) => void
  onDocChange: (doc: DocInfo) => void
  /** 分頁列「開啟」動作一律開新分頁，交給外層（shell）處理，非取代本分頁內容。 */
  onOpenFileNewTab: (file: File) => void
  onOpenLocalNewTab: () => void
  /** 全螢幕狀態是整個 app 共用（單一視窗／單一 document），不是分頁各自的事，狀態留在 App.tsx。 */
  fullscreen: boolean
  onToggleFullscreen: () => void
}

export default function DocumentWorkspace({
  initialDoc,
  mode,
  active,
  onDirtyChange,
  onDocChange,
  onOpenFileNewTab,
  onOpenLocalNewTab,
  fullscreen,
  onToggleFullscreen,
}: DocumentWorkspaceProps) {
  const [doc, setDoc] = useState<DocInfo>(initialDoc)
  const [error, setError] = useState<string | null>(null)
  const [scale, setScale] = useState(1.25)
  const [currentPage, setCurrentPage] = useState(0)
  const [showThumbs, setShowThumbs] = useState(true)
  const [showOutline, setShowOutline] = useState(false)
  const [showSearch, setShowSearch] = useState(false)
  // ---- 頂部工具分類頁籤（②b）----
  // 用 lazy initializer 讀 localStorage：只在這個 DocumentWorkspace 掛載那一刻讀一次，
  // 之後純粹是本分頁自己的 state；切分類頁籤時同步寫回 localStorage，讓下次開檔（含
  // 別的分頁／別次啟動）記得使用者上次選的分類。壞值／清過的 localStorage 一律退回「檢視」。
  const [activeTab, setActiveTabState] = useState<ToolTabId>(() => loadStoredToolTab())
  const setActiveTab = useCallback((id: ToolTabId) => {
    setActiveTabState(id)
    storeToolTab(id)
  }, [])
  const [hits, setHits] = useState<SearchHit[]>([])
  const [activeHit, setActiveHit] = useState(-1)
  const viewerRef = useRef<ViewerHandle>(null)

  // ---- 註解相關狀態 ----
  const [tool, setTool] = useState<AnnotTool>('select')
  const [color, setColor] = useState<Color>({ r: 255, g: 214, b: 0 })
  const [inkWidth, setInkWidth] = useState(2)
  const [showAnnotPanel, setShowAnnotPanel] = useState(false)
  // ---- 彈出選單（頂部工具列下拉 ＋ 左側浮動條 flyout）：**共用同一個 state**，所以
  // 「同時只開一個」是結構保證，而不是靠每個彈出層自己記得關掉別人。放這裡（不是各元件
  // 內部、也不是 App.tsx）才能讓下面 Escape 鏈在別的分支之前優先把它收掉。
  // id 用字串命名空間區隔：浮動條是 `rail:*`，工具列是 `tb:*`。 ----
  const [openMenu, setOpenMenu] = useState<string | null>(null)
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

  const [rotatingAll, setRotatingAll] = useState(false)

  // ---- 區域密文／光柵化相關狀態（Phase 12）----
  const [redactMode, setRedactMode] = useState(false)
  const [redactBoxes, setRedactBoxes] = useState<RedactBox[]>([])

  // ---- 平移（手形）模式相關狀態 ----
  const [panMode, setPanMode] = useState(false)

  // ---- 連結模式 ----
  // 跟密文一樣是跨頁的模式：每一頁都掛互動層，拉完框才彈對話框問目標。
  const [linkMode, setLinkMode] = useState(false)
  const [pendingLink, setPendingLink] = useState<{ page: number; rect: Rect } | null>(null)

  // ---- 表單建立相關狀態（Phase 14）----
  const [formBuilderMode, setFormBuilderMode] = useState(false)
  const [builderFieldType, setBuilderFieldType] = useState<BuilderFieldType>('text')
  /** 拖曳畫出的新欄位範圍，非 null 時開啟 FieldDialog（create 模式）。 */
  const [pendingField, setPendingField] = useState<{ page: number; rect: Rect } | null>(null)
  /** 雙擊選取的既有欄位，非 null 時開啟 FieldDialog（edit 模式）。 */
  const [editingField, setEditingField] = useState<FormField | null>(null)

  // ---- 文件工具對話框（匯出/壓縮/OCR/保護/密文/動作精靈/比較，Phase 8/9/11/12/13/P17）----
  // 七個原本各自獨立的 boolean 收成一個互斥狀態：同時間最多一個對話框開著。
  const [activeDialog, setActiveDialog] = useState<DialogKind | null>(null)
  const toggleDialog = useCallback((kind: DialogKind) => {
    setActiveDialog((cur) => (cur === kind ? null : kind))
  }, [])
  // 只在目前開著的就是這個 kind 時才關，避免非同步 onOpenDoc 延遲觸發時誤關掉使用者
  // 已經切換開啟的另一個對話框。
  const closeDialog = useCallback((kind: DialogKind) => {
    setActiveDialog((cur) => (cur === kind ? null : cur))
  }, [])
  // 分頁內動作（壓縮/OCR/保護/密文/比較/動作精靈/縮圖跨文件連結）產出新文件 id 後，
  // 用 replaceDoc 換掉本分頁內容；若換入的文件剛好是開檔密碼加密的，一樣走這個提示。
  const [lockedDoc, setLockedDoc] = useState<{ id: string; filename?: string } | null>(null)

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

  // 本分頁換文件內容（合併/擷取/壓縮/OCR/保護/密文/比較/動作精靈結果、縮圖跨文件連結）共用的重置邏輯。
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
    setActiveDialog(null)
    setDirty(false)
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

  const replaceDoc = useCallback(
    async (id: string) => {
      setError(null)
      setLockedDoc(null)
      try {
        await loadDoc(id)
      } catch (e) {
        if (await tryHandleEncrypted(id)) return
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [loadDoc, tryHandleEncrypted],
  )

  // ---- 本機模式存檔（ADR-004）：mode==='local' 才會被呼叫 ----
  const [saveMessage, setSaveMessage] = useState<string | null>(null)
  const flashSaveMessage = useCallback((msg: string) => {
    setSaveMessage(msg)
    setTimeout(() => setSaveMessage((cur) => (cur === msg ? null : cur)), 2500)
  }, [])

  // dirty = 後端 revision != saved_revision（pdf-core storage.rs 已有此不變量）；
  // 直接問伺服器而非在前端鏡一份 revision，因為多數編輯只 bump 本地 pageVersions
  // 快取，doc.revision 未必即時同步——問伺服器才不會漏標。pageVersions 在每一種
  // 文件內容變動後都會變（bumpPageVersion／refreshDocStructure 兩個集中點），拿來當
  // 重新檢查的觸發點剛好不必碰任何個別 mutation call site。
  const [dirty, setDirty] = useState(false)
  useEffect(() => {
    if (mode !== 'local') return
    // 背景分頁不輪詢 dirty：只有 active 分頁的內容會被編輯（pageVersions 才會變），
    // 背景分頁問了也永遠拿到同一答案。變 active 時 active 進依賴會重跑一次補上最新狀態。
    if (!active) return
    let cancelled = false
    checkDirty(doc.id)
      .then((d) => {
        if (!cancelled) setDirty(d)
      })
      .catch(() => {
        // 問不到就維持現狀；下一次編輯或存檔還會再問一次。
      })
    return () => {
      cancelled = true
    }
  }, [mode, doc, pageVersions, active])

  // 分頁的 dirty／doc 變動回報給外層（分頁列的未存檔圓點＋視窗關閉前的批次存檔都要看這個）。
  // 依賴陣列刻意只放 dirty／doc 本身，不放 callback：外層（shell）每次 render 都會傳入新的
  // inline closure（per-tab 分別綁定 tab key），若把 callback 也列進依賴，會變成「回報一次
  // →外層 setState 觸發 re-render→closure 換新引用→依賴變了再回報一次」的無限迴圈
  // （實測 React 直接丟 Maximum update depth exceeded，整個分頁鎖死）。
  useEffect(() => {
    onDirtyChange(dirty)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dirty])
  useEffect(() => {
    onDocChange(doc)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [doc])

  const saveLocal = useCallback(async () => {
    setError(null)
    try {
      await saveDoc(doc.id)
      setDirty(false)
      flashSaveMessage('已存檔')
    } catch (e) {
      if (e instanceof SaveConflictError) {
        const forceOverwrite = window.confirm(`${e.message}\n是否強制覆寫原檔？`)
        if (!forceOverwrite) return
        try {
          await saveDoc(doc.id, true)
          setDirty(false)
          flashSaveMessage('已存檔（覆寫外部修改）')
        } catch (e2) {
          setError(e2 instanceof Error ? e2.message : String(e2))
        }
        return
      }
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [doc, flashSaveMessage])

  const saveAsLocal = useCallback(async () => {
    setError(null)
    try {
      const meta = await saveAsDialog(doc.id)
      if (!meta) return // 使用者取消
      setDoc((cur) => ({ ...cur, filename: meta.filename }))
      setDirty(false)
      flashSaveMessage('已另存新檔')
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [doc, flashSaveMessage])

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

  const rotateDocument = useCallback(async () => {
    if (rotatingAll) return
    setRotatingAll(true)
    setError(null)
    try {
      await rotateAllPages(doc.id, 90)
      await refreshDocStructure()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setRotatingAll(false)
    }
  }, [doc, rotatingAll, refreshDocStructure])

  // ---- 頁面操作的編輯頁入口（②d）：ThumbnailPanel 內的旋轉/插入/刪除/合併/擷取按鈕是
  // 唯一入口，縮圖面板關掉就完全碰不到——這裡補一組作用在 currentPage 上的等效操作。
  // 重新排序仍然只能拖放縮圖，所以「重新排序…」按鈕只負責開面板，不是自己的 UI。
  const [pageOpBusy, setPageOpBusy] = useState(false)
  const runPageOp = useCallback(
    async (fn: () => Promise<unknown>) => {
      setPageOpBusy(true)
      try {
        await fn()
        await refreshDocStructure()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setPageOpBusy(false)
      }
    },
    [refreshDocStructure],
  )
  const rotateCurrentPage = useCallback(() => {
    const page = doc.pages[currentPage]
    if (!page) return
    const next = ((page.rotation + 90) % 360) as 0 | 90 | 180 | 270
    void runPageOp(() => rotatePage(doc.id, currentPage, next))
  }, [doc, currentPage, runPageOp])
  const insertBlankAboveCurrent = useCallback(() => {
    void runPageOp(() => insertPage(doc.id, currentPage))
  }, [doc, currentPage, runPageOp])
  // 與 ThumbnailPanel 的縮圖刪除按鈕刻意不同：那裡刪的是滑鼠正懸停的那一頁，目標明確；
  // 這裡刪的是「目前檢視的頁」，按錯的代價一樣（無 undo），但誤觸機率更高，所以多一道確認。
  // ThumbnailPanel 既有行為不動——不一致是刻意的，保護風險較高的這條路徑。
  const deleteCurrentPage = useCallback(() => {
    if (doc.pageCount <= 1) return
    if (!window.confirm(`刪除第 ${currentPage + 1} 頁？此操作無法復原。`)) return
    void runPageOp(() => deletePage(doc.id, currentPage))
  }, [doc, currentPage, runPageOp])
  const openThumbsForReorder = useCallback(() => setShowThumbs(true), [])

  const selectAnnotation = useCallback((page: number, rect: Rect) => {
    flashSeq.current += 1
    setFlash({ page, rect, key: flashSeq.current })
    viewerRef.current?.scrollToRect(page, rect)
  }, [])

  // 表單工具選中、或表單建立模式啟用時，抓一次全文件欄位（含每頁 rect）。
  useEffect(() => {
    if (tool !== 'form' && !formBuilderMode) return
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
        setRedactMode(false)
        setRedactBoxes([])
        setPanMode(false)
        setLinkMode(false)
        setPendingLink(null)
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
        setRedactMode(false)
        setRedactBoxes([])
        setPanMode(false)
        setLinkMode(false)
        setPendingLink(null)
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
        setRedactMode(false)
        setRedactBoxes([])
        setPanMode(false)
        setLinkMode(false)
        setPendingLink(null)
      } else {
        setPendingField(null)
        setEditingField(null)
      }
      return next
    })
  }, [resetImageInteraction])

  const toggleRedact = useCallback(() => {
    setRedactMode((v) => {
      const next = !v
      if (next) {
        setTool('select') // 密文模式時停用其他註解工具，避免 AnnotLayer 搶走指標事件
        setCropMode(false)
        setCropRect(null)
        setImageMode(false)
        resetImageInteraction()
        setFormBuilderMode(false)
        setPendingField(null)
        setEditingField(null)
        setPanMode(false)
        setLinkMode(false)
        setPendingLink(null)
      } else {
        setRedactBoxes([])
      }
      return next
    })
  }, [resetImageInteraction])

  const onRedactAddBox = useCallback((page: number, rectPt: Rect) => {
    setRedactBoxes((prev) => [...prev, { page, x: rectPt.x, y: rectPt.y, w: rectPt.w, h: rectPt.h }])
  }, [])

  const togglePan = useCallback(() => {
    setPanMode((v) => {
      const next = !v
      if (next) {
        setTool('select') // 平移模式時停用其他註解工具，避免 AnnotLayer 搶走指標事件
        setCropMode(false)
        setCropRect(null)
        setImageMode(false)
        resetImageInteraction()
        setFormBuilderMode(false)
        setPendingField(null)
        setEditingField(null)
        setRedactMode(false)
        setRedactBoxes([])
        setLinkMode(false)
        setPendingLink(null)
      }
      return next
    })
  }, [resetImageInteraction])

  const toggleLinkMode = useCallback(() => {
    setLinkMode((v) => {
      const next = !v
      if (next) {
        setTool('select') // 連結模式時停用其他註解工具，避免 AnnotLayer 搶走指標事件
        setCropMode(false)
        setCropRect(null)
        setImageMode(false)
        resetImageInteraction()
        setFormBuilderMode(false)
        setPendingField(null)
        setEditingField(null)
        setRedactMode(false)
        setRedactBoxes([])
        setPanMode(false)
      } else {
        setPendingLink(null)
      }
      return next
    })
  }, [resetImageInteraction])

  // 選工具時一定清掉 pan／連結：ToolRail／AnnotToolbar 以前只 setTool，平移旗標還在，
  // 畫面上看起來選了螢光但其實還在抓頁面拖（checklist B2）。連結模式同理——它的互動層
  // 蓋在 AnnotLayer 上面，不關掉的話選了螢光筆也畫不出東西。
  const selectTool = useCallback((t: AnnotTool) => {
    setPanMode(false)
    setLinkMode(false)
    setPendingLink(null)
    setTool(t)
  }, [])

  // 背景分頁（active=false）根本不掛 keydown listener：否則 N 個開啟的分頁各掛一個，
  // 每次按鍵所有 N 個 handler 都被呼叫（各自再 if(!active) return），純浪費。只有 active
  // 分頁掛；切分頁時 active 在依賴陣列裡，會自動 remove 舊的 listener／add 新的。
  useEffect(() => {
    if (!active) return
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'f') {
        e.preventDefault()
        setShowSearch(true)
      }
      if ((e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 'p') {
        // preventDefault 一定要下：不然瀏覽器自己的列印對話框會跟這裡的 PrintDialog 一起跳出來。
        e.preventDefault()
        toggleDialog('print')
      }
      if (mode === 'local' && (e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === 's') {
        e.preventDefault()
        if (e.shiftKey) void saveAsLocal()
        else void saveLocal()
      }
      if (e.key === 'Escape') {
        // AnnotLayer（select 工具）用 capture 先處理拖曳／清選取，並 preventDefault。
        // 這裡看到 defaultPrevented 就代表已經被接手，不要再往下吃掉 tool/全螢幕。
        if (e.defaultPrevented) return
        // 彈出選單排最前面：它只是個懸浮選單，跟下面任何一個「工具模式」都無關，
        // 先把它收掉就 return，不讓 Escape 繼續往下吃掉 tool/全螢幕。
        if (openMenu !== null) {
          setOpenMenu(null)
          return
        }
        // 對話框（列印／匯出／…）比工具模式優先——checklist A5：Esc 應先關對話框，
        // 不可直接退全螢幕卻把 modal 留著。
        if (activeDialog !== null) {
          setActiveDialog(null)
          return
        }
        if (showSearch) {
          setShowSearch(false)
          return
        }
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
        if (redactMode) {
          setRedactMode(false)
          setRedactBoxes([])
          return
        }
        if (linkMode) {
          // 拉完框、對話框還開著時，先退掉那一步而不是整個模式——跟表單建立的
          // pendingField 同一個道理。
          if (pendingLink) {
            setPendingLink(null)
            return
          }
          setLinkMode(false)
          return
        }
        if (panMode) {
          setPanMode(false)
          return
        }
        // 走到這裡＝上面每個「吃掉 Escape」的分支都沒接手。工具已經是 select 就沒東西
        // 可退，這時 Escape 才輪到「離開全螢幕」。優先權天然正確，因為判斷所需的狀態
        // 全都在本元件手上——不需要把這些旗標鏡射一份到 App.tsx 再讓它猜誰該先跑。
        if (tool === 'select') {
          if (fullscreen) onToggleFullscreen()
          return
        }
        setTool('select')
      }
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [
    active,
    mode,
    tool,
    cropMode,
    imageMode,
    formBuilderMode,
    pendingField,
    editingField,
    redactMode,
    panMode,
    linkMode,
    pendingLink,
    openMenu,
    activeDialog,
    showSearch,
    fullscreen,
    onToggleFullscreen,
    resetImageInteraction,
    saveLocal,
    saveAsLocal,
    toggleDialog,
  ])

  return (
    <div className="app" style={{ display: active ? 'flex' : 'none' }}>
      <ToolTabs active={activeTab} onSelect={setActiveTab} />
      <div className="toolbar-row">
      <Toolbar
        doc={doc}
        mode={mode}
        activeTab={activeTab}
        scale={scale}
        setScale={setScale}
        currentPage={currentPage}
        gotoPage={gotoPage}
        showThumbs={showThumbs}
        toggleThumbs={() => setShowThumbs((v) => !v)}
        showOutline={showOutline}
        toggleOutline={() => setShowOutline((v) => !v)}
        showSearch={showSearch}
        toggleSearch={() => setShowSearch((v) => !v)}
        openFile={onOpenFileNewTab}
        onOpenLocal={onOpenLocalNewTab}
        onSaveLocal={saveLocal}
        onSaveAsLocal={saveAsLocal}
        saveMessage={saveMessage}
        dirty={dirty}
        cropMode={cropMode}
        toggleCrop={toggleCrop}
        imageMode={imageMode}
        toggleImageMode={toggleImageMode}
        formBuilderMode={formBuilderMode}
        toggleFormBuilder={toggleFormBuilder}
        linkMode={linkMode}
        toggleLinkMode={toggleLinkMode}
        activeDialog={activeDialog}
        onToggleDialog={toggleDialog}
        redactMode={redactMode}
        toggleRedact={toggleRedact}
        onRotateDocument={rotateDocument}
        rotatingDocument={rotatingAll}
        panMode={panMode}
        togglePan={togglePan}
        fullscreen={fullscreen}
        onToggleFullscreen={onToggleFullscreen}
        onRotateCurrentPage={rotateCurrentPage}
        onInsertBlankAboveCurrent={insertBlankAboveCurrent}
        onDeleteCurrentPage={deleteCurrentPage}
        pageOpBusy={pageOpBusy}
        onOpenThumbsForReorder={openThumbsForReorder}
        openMenu={openMenu}
        onOpenMenu={setOpenMenu}
      />
      <AnnotToolbar
        tool={tool}
        setTool={selectTool}
        color={color}
        setColor={setColor}
        inkWidth={inkWidth}
        setInkWidth={setInkWidth}
        showAnnotPanel={showAnnotPanel}
        toggleAnnotPanel={() => setShowAnnotPanel((v) => !v)}
        noFormFields={formFieldsLoaded && formFields.length === 0}
        activeTab={activeTab}
        openMenu={openMenu}
        onOpenMenu={setOpenMenu}
      />
      </div>
      {error && <p className="error workspace-error">{error}</p>}
      <div className="workspace">
        {showOutline && (
          <OutlinePanel
            doc={doc}
            currentPage={currentPage}
            gotoPage={gotoPage}
            onChanged={() => setDirty(true)}
            onClose={() => setShowOutline(false)}
          />
        )}
        {showThumbs && (
          <ThumbnailPanel
            doc={doc}
            currentPage={currentPage}
            gotoPage={gotoPage}
            pageVersions={renderVersions}
            onStructureChanged={refreshDocStructure}
            onMerge={() => toggleDialog('merge')}
            onExtract={() => toggleDialog('extract')}
          />
        )}
        <div className="viewer-pane">
          {/* 左側浮動工具條（②c）：常駐所有分頁，蓋在頁面內容左緣。故意放在 Viewer 的
              wrapper 裡而不是整個 .workspace——縮圖面板顯示時它有自己的固定寬度，
              浮動條要貼齊「頁面」的左緣，不是貼齊縮圖面板的左緣。 */}
          <ToolRail
            tool={tool}
            setTool={selectTool}
            color={color}
            setColor={setColor}
            inkWidth={inkWidth}
            setInkWidth={setInkWidth}
            panMode={panMode}
            togglePan={togglePan}
            openMenu={openMenu}
            onOpenMenu={setOpenMenu}
            onOpenAllTools={() => setActiveTab('all')}
            isAllTab={activeTab === 'all'}
          />
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
            redactMode={redactMode}
            redactBoxes={redactBoxes}
            onRedactAddBox={onRedactAddBox}
            panMode={panMode}
            linkMode={linkMode}
            onLinkCreateRect={(page, rect) => setPendingLink({ page, rect })}
            onLinkChanged={bumpPageVersion}
          />
        </div>
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
        {redactMode && (
          <RedactBar
            doc={doc}
            boxes={redactBoxes}
            onClear={() => setRedactBoxes([])}
            onApplied={async (newDocId) => {
              setRedactMode(false)
              setRedactBoxes([])
              await replaceDoc(newDocId)
            }}
            onClose={() => {
              setRedactMode(false)
              setRedactBoxes([])
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
        {pendingLink && (
          <LinkDialog
            doc={doc}
            page={pendingLink.page}
            rectPt={pendingLink.rect}
            onClose={() => setPendingLink(null)}
            onCreated={() => bumpPageVersion(pendingLink.page)}
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
            onChanged={bumpPageVersion}
            onSelect={selectAnnotation}
            onClose={() => setShowAnnotPanel(false)}
          />
        )}
        {tool === 'stamp' && (
          <StampDrawer
            selected={selectedStamp}
            onSelect={setSelectedStamp}
            onClose={() => selectTool('select')}
          />
        )}
        {tool === 'draw' && (
          <DrawingModal
            docId={doc.id}
            page={currentPage}
            onDone={(stamp) => {
              setSelectedStamp(stamp)
              selectTool('stamp')
            }}
            onCancel={() => selectTool('select')}
          />
        )}
        {tool === 'sign' && (
          <SignaturePad
            onDone={(stamp) => {
              setSelectedStamp(stamp)
              selectTool('stamp')
            }}
            onCancel={() => selectTool('select')}
          />
        )}
        {activeDialog === 'export' && <ExportDialog doc={doc} onClose={() => closeDialog('export')} />}
        {activeDialog === 'print' && (
          <PrintDialog doc={doc} mode={mode} onClose={() => closeDialog('print')} />
        )}
        {activeDialog === 'compress' && (
          <CompressDialog
            doc={doc}
            onClose={() => closeDialog('compress')}
            onOpenDoc={async (id) => {
              closeDialog('compress')
              await replaceDoc(id)
            }}
          />
        )}
        {activeDialog === 'ocr' && (
          <OcrDialog
            doc={doc}
            onClose={() => closeDialog('ocr')}
            onOpenDoc={async (id) => {
              closeDialog('ocr')
              await replaceDoc(id)
            }}
          />
        )}
        {activeDialog === 'protect' && (
          <ProtectDialog
            doc={doc}
            onClose={() => closeDialog('protect')}
            onOpenDoc={async (id) => {
              closeDialog('protect')
              await replaceDoc(id)
            }}
          />
        )}
        {activeDialog === 'encrypt' && <EncryptDialog doc={doc} onClose={() => closeDialog('encrypt')} />}
        {/* 浮水印／頁首頁尾是就地改寫本文件（不像壓縮／OCR 產出新文件），套用後
            走 refreshDocStructure 讓每一頁的渲染圖重抓。 */}
        {activeDialog === 'watermark' && (
          <WatermarkDialog
            doc={doc}
            onClose={() => closeDialog('watermark')}
            onApplied={() => void refreshDocStructure()}
          />
        )}
        {activeDialog === 'headerFooter' && (
          <HeaderFooterDialog
            doc={doc}
            onClose={() => closeDialog('headerFooter')}
            onApplied={() => void refreshDocStructure()}
          />
        )}
        {activeDialog === 'actionWizard' && (
          <ActionWizardDialog
            onClose={() => closeDialog('actionWizard')}
            onOpenDoc={async (id) => {
              closeDialog('actionWizard')
              await replaceDoc(id)
            }}
          />
        )}
        {activeDialog === 'compare' && (
          <CompareDialog
            doc={doc}
            onClose={() => closeDialog('compare')}
            onOpenDoc={async (id) => {
              closeDialog('compare')
              await replaceDoc(id)
            }}
          />
        )}
        {activeDialog === 'merge' && (
          <MergeDialog
            doc={doc}
            onClose={() => closeDialog('merge')}
            onOpenDoc={async (id) => {
              closeDialog('merge')
              await replaceDoc(id)
            }}
          />
        )}
        {activeDialog === 'extract' && (
          <ExtractDialog
            doc={doc}
            onClose={() => closeDialog('extract')}
            onOpenDoc={async (id) => {
              closeDialog('extract')
              await replaceDoc(id)
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
