// 獨立檔案：只有繪圖模式（DrawingModal）會動態 import 這支模組，
// 讓 Excalidraw 本體與其 CSS 進入獨立 chunk，不拖慢主 bundle。
import { Excalidraw } from '@excalidraw/excalidraw'
import '@excalidraw/excalidraw/index.css'
import type { ExcalidrawImperativeAPI, ExcalidrawInitialDataState } from '@excalidraw/excalidraw/types'

interface Props {
  initialData: ExcalidrawInitialDataState | null
  onApiReady: (api: ExcalidrawImperativeAPI) => void
}

export default function ExcalidrawCanvas({ initialData, onApiReady }: Props) {
  return (
    <Excalidraw
      theme="dark"
      initialData={initialData ?? undefined}
      excalidrawAPI={onApiReady}
    />
  )
}
