/**
 * 写值统一入口（E2：写值面板 / 确认 / 结果解析的唯一实现）
 *
 * 背景：`POST /api/points/write` 单条与多条的分派由后端决定，但四个写入口
 * （点位读写测试、批量写入卡片、远程控制、批量命令）各自解析响应：
 * 有的假设「总有 results[0]」，有的把 5101 的 reject 当另一种错误层层 catch。
 * 这里统一成：
 *   - `writeBatch` 永不抛异常，传输失败也折算成「全部失败」的结果结构；
 *   - `writeOnce` 只关心第 1 条，供单点写页面使用；
 *   - `confirmPointWrite` 统一确认文案与按钮，避免同一动作多个措辞。
 *
 * 权限与审计不在前端：所有写入都经后端统一入口，单写走 writable 白名单 + 审计。
 */
import { confirmAction } from '@/utils/confirm'
import { extractErrorMessage } from '@/utils/errorHandler'
import { writePoints, type BatchWriteResultItem, type PointWriteEntry } from '@/api/points'

/** 批量写结果；始终与入参等长（传输失败时逐条补 null） */
export interface PointWriteOutcome {
  ok: boolean
  status: string
  total: number
  succeeded: number
  failed: number
  results: (BatchWriteResultItem | null)[]
  /** 请求级失败原因（传输错误 / 业务码 reject）；单条失败看 results */
  error: string
}

export interface PointWriteOnceOutcome {
  ok: boolean
  status: string
  value: number | null
  error: string
}

/**
 * 统一写值确认。
 *
 * 单点写明「哪个点位写成什么」，批量写只写明条数——批量表格里逐条列出的
 * 点位/值比把 20 条拼进弹窗更可读。
 */
export async function confirmPointWrite(
  entries: PointWriteEntry[],
  label?: string,
): Promise<boolean> {
  const count = entries.length
  const first = entries[0]
  const message =
    count === 0
      ? ''
      : count === 1 && first
        ? `确认向点位「${label || first.id}」写入值 ${first.value}？`
        : `确认向 ${count} 个点位批量写入目标值？`
  if (!message) return false
  return confirmAction({
    message,
    title: count === 1 ? '写入确认' : '批量写入确认',
    confirmText: '确认写入',
  })
}

export function usePointWrite() {
  /** 批量写；无论成功失败都返回结构化结果，不抛异常 */
  async function writeBatch(entries: PointWriteEntry[]): Promise<PointWriteOutcome> {
    const total = entries.length
    try {
      const resp = await writePoints(entries)
      return {
        ok: resp.failed === 0,
        status: resp.status,
        total: resp.total,
        succeeded: resp.succeeded,
        failed: resp.failed,
        results: resp.results,
        error: '',
      }
    } catch (e) {
      // 单写失败走业务码 5101（reject）；批量请求级失败也走这里
      return {
        ok: false,
        status: 'error',
        total,
        succeeded: 0,
        failed: total,
        results: entries.map(() => null),
        error: extractErrorMessage(e, '写入失败'),
      }
    }
  }

  /** 单点写；只看第 1 条结果，语义与批量一致 */
  async function writeOnce(id: string, value: number): Promise<PointWriteOnceOutcome> {
    const outcome = await writeBatch([{ id, value }])
    const item = outcome.results[0]
    const ok = item?.status === 'ok'
    return {
      ok,
      status: item?.status ?? outcome.status,
      value: ok ? (item?.value ?? value) : null,
      error: ok ? '' : item?.error || outcome.error || '写入失败',
    }
  }

  return { writeBatch, writeOnce }
}
