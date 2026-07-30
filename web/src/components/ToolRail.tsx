import type { ReactNode } from 'react'
import ToolMenu, { useToolMenuClose } from './ToolMenu'
import type { Color } from '../api'
import { ColorPalette, InkWidths, type AnnotTool } from './AnnotToolbar'

/** 左側浮動工具條的 8 格。只有 3、5、7 有子選單（右下角三角）。 */
export type RailSlotId =
  | 'pan'
  | 'select'
  | 'highlight'
  | 'note'
  | 'ink'
  | 'freeText'
  | 'sign'
  | 'more'

interface RailSlotDef {
  id: RailSlotId
  icon: string
  title: string
  hasSubmenu: boolean
}

// 圖示固定不變：Adobe 會把最近用過的子工具「升格」成格子圖示，我們刻意不這麼做——
// 使用者已經記住「第 3 格＝螢光」，圖示自己跳掉比多一次點擊更糟。整格反白代表
// 「這個群組裡有工具正在使用中」，不代表「這就是最後選的那個子工具」。
const RAIL_SLOTS: RailSlotDef[] = [
  { id: 'pan', icon: '✋', title: '平移', hasSubmenu: false },
  { id: 'select', icon: '🖱️', title: '選取', hasSubmenu: false },
  { id: 'highlight', icon: '🖍️', title: '螢光標記', hasSubmenu: true },
  { id: 'note', icon: '📝', title: '便籤', hasSubmenu: false },
  { id: 'ink', icon: '✏️', title: '手繪', hasSubmenu: true },
  { id: 'freeText', icon: '🔤', title: '文字框', hasSubmenu: false },
  { id: 'sign', icon: '✍', title: '簽名', hasSubmenu: true },
  { id: 'more', icon: '⋯', title: '所有工具', hasSubmenu: false },
]

const HIGHLIGHT_GROUP: AnnotTool[] = ['highlight', 'underline', 'strikeout', 'squiggly']
const INK_GROUP: AnnotTool[] = ['ink', 'draw']
const SIGN_GROUP: AnnotTool[] = ['sign', 'stamp']

interface Props {
  tool: AnnotTool
  setTool: (t: AnnotTool) => void
  color: Color
  setColor: (c: Color) => void
  inkWidth: number
  setInkWidth: (w: number) => void
  panMode: boolean
  togglePan: () => void
  /** 目前開著的彈出選單 id；**跟頂部工具列的下拉共用同一個 state**（住在
   *  DocumentWorkspace），所以「全 App 同時只開一個」是結構保證，Escape 鏈也只要處理
   *  一個分支。本元件的 id 一律加 `rail:` 前綴，跟工具列的區隔開。 */
  openMenu: string | null
  onOpenMenu: (id: string | null) => void
  /** 點「⋯」：切到頂部「所有工具」頁籤，不是開子選單。 */
  onOpenAllTools: () => void
  /** 目前頂部頁籤是否為「所有工具」，用來反白「⋯」。 */
  isAllTab: boolean
}

function isSlotActive(id: RailSlotId, tool: AnnotTool, panMode: boolean, isAllTab: boolean): boolean {
  switch (id) {
    case 'pan':
      return panMode
    case 'select':
      return tool === 'select'
    case 'highlight':
      return HIGHLIGHT_GROUP.includes(tool)
    case 'note':
      return tool === 'note'
    case 'ink':
      return INK_GROUP.includes(tool)
    case 'freeText':
      return tool === 'freeText'
    case 'sign':
      return SIGN_GROUP.includes(tool)
    case 'more':
      return isAllTab
  }
}

const menuId = (slot: RailSlotId) => `rail:${slot}`

/** 子選單裡的一顆工具鈕：選了就把選單收掉。維持既有的小圖示外觀（不用
 *  `ToolMenuItem` 的整列樣式）——這一步只統一彈出層的機制，不動視覺。
 *  色票與筆寬刻意不走這裡：連續調整時選單不該關。 */
function RailToolBtn({
  label,
  title,
  active,
  onSelect,
}: {
  label: ReactNode
  title: string
  active: boolean
  onSelect: () => void
}) {
  const close = useToolMenuClose()
  return (
    <button
      type="button"
      className={`tb-btn ${active ? 'active' : ''}`}
      title={title}
      onClick={() => {
        onSelect()
        close()
      }}
    >
      {label}
    </button>
  )
}

export default function ToolRail({
  tool,
  setTool,
  color,
  setColor,
  inkWidth,
  setInkWidth,
  panMode,
  togglePan,
  openMenu,
  onOpenMenu,
  onOpenAllTools,
  isAllTab,
}: Props) {
  const handlePrimary = (id: RailSlotId) => {
    if (id === 'pan') {
      togglePan()
      return
    }
    if (id === 'more') {
      onOpenAllTools()
      return
    }
    // 選取／螢光／便籤／手繪／文字框／簽名：跟 AnnotToolbar 的按鈕做一模一樣的事——
    // 純 setTool，不去動 crop/image/redact/pan 等其他模式（那些模式各自的 toggle*
    // 函式才會互踢；這裡沿用既有行為，不新增規則）。
    setTool(id as AnnotTool)
  }

  // 色票／筆寬跟 AnnotToolbar 共用同一個元件（本來兩邊各刻一份）。
  const renderPalette = () => (
    <ColorPalette color={color} setColor={setColor} className="tool-rail-palette" />
  )

  const renderSubmenu = (id: RailSlotId) => {
    if (id === 'highlight') {
      return (
        <>
          <div className="tool-rail-submenu-group">
            <RailToolBtn
              label="U̲"
              title="底線"
              active={tool === 'underline'}
              onSelect={() => setTool('underline')}
            />
            <RailToolBtn
              label="S̶"
              title="刪除線"
              active={tool === 'strikeout'}
              onSelect={() => setTool('strikeout')}
            />
            <RailToolBtn
              label="〜"
              title="波浪線"
              active={tool === 'squiggly'}
              onSelect={() => setTool('squiggly')}
            />
          </div>
          {renderPalette()}
        </>
      )
    }
    if (id === 'ink') {
      return (
        <>
          <div className="tool-rail-submenu-group">
            <RailToolBtn
              label="🎨"
              title="繪圖（Excalidraw）"
              active={tool === 'draw'}
              onSelect={() => setTool('draw')}
            />
          </div>
          {renderPalette()}
          {tool === 'ink' && (
            <InkWidths
              inkWidth={inkWidth}
              setInkWidth={setInkWidth}
              className="tool-rail-submenu-group"
            />
          )}
        </>
      )
    }
    if (id === 'sign') {
      return (
        <div className="tool-rail-submenu-group">
          <RailToolBtn
            label="🖃"
            title="印章"
            active={tool === 'stamp'}
            onSelect={() => setTool('stamp')}
          />
        </div>
      )
    }
    return null
  }

  return (
    <nav className="tool-rail" aria-label="工具面板">
      {RAIL_SLOTS.map((slot) => {
        const active = isSlotActive(slot.id, tool, panMode, isAllTab)
        const primary = (
          <button
            type="button"
            className="tool-rail-btn"
            title={slot.title}
            aria-pressed={active}
            onClick={() => handlePrimary(slot.id)}
          >
            {slot.icon}
          </button>
        )
        if (!slot.hasSubmenu) {
          return (
            <div key={slot.id} className="tool-rail-slot-wrap">
              <div className={`tool-rail-slot ${active ? 'active' : ''}`}>{primary}</div>
            </div>
          )
        }
        // 有子選單的格子：主按鈕與三角合起來才是「觸發鈕」，所以走 renderTrigger 而不是
        // ToolMenu 內建的那顆。點主按鈕仍然直接選工具，只有三角會開關選單。
        return (
          <div key={slot.id} className="tool-rail-slot-wrap">
            <ToolMenu
              id={menuId(slot.id)}
              openId={openMenu}
              onOpenChange={onOpenMenu}
              placement="right"
              popupClassName="tool-rail-submenu"
              renderTrigger={({ open, toggle }) => (
                <div className={`tool-rail-slot ${active ? 'active' : ''}`}>
                  {primary}
                  <button
                    type="button"
                    className="tool-rail-caret"
                    title={`${slot.title}子選單`}
                    aria-haspopup="menu"
                    aria-expanded={open}
                    onClick={toggle}
                  >
                    ◢
                  </button>
                </div>
              )}
            >
              {renderSubmenu(slot.id)}
            </ToolMenu>
          </div>
        )
      })}
    </nav>
  )
}
