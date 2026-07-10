import type { CharBox, Rect } from '../api'

/** 兩個矩形（同座標系）是否相交。 */
export function rectsIntersect(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y
}

/**
 * 依拖曳選取矩形（PDF points 座標）從頁面字元列表找出相交字元，
 * 再依「同行判定：y 中心差 < 行高一半」把字元合併成每行一個矩形。
 */
export function selectionToLineRects(chars: CharBox[], selection: Rect): Rect[] {
  const hit = chars.filter((c) => rectsIntersect(c, selection))
  if (hit.length === 0) return []

  hit.sort((a, b) => a.y - b.y || a.x - b.x)

  const lines: CharBox[][] = []
  for (const c of hit) {
    const center = c.y + c.h / 2
    let line = lines.find((l) => {
      const ref = l[0]
      const refCenter = ref.y + ref.h / 2
      const lineHeight = Math.max(ref.h, c.h)
      return Math.abs(center - refCenter) < lineHeight / 2
    })
    if (!line) {
      line = []
      lines.push(line)
    }
    line.push(c)
  }

  return lines.map((line) => {
    const minX = Math.min(...line.map((c) => c.x))
    const maxX = Math.max(...line.map((c) => c.x + c.w))
    const minY = Math.min(...line.map((c) => c.y))
    const maxY = Math.max(...line.map((c) => c.y + c.h))
    return { x: minX, y: minY, w: maxX - minX, h: maxY - minY }
  })
}
