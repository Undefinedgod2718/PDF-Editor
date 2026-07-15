/** 解析「1,3-5」形式的頁碼字串為 0-based 索引陣列。預設去重後排序；`preserveOrder` 時保留輸入順序（區間內仍遞增，重複略過）。maxCount 為 1-based 上限。 */
export function parsePageSpec(
  spec: string,
  maxCount: number,
  opts?: { preserveOrder?: boolean },
): number[] {
  const out: number[] = []
  const seen = new Set<number>()
  const parts = spec
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
  if (parts.length === 0) throw new Error('請輸入頁碼')
  for (const part of parts) {
    const m = part.match(/^(\d+)(?:-(\d+))?$/)
    if (!m) throw new Error(`無法解析：${part}`)
    const a = Number(m[1])
    const b = m[2] ? Number(m[2]) : a
    if (a < 1 || b < 1 || a > maxCount || b > maxCount || a > b) {
      throw new Error(`超出範圍（1-${maxCount}）：${part}`)
    }
    for (let p = a; p <= b; p++) {
      const i = p - 1
      if (seen.has(i)) continue
      seen.add(i)
      out.push(i)
    }
  }
  if (opts?.preserveOrder) return out
  return out.sort((x, y) => x - y)
}
