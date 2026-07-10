export interface PageInfo {
  index: number
  width: number
  height: number
}

export interface DocInfo {
  id: string
  filename: string
  size: number
  pageCount: number
  title: string | null
  pages: PageInfo[]
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

/** 後端 GET /annotations 回傳的單筆註解摘要（type 為 PDFium 的大寫命名，如 "Highlight"）。 */
export interface AnnotationInfo {
  index: number
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
  return version ? `${base}&v=${version}` : base
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
): Promise<{ count: number }> {
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
  index: number,
): Promise<{ ok: boolean }> {
  const res = await fetch(`/api/documents/${id}/pages/${page}/annotations/${index}`, {
    method: 'DELETE',
  })
  return jsonOrThrow(res)
}
