export interface PageInfo {
  index: number
  width: number
  height: number
  rotation: number
}

export interface DocInfo {
  id: string
  filename: string
  size: number
  /** 伺服器端持久化的內容版本號，每次寫入 +1；用於渲染圖 cache-busting。 */
  revision: number
  pageCount: number
  title: string | null
  pages: PageInfo[]
}

/** 寫入類 API 的共同回應：伺服器已把文件 revision +1。 */
export interface Mutated {
  ok: boolean
  revision: number
}

export interface Rect {
  x: number
  y: number
  w: number
  h: number
}

export interface SearchHit {
  page: number
  rects: Rect[]
  excerpt: string
}

export interface Color {
  r: number
  g: number
  b: number
  a?: number
}

export interface Point {
  x: number
  y: number
}

export type AnnotationType =
  | 'highlight'
  | 'underline'
  | 'strikeout'
  | 'squiggly'
  | 'note'
  | 'ink'
  | 'freeText'

export type CreateAnnotationRequest =
  | { type: 'highlight' | 'underline' | 'strikeout' | 'squiggly'; rects: Rect[]; color: Color; contents?: string }
  | { type: 'note'; x: number; y: number; contents: string; color: Color }
  | { type: 'ink'; strokes: Point[][]; color: Color; width: number }
  | { type: 'freeText'; rect: Rect; contents: string; color: Color; fontSize: number }
  | { type: 'stamp'; rect: Rect; stampId: string }

/** 後端 GET /annotations 回傳的單筆註解摘要（type 為 PDFium 的大寫命名，如 "Highlight"）。 */
export interface AnnotationInfo {
  index: number
  /** 穩定 ID（PDF /NM 欄位，UUID）。刪除優先用它；null 只會出現在
   *  導入 /NM 之前建立的舊註解，此時退回用 index。 */
  nm: string | null
  type: string
  rect: Rect | null
  contents: string | null
  /** /T —— 留言作者。 */
  author: string | null
  /**
   * 父註解的 nm（由後端 /IRT 解析而來）；頂層註解為 null。
   * 注意：`pdf-core/src/pdf/annots.rs` 的 `AnnotationInfo` struct 沒有
   * `#[serde(rename_all)]`，這欄跟下面 `reply_type` 在 wire 上是
   * snake_case，不要「順手」改成 camelCase（跟本檔其餘欄位不同），
   * 同樣的坑在上面 `RecentEntry` 也記過一份。
   */
  irt: string | null
  /** /RT —— "R" 表示這是一則回覆；其餘情況為 null。 */
  reply_type: string | null
}

export interface CharBox extends Rect {
  c: string
}

export interface PageText {
  text: string
  chars: CharBox[]
}

/** 文件在文件庫中的簡要中繼資料（上傳／合併／擷取回應共用）。 */
export interface DocMeta {
  id: string
  filename: string
  size: number
  revision: number
}

/** 印章庫項目。 */
export interface StampMeta {
  id: string
  filename: string
  width: number
  height: number
}

/** 頁面文字物件（可編輯/刪除），index 為該頁全物件集合中的位置，增刪後需重新 GET。 */
export interface TextObjectInfo {
  index: number
  text: string
  x: number
  y: number
  w: number
  h: number
  font_size: number
}

async function jsonOrThrow<T>(res: Response): Promise<T> {
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(body.error ?? res.statusText)
  }
  return res.json()
}

export async function uploadPdf(file: File): Promise<{ id: string }> {
  const form = new FormData()
  form.append('file', file)
  const res = await fetch('/api/documents', { method: 'POST', body: form })
  return jsonOrThrow(res)
}

export async function fetchDocInfo(id: string): Promise<DocInfo> {
  const res = await fetch(`/api/documents/${id}/info`)
  return jsonOrThrow(res)
}

export function renderUrl(id: string, page: number, scale: number, version?: number): string {
  const base = `/api/documents/${id}/pages/${page}/render?scale=${scale.toFixed(3)}`
  // 注意 version 可以是 0（新文件 revision 0），不能用 truthy 判斷，
  // 否則第一版渲染會退回 no-store 而失去快取。
  return version !== undefined ? `${base}&v=${version}` : base
}

export async function searchDoc(id: string, q: string): Promise<SearchHit[]> {
  const res = await fetch(`/api/documents/${id}/search?q=${encodeURIComponent(q)}`)
  return jsonOrThrow(res)
}

export function downloadUrl(id: string): string {
  return `/api/documents/${id}/download`
}

export async function fetchPageText(id: string, page: number): Promise<PageText> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/text`)
  return jsonOrThrow(res)
}

export async function listAnnotations(id: string, page: number): Promise<AnnotationInfo[]> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/annotations`)
  return jsonOrThrow(res)
}

export async function createAnnotation(
  id: string,
  page: number,
  body: CreateAnnotationRequest,
): Promise<{ count: number; revision: number }> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/annotations`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

export async function deleteAnnotation(
  id: string,
  page: number,
  /** 註解的 nm（穩定 ID）；舊註解無 nm 時傳 index 字串。 */
  annotId: string,
): Promise<Mutated> {
  const res = await fetch(
    `/api/documents/${id}/pages/${page}/annotations/${encodeURIComponent(annotId)}`,
    { method: 'DELETE' },
  )
  return jsonOrThrow(res)
}

/** 編輯留言文字／作者／顏色／位置尺寸，至少要帶一個欄位；nm 必須是穩定 ID（沒有 index 退路）。
 *  Stamp（文字框，見 annots.rs 模組說明）一律拒絕 contents/author 編輯並回 400；
 *  color 只有標記類（Highlight/Underline/Strikeout/Squiggly）後端接受，其餘類型
 *  外觀是「畫出來」而非由顏色欄位生成，同樣回 400（見 annots::set_color 註解）。
 *  rect 只有 Ink/Stamp/Text 接受，四種文字標記類（Highlight/Underline/Strikeout/
 *  Squiggly）幾何是貼在文字上的 QuadPoints，一律拒絕並回 400（見 annots::set_rect 註解）。 */
export async function updateAnnotation(
  id: string,
  page: number,
  nm: string,
  body: { contents?: string; author?: string; color?: Color; rect?: Rect },
): Promise<Mutated> {
  const res = await fetch(
    `/api/documents/${id}/pages/${page}/annotations/${encodeURIComponent(nm)}`,
    {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    },
  )
  return jsonOrThrow(res)
}

/** 對指定註解新增一則回覆；回傳新回覆的 nm，用來在下次 GET 前就能定位到它。 */
export async function replyToAnnotation(
  id: string,
  page: number,
  parentNm: string,
  body: { contents: string; author?: string },
): Promise<{ ok: boolean; nm: string; revision: number }> {
  const res = await fetch(
    `/api/documents/${id}/pages/${page}/annotations/${encodeURIComponent(parentNm)}/replies`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    },
  )
  return jsonOrThrow(res)
}

// ---------- 頁面操作（Phase 3）----------

export async function listDocuments(): Promise<DocMeta[]> {
  const res = await fetch('/api/documents')
  return jsonOrThrow(res)
}

export async function rotatePage(id: string, page: number, degrees: 0 | 90 | 180 | 270): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/rotate`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ degrees }),
  })
  return jsonOrThrow(res)
}

/** 旋轉整份文件所有頁面（相對目前各頁角度），一次呼叫、一次寫檔。 */
export async function rotateAllPages(id: string, delta: 90 | -90 | 180): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/rotate-all`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ delta }),
  })
  return jsonOrThrow(res)
}

export async function deletePage(id: string, page: number): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}`, { method: 'DELETE' })
  return jsonOrThrow(res)
}

export async function insertPage(id: string, at: number): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ at }),
  })
  return jsonOrThrow(res)
}

export async function reorderPages(id: string, order: number[]): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/reorder`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ order }),
  })
  return jsonOrThrow(res)
}

export async function mergeDocuments(ids: string[], filename?: string): Promise<DocMeta> {
  const res = await fetch('/api/documents/merge', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ ids, filename }),
  })
  return jsonOrThrow(res)
}

export async function extractPages(id: string, pages: number[], filename?: string): Promise<DocMeta> {
  const res = await fetch(`/api/documents/${id}/extract`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ pages, filename }),
  })
  return jsonOrThrow(res)
}

// ---------- 印章庫（Phase 3）----------

export async function uploadStamp(file: File): Promise<StampMeta> {
  const form = new FormData()
  form.append('file', file)
  const res = await fetch('/api/stamps', { method: 'POST', body: form })
  return jsonOrThrow(res)
}

export async function listStamps(): Promise<StampMeta[]> {
  const res = await fetch('/api/stamps')
  return jsonOrThrow(res)
}

export function stampImageUrl(id: string): string {
  return `/api/stamps/${id}/image`
}

export async function deleteStamp(id: string): Promise<{ ok: boolean }> {
  const res = await fetch(`/api/stamps/${id}`, { method: 'DELETE' })
  return jsonOrThrow(res)
}

// ---------- 文字物件編輯（Phase 3）----------

export async function listPageObjects(id: string, page: number): Promise<TextObjectInfo[]> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/objects`)
  return jsonOrThrow(res)
}

export async function editPageObject(
  id: string,
  page: number,
  index: number,
  text: string,
): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/objects/${index}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ text }),
  })
  return jsonOrThrow(res)
}

export async function deletePageObject(id: string, page: number, index: number): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/objects/${index}`, { method: 'DELETE' })
  return jsonOrThrow(res)
}

// ---------- 行編輯（P15：有限文字編輯，不做 reflow）----------

/** 文字行（同基線的文字物件群組），index 依由上而下排序，任何變更後需重新 GET。 */
export interface LineInfo {
  index: number
  text: string
  x: number
  y: number
  w: number
  h: number
  font_size: number
  /** 行首物件填色 RGBA。 */
  color: [number, number, number, number]
  /** 組成此行的頁面物件 index。 */
  objects: number[]
}

export async function listPageLines(id: string, page: number): Promise<LineInfo[]> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/lines`)
  return jsonOrThrow(res)
}

export async function editPageLine(
  id: string,
  page: number,
  index: number,
  text: string,
): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/lines/${index}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ text }),
  })
  return jsonOrThrow(res)
}

/** 在 after 行下方插入新行，複製其字級/顏色/左緣；shiftDown 先把下方內容下移一行。 */
export async function insertPageLine(
  id: string,
  page: number,
  after: number,
  text: string,
  shiftDown: boolean,
): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/lines`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ after, text, shift_down: shiftDown }),
  })
  return jsonOrThrow(res)
}

/** 垂直平移一行（delta 為點數，正值向下）；andBelow 連同下方所有行一起移。 */
export async function shiftPageLine(
  id: string,
  page: number,
  index: number,
  delta: number,
  andBelow: boolean,
): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/lines/${index}/shift`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ delta, and_below: andBelow }),
  })
  return jsonOrThrow(res)
}

// ---------- 表單填寫（Phase 4）----------

export type FormFieldType =
  | 'Text'
  | 'Checkbox'
  | 'RadioButton'
  | 'ComboBox'
  | 'ListBox'
  | string

/** 表單欄位（GET /api/documents/{id}/form 回傳整份文件的欄位清單）。 */
export interface FormField {
  page: number
  index: number
  name: string
  fieldType: FormFieldType
  value: string | null
  checked: boolean | null
  options: string[] | null
  /** 後端取不到 widget bounds 時為 null，前端須過濾。 */
  rect: Rect | null
  writable: boolean
  /** AcroForm `/Ff` Required bit。 */
  required: boolean
}

export async function fetchDocForm(id: string): Promise<FormField[]> {
  const res = await fetch(`/api/documents/${id}/form`)
  return jsonOrThrow(res)
}

export async function setFormFieldValue(
  id: string,
  page: number,
  index: number,
  body: { value: string } | { checked: boolean },
): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/form/${index}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

// ---------- 表單建立（P14）----------

/** POST .../form 的請求體，比照 annots.ts CreateAnnotationRequest 的 tag 風格。
 *  rect 一律 points、左上原點，與 annotations 相同約定。 */
export type NewFormField =
  | {
      fieldType: 'text'
      name: string
      rect: Rect
      multiline?: boolean
      required?: boolean
      fontSize?: number
      defaultValue?: string
    }
  | { fieldType: 'checkbox'; name: string; rect: Rect; required?: boolean }
  | { fieldType: 'radio'; name: string; options: { value: string; rect: Rect }[]; required?: boolean }
  | { fieldType: 'combobox'; name: string; rect: Rect; options: string[]; required?: boolean }
  | { fieldType: 'listbox'; name: string; rect: Rect; options: string[]; required?: boolean }
  | { fieldType: 'signature'; name: string; rect: Rect }

/** PATCH .../form/{index} 的請求體；至少帶一個鍵。 */
export interface FormFieldUpdate {
  rect?: Rect
  name?: string
  options?: string[]
  required?: boolean
}

export async function createFormField(id: string, page: number, field: NewFormField): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/form`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(field),
  })
  return jsonOrThrow(res)
}

export async function updateFormField(
  id: string,
  page: number,
  index: number,
  update: FormFieldUpdate,
): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/form/${index}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(update),
  })
  return jsonOrThrow(res)
}

export async function deleteFormField(id: string, page: number, index: number): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/form/${index}`, { method: 'DELETE' })
  return jsonOrThrow(res)
}

// ---------- 頁面幾何（Phase 6）----------

export type ResizeMode = 'scale' | 'canvas'

/** 裁切頁面。rect 為 view-space points（已套用旋轉、與渲染畫面一致），null 表示重設為整頁。 */
export async function cropPages(id: string, pages: number[], rect: Rect | null): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/crop`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ pages, rect }),
  })
  return jsonOrThrow(res)
}

/** 調整頁面大小。width/height 為顯示方向下的 points（36–14400）。 */
export async function resizePages(
  id: string,
  pages: number[],
  width: number,
  height: number,
  mode: ResizeMode,
): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/resize`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ pages, width, height, mode }),
  })
  return jsonOrThrow(res)
}

/** 從另一份文件（可為同一份）插入頁面。pages 為來源 0-based 索引，at 為目的地 0-based 插入位置。 */
export async function insertPagesFrom(
  id: string,
  sourceId: string,
  pages: number[],
  at: number,
): Promise<Mutated> {
  const res = await fetch(`/api/documents/${id}/pages/insert-from`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ sourceId, pages, at }),
  })
  return jsonOrThrow(res)
}

// ---------- 影像插入／取代（Phase 7）----------

/** 頁面影像物件（可插入/取代），index 為該頁全物件集合中的位置，增刪後需重新 GET。 */
export interface ImageInfo {
  index: number
  x: number
  y: number
  w: number
  h: number
  pxWidth: number
  pxHeight: number
  filters: string[]
  bitsPerPixel: number | null
}

export async function listPageImages(id: string, page: number): Promise<ImageInfo[]> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/images`)
  return jsonOrThrow(res)
}

/** 插入影像。rect 為 view-space points（左上原點），與裁切/註解座標系一致。 */
export async function insertImage(id: string, page: number, file: File, rect: Rect): Promise<Mutated> {
  const form = new FormData()
  form.append('file', file)
  form.append('x', String(rect.x))
  form.append('y', String(rect.y))
  form.append('w', String(rect.w))
  form.append('h', String(rect.h))
  const res = await fetch(`/api/documents/${id}/pages/${page}/images`, { method: 'POST', body: form })
  return jsonOrThrow(res)
}

/** 取代指定 index 的影像物件。index 為該頁影像清單中的位置，任何頁面變更後需重新 GET。 */
export async function replaceImage(id: string, page: number, index: number, file: File): Promise<Mutated> {
  const form = new FormData()
  form.append('file', file)
  const res = await fetch(`/api/documents/${id}/pages/${page}/images/${index}`, {
    method: 'POST',
    body: form,
  })
  return jsonOrThrow(res)
}

// ---------- 匯出（Phase 8）----------

export type ExportFormat = 'png' | 'jpg' | 'tiff' | 'pptx' | 'docx' | 'xlsx' | 'markdown'

export interface ExportOptions {
  format: ExportFormat
  /** 0-based 頁碼；省略＝全部頁面。 */
  pages?: number[]
  /** docx／xlsx 為文字／表格轉換，後端會忽略此欄位，可省略。 */
  dpi?: number
  /** 僅 format 為 jpg 時後端會讀取；其餘格式可省略。 */
  quality?: number
}

/** 從 Content-Disposition 解析檔名：優先 RFC 5987 的 filename*=UTF-8''<encoded>，
 *  其次退回一般 filename="..."，都沒有就用 export.<fallbackExt>。 */
function parseFilenameFromDisposition(header: string | null, fallbackExt: string): string {
  if (header) {
    const starMatch = header.match(/filename\*\s*=\s*UTF-8''([^;]+)/i)
    if (starMatch) {
      try {
        return decodeURIComponent(starMatch[1].trim())
      } catch {
        // 解碼失敗則退回下面的 plain filename／fallback
      }
    }
    const plainMatch = header.match(/filename\s*=\s*"?([^";]+)"?/i)
    if (plainMatch) return plainMatch[1].trim()
  }
  return `export.${fallbackExt}`
}

/** 匯出文件為圖片／簡報／Office 文件並觸發瀏覽器下載。PNG/JPG 多頁由後端打包成 zip，TIFF 為單一多頁檔，
 *  PPTX 每頁一張投影片，DOCX／XLSX 為文字／表格轉換（不套用 dpi／quality）。 */
export async function exportDocument(id: string, opts: ExportOptions): Promise<void> {
  const body: Record<string, unknown> = { format: opts.format }
  if (opts.pages !== undefined) body.pages = opts.pages
  if (opts.dpi !== undefined) body.dpi = opts.dpi
  if (opts.format === 'jpg' && opts.quality !== undefined) body.quality = opts.quality

  const res = await fetch(`/api/documents/${id}/export`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) {
    const errBody = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(errBody.error ?? res.statusText)
  }
  const blob = await res.blob()
  const filename = parseFilenameFromDisposition(res.headers.get('Content-Disposition'), opts.format)

  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  a.remove()
  URL.revokeObjectURL(url)
}

// ---------- 壓縮（Phase 9）----------

export type CompressPreset = 'screen' | 'ebook' | 'printer' | 'custom'

export interface CompressOptions {
  preset: CompressPreset
  /** 僅 preset 為 custom 時後端會讀取；範圍 36–600。 */
  dpi?: number
  /** 僅 preset 為 custom 時後端會讀取；範圍 10–100。 */
  quality?: number
  /** 可選；省略則後端預設為 compressed_<原檔名>。 */
  filename?: string
}

export interface CompressStats {
  images_recompressed: number
  images_skipped: number
  duplicates_merged: number
  objects_pruned: number
}

export interface CompressResult {
  document: DocMeta
  before: number
  after: number
  stats: CompressStats
}

export async function compressDocument(id: string, opts: CompressOptions): Promise<CompressResult> {
  const body: Record<string, unknown> = { preset: opts.preset }
  if (opts.preset === 'custom') {
    if (opts.dpi !== undefined) body.dpi = opts.dpi
    if (opts.quality !== undefined) body.quality = opts.quality
  }
  if (opts.filename !== undefined) body.filename = opts.filename

  const res = await fetch(`/api/documents/${id}/compress`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

// ---------- OCR ----------

export interface OcrOptions {
  /** Tesseract 語言代碼，如 "eng+chi_tra"；省略則用後端預設。 */
  langs?: string
  /** 辨識解析度 DPI，範圍 36–600；省略則用後端預設（300）。 */
  dpi?: number
  /** 信心分數門檻 0–100，低於此值的字會被捨棄；省略則用後端預設（60）。 */
  minConfidence?: number
  /** 已有文字層的頁面預設會跳過；設 true 強制重新辨識全部頁面。 */
  force?: boolean
  /** 可選；省略則後端預設為 ocr_<原檔名>。 */
  filename?: string
}

export interface OcrStats {
  pages_processed: number
  pages_skipped_existing_text: number
  words_added: number
  words_skipped_low_confidence: number
  words_skipped_no_font: number
  pages_truncated: number
}

export interface OcrResult {
  document: DocMeta
  stats: OcrStats
}

export interface OcrLanguage {
  code: string
  label: string
}

/** 目前伺服器 tessdata 目錄下實際可用的語言——清單隨環境增減，不寫死。 */
export async function fetchOcrLanguages(): Promise<OcrLanguage[]> {
  const res = await fetch('/api/ocr/languages')
  return jsonOrThrow(res)
}

/** 啟動 OCR 背景 job，立刻回傳 job_id；進度用 pollOcrJob 輪詢。 */
export async function startOcrJob(id: string, opts: OcrOptions): Promise<{ job_id: string }> {
  const body: Record<string, unknown> = {}
  if (opts.langs !== undefined) body.langs = opts.langs
  if (opts.dpi !== undefined) body.dpi = opts.dpi
  if (opts.minConfidence !== undefined) body.min_confidence = opts.minConfidence
  if (opts.force !== undefined) body.force = opts.force
  if (opts.filename !== undefined) body.filename = opts.filename

  const res = await fetch(`/api/documents/${id}/ocr`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

export type OcrJobStatus =
  | { status: 'running'; pages_done: number; pages_total: number }
  | ({ status: 'done' } & OcrResult)
  | { status: 'error'; message: string }

export async function pollOcrJob(id: string, jobId: string): Promise<OcrJobStatus> {
  const res = await fetch(`/api/documents/${id}/ocr/jobs/${jobId}`)
  return jsonOrThrow(res)
}

// ---------- 區域密文／光柵化（Phase 12）----------

/** 一個待套用的密文方框：頁面 index + points（top-left origin，同註解慣例）。 */
export interface RedactBox {
  page: number
  x: number
  y: number
  w: number
  h: number
}

export interface RedactOptions {
  /** 光柵化 DPI，範圍 36–600；省略則用後端預設（300）。 */
  dpi?: number
  /** JPEG 品質 10–100；省略則用後端預設（90）。 */
  jpegQuality?: number
  /** 可選；省略則後端預設為 redacted_<原檔名>。 */
  filename?: string
}

export interface RedactStats {
  pages_rasterized: number
  boxes_burned: number
  objects_pruned: number
  struct_elements_removed: number
}

export interface RedactResult {
  document: DocMeta
  stats: RedactStats
}

/** 套用密文成功後，伺服器會刪除套用前的原始文件（見後端 P12 review）——
 *  回傳的新文件是唯一保留下來的版本。 */
export async function redactDocument(
  id: string,
  boxes: RedactBox[],
  opts: RedactOptions = {},
): Promise<RedactResult> {
  const body: Record<string, unknown> = { boxes }
  if (opts.dpi !== undefined) body.dpi = opts.dpi
  if (opts.jpegQuality !== undefined) body.jpeg_quality = opts.jpegQuality
  if (opts.filename !== undefined) body.filename = opts.filename

  const res = await fetch(`/api/documents/${id}/redact`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

// ---------- 保護（Phase 11）----------

export interface PermissionFlags {
  print: boolean
  printHighQuality: boolean
  modify: boolean
  copy: boolean
  copyForAccessibility: boolean
  annotate: boolean
  fillForms: boolean
  assemble: boolean
}

export interface ProtectionStatus {
  protected: boolean
  permissions: PermissionFlags | null
}

export async function getProtectionStatus(id: string): Promise<ProtectionStatus> {
  const res = await fetch(`/api/documents/${id}/protection`)
  return jsonOrThrow(res)
}

export async function protectDocument(
  id: string,
  ownerPassword: string,
  permissions: PermissionFlags,
  filename?: string,
): Promise<{ document: DocMeta }> {
  const body: Record<string, unknown> = { ownerPassword, permissions }
  if (filename !== undefined) body.filename = filename

  const res = await fetch(`/api/documents/${id}/protect`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

export async function unprotectDocument(
  id: string,
  password: string,
  filename?: string,
): Promise<{ document: DocMeta }> {
  const body: Record<string, unknown> = { password }
  if (filename !== undefined) body.filename = filename

  const res = await fetch(`/api/documents/${id}/unprotect`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

// ---------- 密文（Phase 12）----------
//
// 與 Phase 11「保護」不同：這裡是真正的開檔密碼加密，加密後的檔案沒有密碼
// 連本編輯器自己都無法開啟／渲染。因此加密／解密都是「只下載、不存回文件庫」
// 的操作（跟 exportDocument 完全一樣的下載機制），原文件維持不變、仍可檢視。

export interface EncryptOptions {
  userPassword: string
  /** 省略則後端預設同 userPassword。 */
  ownerPassword?: string
  /** 省略則後端預設全部允許。 */
  permissions?: PermissionFlags
  /** 可選；省略則後端預設為 encrypted_<原檔名>。 */
  filename?: string
}

/** 觸發瀏覽器下載一個 blob（沿用 exportDocument 的下載機制）。 */
function triggerDownload(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  document.body.appendChild(a)
  a.click()
  a.remove()
  URL.revokeObjectURL(url)
}

/** 為文件加上開檔密碼並下載加密後的副本；不會存回文件庫，原文件不受影響。 */
export async function encryptDocument(id: string, opts: EncryptOptions): Promise<void> {
  const body: Record<string, unknown> = { userPassword: opts.userPassword }
  if (opts.ownerPassword !== undefined) body.ownerPassword = opts.ownerPassword
  if (opts.permissions !== undefined) body.permissions = opts.permissions
  if (opts.filename !== undefined) body.filename = opts.filename

  const res = await fetch(`/api/documents/${id}/encrypt`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) {
    const errBody = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(errBody.error ?? res.statusText)
  }
  const blob = await res.blob()
  const filename = parseFilenameFromDisposition(res.headers.get('Content-Disposition'), 'pdf')
  triggerDownload(blob, filename)
}

/** 用開檔密碼解密文件並下載解密後的副本；不會存回文件庫，原文件不受影響。 */
export async function decryptDocument(id: string, password: string, filename?: string): Promise<void> {
  const body: Record<string, unknown> = { password }
  if (filename !== undefined) body.filename = filename

  const res = await fetch(`/api/documents/${id}/decrypt`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) {
    const errBody = await res.json().catch(() => ({ error: res.statusText }))
    throw new Error(errBody.error ?? res.statusText)
  }
  const blob = await res.blob()
  const outFilename = parseFilenameFromDisposition(res.headers.get('Content-Disposition'), 'pdf')
  triggerDownload(blob, outFilename)
}

// ---------- 比較（Phase 13）----------
//
// 比較兩份文件的文字內容與視覺（像素）差異，並可選擇串接 LLM 產生自然語言
// 摘要。輸出是「下載即入庫」：一份新文件（新增/刪除/修改處已標註），
// 跟 merge/extract 一樣存回文件庫（不同於 Phase 12 加密的「只下載」，因為
// 這份輸出檔本身仍可正常檢視／編輯）。

export type CompareChangeKind = 'added' | 'deleted'

export interface CompareTextChange {
  kind: CompareChangeKind
  rects: Rect[]
  excerpt: string
}

export interface ComparePageDiff {
  oldPage: number | null
  newPage: number | null
  textChanges: CompareTextChange[]
  visualChanged: boolean
  visualRegions: Rect[]
}

export interface CompareStats {
  pagesAdded: number
  pagesDeleted: number
  pagesModified: number
  textChangesTotal: number
}

export interface CompareReport {
  oldPageCount: number
  newPageCount: number
  pages: ComparePageDiff[]
  stats: CompareStats
  /** LLM 產生的摘要；未設定 ANTHROPIC_API_KEY 或呼叫失敗時為 null。 */
  summary: string | null
}

export interface CompareResult {
  document: DocMeta
  report: CompareReport
}

export interface CompareOptions {
  /** 是否同時執行像素層級的視覺差異比對；預設 true。 */
  visualDiff?: boolean
  /** 是否呼叫 LLM 產生摘要（後端未設金鑰時仍會安全跳過）；預設 true。 */
  llmSummary?: boolean
  /** 可選；省略則後端預設為 compare_<原文件名>_vs_<新文件名>。 */
  filename?: string
}

export async function compareDocuments(
  oldId: string,
  newId: string,
  opts?: CompareOptions,
): Promise<CompareResult> {
  const body: Record<string, unknown> = { oldId, newId }
  if (opts?.visualDiff !== undefined) body.visualDiff = opts.visualDiff
  if (opts?.llmSummary !== undefined) body.llmSummary = opts.llmSummary
  if (opts?.filename !== undefined) body.filename = opts.filename

  const res = await fetch('/api/documents/compare', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

// ---- 桌面本機模式（ADR-002/003/004）----
// `/api/local/*` 只掛在桌面內嵌 axum；web 版一律 404。mode 用 ping 一次
// 偵測結果快取，同一次頁面生命週期不重打。

export type AppMode = 'web' | 'local'

let modePromise: Promise<AppMode> | null = null

/** 偵測執行環境：local build 下 `/api/local/ping` 回 200，web 版 404/網路錯誤都視為 web。 */
export function detectMode(): Promise<AppMode> {
  if (!modePromise) {
    modePromise = fetch('/api/local/ping')
      .then((res): AppMode => (res.ok ? 'local' : 'web'))
      .catch((): AppMode => 'web')
  }
  return modePromise
}

/** 409：存檔時發現原檔在磁碟上已被外部程式改過（mtime 不符）。前端應提示使用者是否強制覆寫。 */
export class SaveConflictError extends Error {}

/** 人操作開檔：跳系統開檔對話框。使用者取消回 null。 */
export async function openDialog(): Promise<DocMeta | null> {
  const res = await fetch('/api/local/open-dialog', { method: 'POST' })
  if (res.status === 204) return null
  return jsonOrThrow(res)
}

/** 已知路徑直接開（檔案關聯/拖放進視窗給的真實路徑）。 */
export async function openByPath(path: string): Promise<DocMeta> {
  const res = await fetch('/api/local/open', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path }),
  })
  return jsonOrThrow(res)
}

/** Ctrl+S：寫回原檔（atomic write-temp-then-rename）。外部改過原檔且 force 為 false 時丟 SaveConflictError。 */
export async function saveDoc(id: string, force = false): Promise<DocMeta> {
  const res = await fetch(`/api/local/documents/${id}/save`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ force }),
  })
  if (res.status === 409) {
    const body = await res.json().catch(() => ({ error: '原檔已被其他程式修改' }))
    throw new SaveConflictError(body.error ?? '原檔已被其他程式修改')
  }
  return jsonOrThrow(res)
}

/** 另存新檔：跳系統另存對話框。使用者取消回 null。 */
export async function saveAsDialog(id: string): Promise<DocMeta | null> {
  const res = await fetch(`/api/local/documents/${id}/save-as-dialog`, { method: 'POST' })
  if (res.status === 204) return null
  return jsonOrThrow(res)
}

/** 另存新檔：已知路徑（測試/自動化入口）。 */
export async function saveAsPath(id: string, path: string): Promise<DocMeta> {
  const res = await fetch(`/api/local/documents/${id}/save-as`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path }),
  })
  return jsonOrThrow(res)
}

/** POST /api/local/print 的請求體（見 desktop/src/local_api.rs::PrintReq）。 */
export interface PrintOptions {
  /** 0-based 頁碼；省略或空陣列＝整份。 */
  pages?: number[]
  /** 是否連註解一起印；省略時後端預設 true（同 Acrobat 的「文件和標記」）。 */
  annotations?: boolean
  /** 指定印表機＝不跳系統列印對話框；給自動化/測試用，UI 一律留空讓使用者自己選印表機。 */
  printer?: string
  /** 搭配 printer 使用；UI 一律留空。 */
  output?: string
}

/** 系統列印（Windows GDI，見 desktop/src/print.rs）。只有桌面版有這支端點，網頁版走
 *  window.print()。回傳實際送出的頁數；0 代表使用者在系統列印對話框按了取消——不是錯誤。 */
export async function printLocal(id: string, opts: PrintOptions = {}): Promise<{ printed: number }> {
  const body: Record<string, unknown> = { docId: id }
  if (opts.pages !== undefined) body.pages = opts.pages
  if (opts.annotations !== undefined) body.annotations = opts.annotations
  if (opts.printer !== undefined) body.printer = opts.printer
  if (opts.output !== undefined) body.output = opts.output

  const res = await fetch('/api/local/print', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

/** dirty = revision != saved_revision，後端直接讀記憶體 meta，不經 PDFium，隨便問都不貴。 */
export async function checkDirty(id: string): Promise<boolean> {
  const res = await fetch(`/api/local/documents/${id}/dirty`)
  const body = await jsonOrThrow<{ dirty: boolean }>(res)
  return body.dirty
}

/** 關窗攔截確認流程最後一步：使用者已經決定（存檔後關／捨棄後關），真的關窗。 */
export async function requestClose(): Promise<void> {
  const res = await fetch('/api/local/close', { method: 'POST' })
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: '關閉視窗失敗' }))
    throw new Error(body.error ?? '關閉視窗失敗')
  }
}

/** 全螢幕（F11／工具列按鈕）：桌面版讓 Rust 端真的把 `"main"` 視窗切全螢幕；
 *  web 版走瀏覽器原生 Fullscreen API（見 App.tsx），不經此端點。 */
export async function setLocalFullscreen(fullscreen: boolean): Promise<void> {
  const res = await fetch('/api/local/fullscreen', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ fullscreen }),
  })
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: '切換全螢幕失敗' }))
    throw new Error(body.error ?? '切換全螢幕失敗')
  }
}

/**
 * `desktop/src/recent.rs` 的 `ClientEntry` 沒有 `#[serde(rename_all)]`，
 * wire 上是 snake_case（`last_opened`／`thumb_key`），跟本檔其餘 camelCase
 * 端點不同——同樣的不一致在上面 StepSecrets／RunActionRequest 也有一份
 * 註解。這裡照抄後端實際欄位名，不要「順手」改成 camelCase，改了會跟
 * 後端對不起來，靜靜地拿到 undefined。
 */
export interface RecentEntry {
  path: string
  filename: string
  last_opened: string
  starred: boolean
  exists: boolean
  size: number | null
  thumb_key: string | null
}

/** 最近使用清單，已依 last_opened 新到舊排序（星號分組是前端的事）。 */
export async function listRecent(): Promise<RecentEntry[]> {
  const res = await fetch('/api/local/recent')
  const body = await jsonOrThrow<{ entries: RecentEntry[] }>(res)
  return body.entries
}

/**
 * 404＝這筆項目已經不在清單裡（例如另一個視窗剛好清過／移除過）。這種
 * 「清單過期」跟真正的失敗（伺服器掛了、連不上）該分開處理：前者只要重抓
 * 清單就對了，後者要讓使用者看見。用獨立的錯誤類別區分，同 SaveConflictError
 * 的作法——丟通用 Error 的話狀態碼就沒了，呼叫端只能一律吞掉，連真故障
 * 也一起靜音。
 */
export class RecentEntryGoneError extends Error {}

async function recentMutate(url: string, body: unknown, fallbackMsg: string): Promise<void> {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (res.ok) return
  const payload = await res.json().catch(() => ({ error: fallbackMsg }))
  const msg = payload.error ?? fallbackMsg
  throw res.status === 404 ? new RecentEntryGoneError(msg) : new Error(msg)
}

export async function setRecentStarred(path: string, starred: boolean): Promise<void> {
  return recentMutate('/api/local/recent/star', { path, starred }, '設定星號失敗')
}

export async function removeRecent(path: string): Promise<void> {
  return recentMutate('/api/local/recent/remove', { path }, '移除項目失敗')
}

/** 只清未加星號的項目；加星號的原地不動（後端行為，見 recent.rs）。 */
export async function clearRecent(): Promise<void> {
  const res = await fetch('/api/local/recent/clear', { method: 'POST' })
  if (!res.ok) {
    const body = await res.json().catch(() => ({ error: '清除清單失敗' }))
    throw new Error(body.error ?? '清除清單失敗')
  }
}

/** 縮圖 URL 建構（給 `<img src>` 用，不是 fetch）。key 後端已限制 16 hex 字元，這裡仍 encode 以防萬一。 */
export function recentThumbUrl(key: string): string {
  return `/api/local/recent/thumb/${encodeURIComponent(key)}`
}

// ---------- 動作精靈（Action Wizard, P17）----------
//
// 每個 Step 的欄位形狀刻意對齊對應單發 endpoint 的 request body（見後端
// server/src/actions.rs 註解），方便共用同一套參數表單。唯一例外是
// StepSecrets／RunActionRequest.stepSecrets：後端該兩個 struct 沒有
// #[serde(rename_all)]，wire 上是 snake_case（owner_password／
// user_password／document_ids／step_secrets），跟其餘 camelCase 的單發
// endpoint（如 protectDocument 的 ownerPassword）不同，這裡照抄後端實際欄位名。

export interface StepRedactBox {
  page: number
  x: number
  y: number
  w: number
  h: number
}

export type Step =
  | { type: 'rotateAll'; delta: number }
  | { type: 'crop'; pages: number[]; rect?: Rect }
  | { type: 'resize'; pages: number[]; width: number; height: number; mode: ResizeMode }
  | { type: 'compress'; preset: CompressPreset; dpi?: number; quality?: number }
  | { type: 'protect'; permissions: PermissionFlags }
  | { type: 'encrypt'; permissions?: PermissionFlags }
  | { type: 'ocr'; langs?: string; dpi?: number; min_confidence?: number; force?: boolean }
  | { type: 'redact'; boxes: StepRedactBox[]; dpi?: number; jpeg_quality?: number }
  | { type: 'export'; format: ExportFormat; dpi?: number; quality?: number }

export interface ActionDef {
  id: string
  name: string
  steps: Step[]
}

/** Run-time-only 密碼，只在 runAction 這次請求帶入，從不隨 ActionDef 存檔。 */
export interface StepSecrets {
  owner_password?: string
  user_password?: string
}

export async function createAction(name: string, steps: Step[]): Promise<ActionDef> {
  const res = await fetch('/api/actions', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ name, steps }),
  })
  return jsonOrThrow(res)
}

export async function listActions(): Promise<ActionDef[]> {
  const res = await fetch('/api/actions')
  return jsonOrThrow(res)
}

export async function getAction(id: string): Promise<ActionDef> {
  const res = await fetch(`/api/actions/${id}`)
  return jsonOrThrow(res)
}

export async function deleteAction(id: string): Promise<{ ok: boolean }> {
  const res = await fetch(`/api/actions/${id}`, { method: 'DELETE' })
  return jsonOrThrow(res)
}

export interface RunActionRequest {
  documentIds: string[]
  /** 只有 Protect/Encrypt 這類 step 才需要，鍵是 steps 陣列的 index。 */
  stepSecrets?: Record<number, StepSecrets>
}

export async function runAction(actionId: string, req: RunActionRequest): Promise<{ run_id: string }> {
  const body: Record<string, unknown> = { document_ids: req.documentIds }
  if (req.stepSecrets !== undefined) body.step_secrets = req.stepSecrets

  const res = await fetch(`/api/actions/${actionId}/run`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  })
  return jsonOrThrow(res)
}

export type ActionRunFileResult =
  | { outcome: 'document'; source_document_id: string; document: DocMeta }
  | {
      outcome: 'exported'
      source_document_id: string
      index: number
      filename: string
      content_type: string
      size: number
    }
  | { outcome: 'failed'; source_document_id: string; step_index: number; message: string }

export type ActionRunStatus =
  | { status: 'running'; current_file: number; total_files: number; current_step: number; total_steps: number }
  | { status: 'done'; results: ActionRunFileResult[] }

export async function pollActionRun(runId: string): Promise<ActionRunStatus> {
  const res = await fetch(`/api/actions/runs/${runId}`)
  return jsonOrThrow(res)
}

/** 下載單一 Exported 結果（index 為 pollActionRun 回傳 results 裡該筆的 index 欄位）。 */
export function actionRunFileUrl(runId: string, index: number): string {
  return `/api/actions/runs/${runId}/files/${index}`
}

/** 打包整批 Exported 結果成 zip；action 沒有 Export step（或全部失敗）時後端回 400。 */
export function actionRunDownloadUrl(runId: string): string {
  return `/api/actions/runs/${runId}/download`
}
