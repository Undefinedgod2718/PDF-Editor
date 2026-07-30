import type { ReactNode } from 'react'
import type { Color } from '../api'
import ToolMenu, { ToolMenuItem } from './ToolMenu'
import type { ToolTabId } from './ToolTabs'

export type AnnotTool =
  | 'select'
  | 'highlight'
  | 'underline'
  | 'strikeout'
  | 'squiggly'
  | 'note'
  | 'ink'
  | 'freeText'
  | 'stamp'
  | 'editText'
  | 'editLine'
  | 'draw'
  | 'form'
  | 'sign'

interface Props {
  tool: AnnotTool
  setTool: (t: AnnotTool) => void
  color: Color
  setColor: (c: Color) => void
  inkWidth: number
  setInkWidth: (w: number) => void
  showAnnotPanel: boolean
  toggleAnnotPanel: () => void
  /** 表單欄位已載入且該文件沒有任何欄位時為 true，顯示提示。 */
  noFormFields: boolean
  /** 目前選中的頂部分類頁籤——決定本列哪些按鈕群組要顯示。'all' 時全部顯示並加分類標題。 */
  activeTab: ToolTabId
  /** 彈出選單狀態，跟 Toolbar 與左側浮動條共用（見 DocumentWorkspace 的 openMenu）。 */
  openMenu: string | null
  onOpenMenu: (id: string | null) => void
}

interface ToolDef {
  id: AnnotTool
  icon: string
  title: string
}

// 「註解」分類的 11 個工具依用途分三組收進下拉。攤平時這一組加上 8 個色票就是整條
// 第二列，也是「所有工具」放不下的另一半原因。「選取」與「註解列表」留在外面：前者
// 是所有標註操作的預設起點，後者是面板開關，都不該多一次點擊。
const MARKUP_TOOLS: ToolDef[] = [
  { id: 'highlight', icon: '🖍️', title: '螢光標記' },
  { id: 'underline', icon: 'U̲', title: '底線' },
  { id: 'strikeout', icon: 'S̶', title: '刪除線' },
  { id: 'squiggly', icon: '〜', title: '波浪線' },
]

const INSERT_TOOLS: ToolDef[] = [
  { id: 'note', icon: '📝', title: '便籤' },
  { id: 'freeText', icon: '🔤', title: '文字框' },
  { id: 'stamp', icon: '🖃', title: '印章' },
  { id: 'sign', icon: '✍', title: '簽名' },
]

const DRAW_TOOLS: ToolDef[] = [
  { id: 'ink', icon: '✏️', title: '手繪' },
  { id: 'draw', icon: '🎨', title: '繪圖（Excalidraw）' },
]

// 「編輯」分類：改文件內容本身的工具（另外三個編輯工具「裁切」「影像」「建立表單」在 Toolbar.tsx）。
const EDIT_TOOLS: ToolDef[] = [
  { id: 'editText', icon: '✎', title: '編輯文字' },
  { id: 'editLine', icon: '☰', title: '行編輯' },
  { id: 'form', icon: '📋', title: '表單（填寫）' },
]

// 匯出給 ToolRail（浮動條的螢光／手繪子選單）重用，兩邊共用同一份色票／筆寬定義與同一組
// 轉換函式，避免子選單跟本列的顏色狀態各算各的、悄悄長出兩套邏輯。
export const PALETTE: Color[] = [
  { r: 255, g: 214, b: 0 }, // 黃
  { r: 255, g: 82, b: 82 }, // 紅
  { r: 76, g: 175, b: 80 }, // 綠
  { r: 76, g: 141, b: 255 }, // 藍
  { r: 255, g: 152, b: 0 }, // 橙
  { r: 171, g: 71, b: 188 }, // 紫
  { r: 236, g: 64, b: 122 }, // 粉
  { r: 0, g: 0, b: 0 }, // 黑
]

export const INK_WIDTHS = [1, 2, 4, 8]

export function colorToHex(c: Color): string {
  const h = (n: number) => n.toString(16).padStart(2, '0')
  return `#${h(c.r)}${h(c.g)}${h(c.b)}`
}

export function hexToColor(hex: string): Color {
  const r = parseInt(hex.slice(1, 3), 16)
  const g = parseInt(hex.slice(3, 5), 16)
  const b = parseInt(hex.slice(5, 7), 16)
  return { r, g, b }
}

export function sameColor(a: Color, b: Color): boolean {
  return a.r === b.r && a.g === b.g && a.b === b.b
}

/** 8 個色票＋自訂色，本列與浮動條子選單共用一份。
 *
 *  刻意**不**用 `ToolMenuItem`：選色票不該關掉選單（要連續試顏色）。`ToolMenuItem`
 *  的職責就是「選了就關」，所以這裡直接放原始按鈕。 */
export function ColorPalette({
  color,
  setColor,
  className,
}: {
  color: Color
  setColor: (c: Color) => void
  className?: string
}) {
  return (
    <div className={`toolbar-group annot-palette ${className ?? ''}`}>
      {PALETTE.map((c) => (
        <button
          key={colorToHex(c)}
          type="button"
          className={`swatch ${sameColor(c, color) ? 'active' : ''}`}
          style={{ background: `rgb(${c.r},${c.g},${c.b})` }}
          title={colorToHex(c)}
          onClick={() => setColor(c)}
        />
      ))}
      <input
        type="color"
        className="swatch-picker"
        title="自訂顏色"
        value={colorToHex(color)}
        onChange={(e) => setColor(hexToColor(e.target.value))}
      />
    </div>
  )
}

/** 筆寬選擇，同樣共用；跟色票一樣不關選單。 */
export function InkWidths({
  inkWidth,
  setInkWidth,
  className,
}: {
  inkWidth: number
  setInkWidth: (w: number) => void
  className?: string
}) {
  return (
    <div className={`toolbar-group ${className ?? ''}`}>
      {INK_WIDTHS.map((w) => (
        <button
          key={w}
          type="button"
          className={`tb-btn ${inkWidth === w ? 'active' : ''}`}
          title={`筆寬 ${w}`}
          onClick={() => setInkWidth(w)}
        >
          {w}
        </button>
      ))}
    </div>
  )
}

export default function AnnotToolbar({
  tool,
  setTool,
  color,
  setColor,
  inkWidth,
  setInkWidth,
  showAnnotPanel,
  toggleAnnotPanel,
  noFormFields,
  activeTab,
  openMenu,
  onOpenMenu,
}: Props) {
  const isAll = activeTab === 'all'
  const showAnnotate = isAll || activeTab === 'annotate'
  const showEdit = isAll || activeTab === 'edit'

  const menu = (id: string, label: string, tools: ToolDef[], extra?: ReactNode): ReactNode => (
    <ToolMenu
      id={`tb:${id}`}
      openId={openMenu}
      onOpenChange={onOpenMenu}
      label={label}
      title={label}
      active={tools.some((t) => t.id === tool)}
    >
      {tools.map((t) => (
        <ToolMenuItem
          key={t.id}
          onSelect={() => setTool(t.id)}
          active={tool === t.id}
          title={t.title}
        >
          <span className="tool-menu-icon">{t.icon}</span>
          {t.title}
        </ToolMenuItem>
      ))}
      {extra}
    </ToolMenu>
  )

  return (
    <div className="annot-toolbar">
      {showAnnotate && (
        <div className="tool-section">
          {isAll && <div className="tool-section-label">註解</div>}
          <div className="toolbar-group">
            <button
              className={`tb-btn ${tool === 'select' ? 'active' : ''}`}
              title="選取"
              onClick={() => setTool('select')}
            >
              🖱️
            </button>
            {/* 色票跟著會用到它的兩組走（提案書 ②c：色票不再常駐一整段寬度）；
                筆寬只有真的在手繪時才出現，跟改版前的條件一致。 */}
            {menu('markup', '標記', MARKUP_TOOLS, <ColorPalette color={color} setColor={setColor} />)}
            {menu('insert', '插入', INSERT_TOOLS)}
            {menu(
              'draw',
              '繪圖',
              DRAW_TOOLS,
              <>
                <ColorPalette color={color} setColor={setColor} />
                {tool === 'ink' && <InkWidths inkWidth={inkWidth} setInkWidth={setInkWidth} />}
              </>,
            )}
            <button
              className={`tb-btn ${showAnnotPanel ? 'active' : ''}`}
              title="註解列表"
              onClick={toggleAnnotPanel}
            >
              📋
            </button>
          </div>

          {tool === 'stamp' && (
            <div className="annot-hint">請先在印章庫選擇印章，再於頁面拖曳出印章範圍</div>
          )}
        </div>
      )}

      {showEdit && (
        <div className="tool-section">
          {isAll && <div className="tool-section-label">編輯</div>}
          <div className="toolbar-group">{menu('text', '文字', EDIT_TOOLS)}</div>

          {tool === 'form' && noFormFields && <div className="annot-hint">此文件無表單欄位</div>}
        </div>
      )}
    </div>
  )
}
