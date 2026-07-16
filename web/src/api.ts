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

export type ExportFormat = 'png' | 'jpg' | 'tiff' | 'pptx' | 'docx' | 'xlsx'

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
