import type { ReactNode } from 'react'
import { downloadUrl, type AppMode, type DocInfo } from '../api'
import ToolMenu, { ToolMenuItem } from './ToolMenu'
import type { ToolTabId } from './ToolTabs'

/** 文件工具下拉裡「開對話框」類的項目；同時最多一個開著，由 DocumentWorkspace 的
 *  activeDialog 統一管理。redactMode 不算在內——它是工具模式，不是對話框。 */
export type DialogKind =
  | 'export'
  | 'print'
  | 'compress'
  | 'ocr'
  | 'protect'
  | 'encrypt'
  | 'actionWizard'
  | 'compare'
  | 'merge'
  | 'extract'
  | 'watermark'
  | 'headerFooter'

interface Props {
  doc: DocInfo
  mode: AppMode
  /** 目前選中的頂部分類頁籤——決定本列哪些按鈕群組要顯示。'all' 時全部顯示並加分類標題。 */
  activeTab: ToolTabId
  scale: number
  setScale: (s: number) => void
  currentPage: number
  gotoPage: (p: number) => void
  showThumbs: boolean
  toggleThumbs: () => void
  showOutline: boolean
  toggleOutline: () => void
  showSearch: boolean
  toggleSearch: () => void
  openFile: (f: File) => void
  onOpenLocal: () => void
  onSaveLocal: () => void
  onSaveAsLocal: () => void
  saveMessage: string | null
  dirty: boolean
  cropMode: boolean
  toggleCrop: () => void
  imageMode: boolean
  toggleImageMode: () => void
  formBuilderMode: boolean
  toggleFormBuilder: () => void
  linkMode: boolean
  toggleLinkMode: () => void
  activeDialog: DialogKind | null
  onToggleDialog: (kind: DialogKind) => void
  redactMode: boolean
  toggleRedact: () => void
  onRotateDocument: () => void
  rotatingDocument: boolean
  panMode: boolean
  togglePan: () => void
  fullscreen: boolean
  onToggleFullscreen: () => void
  /** 頁面操作（②d）：作用在 currentPage 上的單頁旋轉／插入／刪除，以及跳去縮圖面板做重新排序。 */
  onRotateCurrentPage: () => void
  onInsertBlankAboveCurrent: () => void
  onDeleteCurrentPage: () => void
  pageOpBusy: boolean
  onOpenThumbsForReorder: () => void
  /** 彈出選單狀態，跟左側浮動條共用（見 DocumentWorkspace 的 openMenu）。 */
  openMenu: string | null
  onOpenMenu: (id: string | null) => void
}

const ZOOM_STEPS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4]

/** 「轉換▾」底下的對話框——其中任何一個開著，觸發鈕就要反白。 */
const CONVERT_DIALOGS: DialogKind[] = ['export', 'print', 'compress', 'ocr', 'actionWizard']

export default function Toolbar({
  doc,
  mode,
  activeTab,
  scale,
  setScale,
  currentPage,
  gotoPage,
  showThumbs,
  toggleThumbs,
  showOutline,
  toggleOutline,
  showSearch,
  toggleSearch,
  openFile,
  onOpenLocal,
  onSaveLocal,
  onSaveAsLocal,
  saveMessage,
  dirty,
  cropMode,
  toggleCrop,
  imageMode,
  toggleImageMode,
  formBuilderMode,
  toggleFormBuilder,
  linkMode,
  toggleLinkMode,
  activeDialog,
  onToggleDialog,
  redactMode,
  toggleRedact,
  onRotateDocument,
  rotatingDocument,
  panMode,
  togglePan,
  fullscreen,
  onToggleFullscreen,
  onRotateCurrentPage,
  onInsertBlankAboveCurrent,
  onDeleteCurrentPage,
  pageOpBusy,
  onOpenThumbsForReorder,
  openMenu,
  onOpenMenu,
}: Props) {
  const zoomIn = () => {
    const next = ZOOM_STEPS.find((z) => z > scale + 0.001)
    if (next) setScale(next)
  }
  const zoomOut = () => {
    const next = [...ZOOM_STEPS].reverse().find((z) => z < scale - 0.001)
    if (next) setScale(next)
  }

  // 所有工具頁籤：五個分類全部顯示，各自掛上標題方便瀏覽。單一頁籤時（例如「檢視」）
  // 頁籤列本身已經說明了情境，不必再重複一次分類名稱。
  // 收進下拉的原則：**模式型**工具（點了滑鼠行為就變、需要一直看得到選中狀態）留裸按鈕，
  // 一次性動作與開對話框的收進來。這樣一列的格數從 38 降到 20 上下，正常視窗寬度放得下，
  // 不必再靠橫向捲動（見 .toolbar-row 的註解）。
  const menu = (id: string, label: string, active: boolean, children: ReactNode): ReactNode => (
    <ToolMenu
      id={`tb:${id}`}
      openId={openMenu}
      onOpenChange={onOpenMenu}
      label={label}
      title={label}
      active={active}
    >
      {children}
    </ToolMenu>
  )

  const isAll = activeTab === 'all'
  const section = (
    id: Exclude<ToolTabId, 'all'>,
    label: string,
    content: ReactNode,
  ): ReactNode => {
    if (!isAll && activeTab !== id) return null
    return (
      <div className="tool-section" key={id}>
        {isAll && <div className="tool-section-label">{label}</div>}
        {content}
      </div>
    )
  }

  return (
    <div className="toolbar">
      {/* 常駐區：開啟/儲存/另存（桌面版）或開啟/下載（web 版）、檔名、搜尋、全螢幕——
          不管切到哪個分類頁籤都要看得到。 */}
      <div className="toolbar-group">
        {mode === 'local' ? (
          <>
            <button className="tb-btn" title="開啟檔案" onClick={onOpenLocal}>
              📂
            </button>
            <button
              className="tb-btn"
              title={dirty ? '存檔 (Ctrl+S) — 有未存修改' : '存檔 (Ctrl+S)'}
              onClick={onSaveLocal}
            >
              💾{dirty && <span className="dirty-dot">●</span>}
            </button>
            <button className="tb-btn" title="另存新檔 (Ctrl+Shift+S)" onClick={onSaveAsLocal}>
              💾+
            </button>
          </>
        ) : (
          <>
            <label className="tb-btn" title="開啟檔案">
              📂
              <input
                type="file"
                accept="application/pdf"
                hidden
                onChange={(e) => {
                  const f = e.target.files?.[0]
                  if (f) openFile(f)
                }}
              />
            </label>
            <a className="tb-btn" title="下載" href={downloadUrl(doc.id)}>
              💾
            </a>
          </>
        )}
      </div>

      <div className="toolbar-group toolbar-title" title={doc.filename}>
        {doc.title || doc.filename}
        {saveMessage && <span className="save-message"> · {saveMessage}</span>}
      </div>

      <div className="toolbar-group">
        <button
          className={`tb-btn ${showSearch ? 'active' : ''}`}
          title="搜尋 (Ctrl+F)"
          onClick={toggleSearch}
        >
          🔍
        </button>
        <button
          className={`tb-btn ${fullscreen ? 'active' : ''}`}
          title={fullscreen ? '退出全螢幕 (Esc / F11)' : '全螢幕閱讀 (F11)'}
          onClick={onToggleFullscreen}
        >
          ⛶
        </button>
      </div>

      {/* ---- 檢視 ---- */}
      {section(
        'view',
        '檢視',
        <>
          <div className="toolbar-group">
            <button
              className={`tb-btn ${showThumbs ? 'active' : ''}`}
              title="頁面縮圖"
              onClick={toggleThumbs}
            >
              🗂
            </button>
            <button
              className={`tb-btn ${showOutline ? 'active' : ''}`}
              title="書籤（大綱）"
              onClick={toggleOutline}
            >
              🔖
            </button>
            <button
              className="tb-btn"
              title="上一頁"
              disabled={currentPage <= 0}
              onClick={() => gotoPage(currentPage - 1)}
            >
              ▲
            </button>
            <span className="page-indicator">
              <input
                type="number"
                min={1}
                max={doc.pageCount}
                value={currentPage + 1}
                onChange={(e) => {
                  const p = Number(e.target.value) - 1
                  if (p >= 0 && p < doc.pageCount) gotoPage(p)
                }}
              />
              / {doc.pageCount}
            </span>
            <button
              className="tb-btn"
              title="下一頁"
              disabled={currentPage >= doc.pageCount - 1}
              onClick={() => gotoPage(currentPage + 1)}
            >
              ▼
            </button>
          </div>
          <div className="toolbar-group">
            <button className="tb-btn" title="縮小" onClick={zoomOut}>
              −
            </button>
            <span className="zoom-indicator">{Math.round(scale * 100)}%</span>
            <button className="tb-btn" title="放大" onClick={zoomIn}>
              ＋
            </button>
            <button
              className={`tb-btn ${panMode ? 'active' : ''}`}
              title="平移（拖曳捲動）"
              onClick={togglePan}
            >
              ✋
            </button>
          </div>
          {/* 導覽與縮放是高頻操作，留裸按鈕；其餘三項一次性動作收進下拉。全螢幕在頁籤外
              已經常駐一顆，這裡是規劃書指定的第二個入口，同一個 handler。 */}
          <div className="toolbar-group">
            {menu(
              'view',
              '檢視',
              activeDialog === 'compare',
              <>
                <ToolMenuItem
                  onSelect={onRotateDocument}
                  disabled={rotatingDocument}
                  title="向右旋轉整份文件 90°"
                >
                  ↻ 旋轉整份文件
                </ToolMenuItem>
                <ToolMenuItem onSelect={onToggleFullscreen} active={fullscreen}>
                  ⛶ {fullscreen ? '退出全螢幕 (Esc / F11)' : '全螢幕閱讀 (F11)'}
                </ToolMenuItem>
                <ToolMenuItem
                  onSelect={() => onToggleDialog('compare')}
                  active={activeDialog === 'compare'}
                >
                  比較文件…
                </ToolMenuItem>
              </>,
            )}
          </div>
        </>,
      )}

      {/* ---- 編輯（本檔案負責的部分：影像/裁切/建立表單/頁面操作；編輯文字/行編輯/表單填寫在 AnnotToolbar） ---- */}
      {section(
        'edit',
        '編輯',
        <>
          <div className="toolbar-group">
            <button className={`tb-btn ${cropMode ? 'active' : ''}`} title="裁切" onClick={toggleCrop}>
              裁切
            </button>
            <button
              className={`tb-btn ${imageMode ? 'active' : ''}`}
              title="影像"
              onClick={toggleImageMode}
            >
              影像
            </button>
            <button
              className={`tb-btn ${formBuilderMode ? 'active' : ''}`}
              title="建立表單"
              onClick={toggleFormBuilder}
            >
              📝
            </button>
            {/* 連結是模式型工具（進入後在頁面上拉框），照這一列的慣例留裸按鈕。 */}
            <button
              className={`tb-btn ${linkMode ? 'active' : ''}`}
              title="連結（拉框建立跳頁或網址連結）"
              onClick={toggleLinkMode}
            >
              🔗
            </button>
          </div>
          <div className="toolbar-group">
            {menu(
              'stamptext',
              '浮水印/頁首頁尾',
              activeDialog === 'watermark' || activeDialog === 'headerFooter',
              <>
                <ToolMenuItem
                  onSelect={() => onToggleDialog('watermark')}
                  active={activeDialog === 'watermark'}
                >
                  浮水印…
                </ToolMenuItem>
                <ToolMenuItem
                  onSelect={() => onToggleDialog('headerFooter')}
                  active={activeDialog === 'headerFooter'}
                  title="頁首頁尾與自動頁碼"
                >
                  頁首頁尾與頁碼…
                </ToolMenuItem>
              </>,
            )}
          </div>
          {/* 頁面操作（②d）：目前 ThumbnailPanel 內縮圖按鈕是唯一入口，關掉縮圖面板就完全碰不到——
              這裡補一份作用在 currentPage 上的入口。重排維持拖放縮圖，這裡只負責開面板。
              六個標籤全是長中文字，攤平時就是把這一列撐爆的主因，所以整組收進下拉。 */}
          <div className="toolbar-group">
            {menu(
              'pageops',
              '頁面操作',
              activeDialog === 'merge' || activeDialog === 'extract',
              <>
                <ToolMenuItem
                  onSelect={onRotateCurrentPage}
                  disabled={pageOpBusy}
                  title="旋轉本頁 90°"
                >
                  旋轉本頁
                </ToolMenuItem>
                <ToolMenuItem onSelect={onInsertBlankAboveCurrent} disabled={pageOpBusy}>
                  上方插入空白頁
                </ToolMenuItem>
                <ToolMenuItem
                  onSelect={onDeleteCurrentPage}
                  disabled={pageOpBusy || doc.pageCount <= 1}
                  title={doc.pageCount <= 1 ? '文件只剩一頁，無法刪除' : '刪除本頁'}
                >
                  刪除本頁
                </ToolMenuItem>
                <ToolMenuItem onSelect={onOpenThumbsForReorder} title="拖曳縮圖以重新排序">
                  重新排序…
                </ToolMenuItem>
                <ToolMenuItem
                  onSelect={() => onToggleDialog('merge')}
                  active={activeDialog === 'merge'}
                >
                  合併文件…
                </ToolMenuItem>
                <ToolMenuItem
                  onSelect={() => onToggleDialog('extract')}
                  active={activeDialog === 'extract'}
                >
                  擷取頁面…
                </ToolMenuItem>
              </>,
            )}
          </div>
        </>,
      )}

      {/* ---- 轉換 ---- */}
      {section(
        'convert',
        '轉換',
        <div className="toolbar-group">
          {/* 五項全是開對話框，沒有一個是模式型工具，整組收進下拉沒有損失。 */}
          {menu(
            'convert',
            '轉換',
            CONVERT_DIALOGS.some((d) => d === activeDialog),
            <>
              <ToolMenuItem onSelect={() => onToggleDialog('export')} active={activeDialog === 'export'}>
                匯出…
              </ToolMenuItem>
              <ToolMenuItem
                onSelect={() => onToggleDialog('print')}
                active={activeDialog === 'print'}
                title="列印 (Ctrl+P)"
              >
                列印…
              </ToolMenuItem>
              <ToolMenuItem
                onSelect={() => onToggleDialog('compress')}
                active={activeDialog === 'compress'}
              >
                壓縮…
              </ToolMenuItem>
              <ToolMenuItem
                onSelect={() => onToggleDialog('ocr')}
                active={activeDialog === 'ocr'}
                title="OCR 文字辨識"
              >
                OCR 文字辨識…
              </ToolMenuItem>
              <ToolMenuItem
                onSelect={() => onToggleDialog('actionWizard')}
                active={activeDialog === 'actionWizard'}
                title="動作精靈（批次執行）"
              >
                動作精靈…
              </ToolMenuItem>
            </>,
          )}
        </div>,
      )}

      {/* ---- 保護 ---- */}
      {section(
        'protect',
        '保護',
        <div className="toolbar-group">
          {/* 區域密文是模式型（進入後在頁面上框選），留裸按鈕才看得到自己還在密文模式。 */}
          {menu(
            'protect',
            '保護',
            activeDialog === 'protect' || activeDialog === 'encrypt',
            <>
              <ToolMenuItem
                onSelect={() => onToggleDialog('protect')}
                active={activeDialog === 'protect'}
                title="保護（權限）"
              >
                保護（權限）…
              </ToolMenuItem>
              <ToolMenuItem
                onSelect={() => onToggleDialog('encrypt')}
                active={activeDialog === 'encrypt'}
              >
                加密…
              </ToolMenuItem>
            </>,
          )}
          <button className={`tb-btn ${redactMode ? 'active' : ''}`} title="區域密文（光柵化）" onClick={toggleRedact}>
            區域密文
          </button>
        </div>,
      )}
    </div>
  )
}
