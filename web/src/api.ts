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
