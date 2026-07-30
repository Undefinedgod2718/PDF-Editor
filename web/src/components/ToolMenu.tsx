import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'

/** 工具列與左側浮動條共用的彈出選單。
 *
 *  在這之前工具列沒有彈出層，浮動條自己刻了一套（`.tool-rail-submenu` ＋ 它自己的
 *  document mousedown 監聽）。把兩邊統一成同一個元件的理由不只是省程式碼：**開關狀態
 *  只有一份**（`DocumentWorkspace` 的 `openMenu`），所以「同時只能開一個」是結構保證，
 *  Escape 鏈也只要處理一個分支。上一輪就是因為彈出層各有各的關閉路徑，才會出現
 *  「一下 Escape 同時關搜尋又退出全螢幕」那種 bug。
 *
 *  刻意**不**在這裡處理 Escape：`DocumentWorkspace` 的 keydown 鏈已經擁有整個優先順序
 *  （選單 → 對話框 → 搜尋 → 工具模式 → 全螢幕），在這裡再攔一次只會多一個競爭者。
 *
 *  彈出層走 **portal ＋ position: fixed**，不是 `position: absolute`。工具列那一列
 *  （`.toolbar-row`）有 `overflow-x: auto`，而 CSS 規定另一軸的 `visible` 會被算成
 *  `auto`——絕對定位的子元素會被那個捲動容器裁掉，實測只露出 3px。 */

type Placement = 'below' | 'right'

/** 讓選單內的項目關閉自己所屬的選單，不必一路傳 prop。 */
const CloseContext = createContext<() => void>(() => {})

export function useToolMenuClose(): () => void {
  return useContext(CloseContext)
}

interface Props {
  /** 這個選單的識別字，跟 `openId` 比對。整個 App 裡要唯一。 */
  id: string
  openId: string | null
  onOpenChange: (id: string | null) => void
  /** 觸發鈕的文字。給了就用內建的「標籤＋▾」按鈕；要完全自訂就用 `renderTrigger`。 */
  label?: ReactNode
  title?: string
  /** 觸發鈕是否要顯示成作用中（例如群組裡有工具正在使用）。 */
  active?: boolean
  /** 'below' 給頂部工具列，'right' 給左側浮動條。 */
  placement?: Placement
  /** 自訂觸發鈕。浮動條要的是「主按鈕＋右下角小三角」的組合，內建那顆做不出來。 */
  renderTrigger?: (state: { open: boolean; toggle: () => void }) => ReactNode
  /** 彈出層的額外 class，讓兩種情境各自套自己的排版。 */
  popupClassName?: string
  children: ReactNode
}

export default function ToolMenu({
  id,
  openId,
  onOpenChange,
  label,
  title,
  active = false,
  placement = 'below',
  renderTrigger,
  popupClassName,
  children,
}: Props) {
  const open = openId === id
  const wrapRef = useRef<HTMLDivElement | null>(null)
  const popRef = useRef<HTMLDivElement | null>(null)
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null)

  const close = () => onOpenChange(null)
  const toggle = () => onOpenChange(open ? null : id)

  // 依觸發鈕的實際位置算出 fixed 座標，並夾在視窗內（工具列右端的選單不能掉到畫面外）。
  const place = useCallback(() => {
    const anchor = wrapRef.current
    const pop = popRef.current
    if (!anchor || !pop) return
    const a = anchor.getBoundingClientRect()
    const w = pop.offsetWidth
    const h = pop.offsetHeight
    const gap = placement === 'below' ? 4 : 8
    let top = placement === 'below' ? a.bottom + gap : a.top
    let left = placement === 'below' ? a.left : a.right + gap
    left = Math.max(8, Math.min(left, window.innerWidth - w - 8))
    top = Math.max(8, Math.min(top, window.innerHeight - h - 8))
    setPos({ top, left })
  }, [placement])

  // useLayoutEffect：在瀏覽器繪製前就定位好，否則會先閃一下左上角。
  useLayoutEffect(() => {
    if (!open) {
      setPos(null)
      return
    }
    place()
  }, [open, place])

  // 選單開著時捲動或改變視窗大小，fixed 座標就過期了。捲動用 capture 才收得到內層
  // 捲動容器（例如 .toolbar-row 自己橫向捲）的事件。
  useEffect(() => {
    if (!open) return
    const onChange = () => place()
    window.addEventListener('resize', onChange)
    window.addEventListener('scroll', onChange, true)
    return () => {
      window.removeEventListener('resize', onChange)
      window.removeEventListener('scroll', onChange, true)
    }
  }, [open, place])

  // 點選單以外的地方就關。監聽 mousedown 而不是 click，才趕在選單內按鈕的 click 之前
  // 收掉狀態；React 的自動批次讓同一輪的兩次 setState 合出正確的最終值，所以「點同一顆
  // 觸發鈕關閉」與「點另一顆觸發鈕切換」都不會互相打架。
  //
  // 判斷範圍是觸發鈕**與**彈出層兩者——只看彈出層的話，點觸發鈕會先被當成「點外面」而
  // 關閉，接著 click 又把它打開，看起來就是關不掉。彈出層走 portal，不在 wrapper 底下，
  // 所以兩個 ref 都要檢查。
  useEffect(() => {
    if (!open) return
    const onDocMouseDown = (e: MouseEvent) => {
      const t = e.target as Node
      if (wrapRef.current?.contains(t) || popRef.current?.contains(t)) return
      close()
    }
    document.addEventListener('mousedown', onDocMouseDown)
    return () => document.removeEventListener('mousedown', onDocMouseDown)
    // close 每次 render 都是新的函式，但它只用到 onOpenChange，放進依賴會讓監聽反覆
    // 掛卸；直接依賴 onOpenChange 就夠了。
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, onOpenChange])

  return (
    <div className="tool-menu" ref={wrapRef}>
      {renderTrigger ? (
        renderTrigger({ open, toggle })
      ) : (
        <button
          type="button"
          className={`tb-btn tool-menu-trigger ${active || open ? 'active' : ''}`}
          title={title}
          aria-haspopup="menu"
          aria-expanded={open}
          onClick={toggle}
        >
          {label}
          <span className="tool-menu-caret" aria-hidden="true">
            ▾
          </span>
        </button>
      )}
      {open &&
        createPortal(
          <div
            ref={popRef}
            className={`tool-menu-pop ${popupClassName ?? ''}`}
            role="menu"
            // pos 還沒算出來的那一幀先隱形，避免閃到左上角。
            style={pos ? { top: pos.top, left: pos.left } : { visibility: 'hidden' }}
          >
            <CloseContext.Provider value={close}>{children}</CloseContext.Provider>
          </div>,
          document.body,
        )}
    </div>
  )
}

interface ItemProps {
  onSelect: () => void
  title?: string
  active?: boolean
  disabled?: boolean
  children: ReactNode
}

/** 選單內的一列。選了就關——色票那種「要連續調整」的控制項不要用這個元件，
 *  直接放原始按鈕即可（不呼叫 close 就不會關）。 */
export function ToolMenuItem({ onSelect, title, active = false, disabled = false, children }: ItemProps) {
  const close = useToolMenuClose()
  return (
    <button
      type="button"
      role="menuitem"
      className={`tool-menu-item ${active ? 'active' : ''}`}
      title={title}
      disabled={disabled}
      onClick={() => {
        onSelect()
        close()
      }}
    >
      {children}
    </button>
  )
}
