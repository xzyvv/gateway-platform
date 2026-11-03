/**
 * Prometheus 文本暴露格式（text/plain; version=0.0.4）底层分词器
 *
 * 抽出自 api/status.ts 与 MetricsDashboard.vue 中重复的两份行解析逻辑。
 * 两类上层消费输出形状不同，故只共享"逐行分词"，不强行合并成一个函数：
 * - parsePrometheusText（api/status.ts）：扁平聚合为 { 指标名: 数值之和 }，供概览卡片取数
 * - MetricsDashboard.parsePrometheus：保留 HELP/TYPE 元数据与每条样本标签，构建分组视图
 */

/** 单条指标样本（一行 `name{labels} value [timestamp]`） */
export interface PromSample {
  name: string
  labels: Record<string, string>
  value: number
  /** 可选服务端时间戳，分词器原样透传（单位由生产端约定） */
  timestamp?: number
}

export interface PromHelpLine {
  kind: 'help'
  name: string
  text: string
}

export interface PromTypeLine {
  kind: 'type'
  name: string
  metricType: string
}

export interface PromSampleLine {
  kind: 'sample'
  sample: PromSample
}

/** 一行 Prometheus 文本的解析结果；空行与普通注释返回 null */
export type PromLine = PromHelpLine | PromTypeLine | PromSampleLine

// 指标名允许字母/下划线/冒号开头（Prom 标识符规则含冒号，供命名空间使用）
const SAMPLE_RE = /^([a-zA-Z_:][a-zA-Z0-9_:]*)(\{([^}]*)\})?\s+(\S+)(\s+(\S+))?$/
// label 值支持 \" 等转义；转义内容原样保留（与 MetricsDashboard 旧解析器行为一致）
const LABEL_RE = /([a-zA-Z_][a-zA-Z0-9_]*)="((?:[^"\\]|\\.)*)"/g

/**
 * 解析单行文本。
 * - `# HELP name 描述` / `# TYPE name 类型` 返回对应元数据行
 * - `name{label="v"} value [timestamp]` 返回样本行
 * - 空行、普通注释、指标名非法、数值非有限数的行返回 null（跳过而非抛错，
 *   避免服务端输出异常时整个解析失败）
 */
export function parsePrometheusLine(line: string): PromLine | null {
  const t = line.trim()
  if (!t) return null

  if (t.startsWith('#')) {
    // 要求 HELP/TYPE 后必须跟空白，避免把 `# HELPER` 之类误判为元数据；
    // 描述/类型 token 缺失时跳过（与旧 MetricsDashboard 解析器行为一致）
    const helpMatch = t.match(/^#\s+HELP\s+(\S+)\s+(.*)$/)
    if (helpMatch) {
      return { kind: 'help', name: helpMatch[1]!, text: helpMatch[2]! }
    }
    const typeMatch = t.match(/^#\s+TYPE\s+(\S+)\s+(\S+)$/)
    if (typeMatch) {
      return { kind: 'type', name: typeMatch[1]!, metricType: typeMatch[2]! }
    }
    // 其他注释（# EOF 等）跳过
    return null
  }

  const m = SAMPLE_RE.exec(t)
  if (!m) return null
  const value = Number(m[4])
  if (!Number.isFinite(value)) return null

  const labels: Record<string, string> = {}
  const labelsStr = m[3] ?? ''
  if (labelsStr) {
    let lm: RegExpExecArray | null
    LABEL_RE.lastIndex = 0
    while ((lm = LABEL_RE.exec(labelsStr)) !== null) {
      labels[lm[1]!] = lm[2]!
    }
  }

  const sample: PromSample = { name: m[1]!, labels, value }
  if (m[6] !== undefined) {
    const ts = Number(m[6])
    if (Number.isFinite(ts)) sample.timestamp = ts
  }
  return { kind: 'sample', sample }
}

/** 解析整段 Prometheus 文本为行结果数组（顺序保留，非法行已丢弃） */
export function parsePrometheusLines(text: string): PromLine[] {
  const out: PromLine[] = []
  for (const rawLine of String(text ?? '').split(/\r?\n/)) {
    const parsed = parsePrometheusLine(rawLine)
    if (parsed) out.push(parsed)
  }
  return out
}
