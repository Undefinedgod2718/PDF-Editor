import { downloadUrl, type DocInfo } from '../api'

interface Props {
  doc: DocInfo
  scale: number
  setScale: (s: number) => void
  currentPage: number
  gotoPage: (p: number) => void
  showThumbs: boolean
  toggleThumbs: () => void
  showSearch: boolean
  toggleSearch: () => void
  openFile: (f: File) => void
  cropMode: boolean
  toggleCrop: () => void
  imageMode: boolean
  toggleImageMode: () => void
  showExport: boolean
  toggleExport: () => void
  showCompress: boolean
  toggleCompress: () => void
  showProtect: boolean
  toggleProtect: () => void
  showEncrypt: boolean
  toggleEncrypt: () => void
}

const ZOOM_STEPS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3, 4]

export default function Toolbar({
  doc,
  scale,
  setScale,
  currentPage,
  gotoPage,
  showThumbs,
  toggleThumbs,
  showSearch,
  toggleSearch,
  openFile,
  cropMode,
  toggleCrop,
  imageMode,
  toggleImageMode,
  showExport,
  toggleExport,
  showCompress,
  toggleCompress,
  showProtect,
  toggleProtect,
  showEncrypt,
  toggleEncrypt,
}: Props) {
  const zoomIn = () => {
    const next = ZOOM_STEPS.find((z) => z > scale + 0.001)
    if (next) setScale(next)
  }
  const zoomOut = () => {
    const next = [...ZOOM_STEPS].reverse().find((z) => z < scale - 0.001)
    if (next) setScale(next)
  }

  return (
    <div className="toolbar">
      <div className="toolbar-group">
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
        <button
          className={`tb-btn ${showThumbs ? 'active' : ''}`}
          title="頁面縮圖"
          onClick={toggleThumbs}
        >
          🗂
        </button>
      </div>

      <div className="toolbar-group toolbar-title" title={doc.filename}>
        {doc.title || doc.filename}
      </div>

      <div className="toolbar-group">
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
      </div>

      <div className="toolbar-group">
        <button
          className={`tb-btn ${showSearch ? 'active' : ''}`}
          title="搜尋 (Ctrl+F)"
          onClick={toggleSearch}
        >
          🔍
        </button>
        <button className={`tb-btn ${cropMode ? 'active' : ''}`} title="裁切" onClick={toggleCrop}>
          裁切
        </button>
        <button className={`tb-btn ${imageMode ? 'active' : ''}`} title="影像" onClick={toggleImageMode}>
          影像
        </button>
        <button className={`tb-btn ${showExport ? 'active' : ''}`} title="匯出" onClick={toggleExport}>
          匯出
        </button>
        <button className={`tb-btn ${showCompress ? 'active' : ''}`} title="壓縮" onClick={toggleCompress}>
          壓縮
        </button>
        <button className={`tb-btn ${showProtect ? 'active' : ''}`} title="保護" onClick={toggleProtect}>
          保護
        </button>
        <button className={`tb-btn ${showEncrypt ? 'active' : ''}`} title="加密" onClick={toggleEncrypt}>
          加密
        </button>
      </div>
    </div>
  )
}
