import { renderUrl, type DocInfo } from '../api'

interface Props {
  doc: DocInfo
  currentPage: number
  gotoPage: (p: number) => void
}

const THUMB_SCALE = 0.25

export default function ThumbnailPanel({ doc, currentPage, gotoPage }: Props) {
  return (
    <div className="thumb-panel">
      {doc.pages.map((page) => (
        <div
          key={page.index}
          className={`thumb ${page.index === currentPage ? 'active' : ''}`}
          onClick={() => gotoPage(page.index)}
        >
          <img
            src={renderUrl(doc.id, page.index, THUMB_SCALE)}
            width={page.width * THUMB_SCALE}
            height={page.height * THUMB_SCALE}
            loading="lazy"
            alt={`第 ${page.index + 1} 頁`}
            draggable={false}
          />
          <span className="thumb-label">{page.index + 1}</span>
        </div>
      ))}
    </div>
  )
}
