import { describe, it, expect } from 'vitest'
import { buildDiagnosticText, buildDiagnosticReport } from './diagnosticReport'

/**
 * 一键复制诊断文本的契约：
 *   - 有 traceId 时必须出现，并提示研发可据此定位日志
 *   - 缺省字段不输出空行，避免用户复制到一堆「undefined」
 */
describe('buildDiagnosticText', () => {
  it('完整凭据：包含时间/请求/状态/错误码/耗时/traceId/错误/页面', () => {
    const text = buildDiagnosticText({
      time: new Date(2026, 9, 24, 23, 40, 12).getTime(),
      method: 'POST',
      url: '/api/config',
      status: 500,
      code: 9999,
      duration: 120,
      traceId: '18f2a1b3c4d5000001',
      error: '网关内部错误',
      page: '/system/config',
    })
    // formatDateTime 走 toLocaleString('zh-CN')，分隔符随运行环境不同，只断言关键时间部分
    expect(text).toContain('2026')
    expect(text).toContain('23:40:12')
    expect(text).toContain('POST /api/config')
    expect(text).toContain('错误码: 9999')
    expect(text).toContain('traceId: 18f2a1b3c4d5000001')
    expect(text).toContain('耗时: 120ms')
    expect(text).toContain('页面: /system/config')
    expect(text).toContain('研发可据此定位网关日志')
  })

  it('缺省字段不产生空行；无 traceId 时给通用提示', () => {
    const text = buildDiagnosticText({ error: '网络错误: timeout' })
    expect(text).toContain('错误: 网络错误: timeout')
    expect(text).not.toContain('traceId:')
    expect(text).not.toContain('undefined')
    expect(text).toContain('请附上本段信息')
  })

  it('status 为 0 / biz 字符串时照常输出（0 是有效值，不能按 falsy 丢掉）', () => {
    expect(buildDiagnosticText({ status: 0 })).toContain('结果: 0')
    expect(buildDiagnosticText({ status: 'biz:8000' })).toContain('结果: biz:8000')
  })
})

/**
 * 诊断报告「错误日志」分节的回归护栏（P1-11）。
 *
 * 背景：原实现只有一节「最近错误日志」，内容取自**浏览器 localStorage 里的
 * 前端操作审计日志**——现场网关重启、采集失败、串口报错这类真正的网关故障
 * 一条都不会出现，该节几乎恒空，还会被误读成「网关没报错」。
 * 修复后拆成两节，各自标注来源：
 *   6. 网关采集错误（/api/diagnostics/stats）
 *   7. 网关侧失败的 API 请求（/api/access/rate-logs）
 * 前端本地操作审计已下线（B3），报告不再单列该节。
 */
describe('buildDiagnosticReport 错误分节来源（P1-11）', () => {
  it('网关采集错误与网关 API 错误各自成节，并标注来源', () => {
    const html = buildDiagnosticReport({
      gatewayErrors: ['串口 IO 错误: 12 次（占比 60%）'],
      apiAccessErrors: ['[2026-10-27 10:00:00] GET /api/points → 500（3ms）'],
    })
    expect(html).toContain('网关采集错误')
    expect(html).toContain('串口 IO 错误: 12 次（占比 60%）')
    expect(html).toContain('/api/diagnostics/stats')
    expect(html).toContain('网关侧失败的 API 请求')
    expect(html).toContain('/api/access/rate-logs')
  })

  it('前端本地审计已下线：不再出现「前端操作审计」或笼统的「最近错误日志」节', () => {
    const html = buildDiagnosticReport({})
    expect(html).not.toContain('前端操作审计失败')
    expect(html).not.toContain('最近错误日志')
  })

  it('网关侧无错误时给出明确说明，而不是空白', () => {
    const html = buildDiagnosticReport({})
    expect(html).toContain('网关侧未记录到采集错误')
    expect(html).toContain('网关侧未记录到失败的 API 请求')
  })

  it('错误内容里的 HTML 仍被转义（XSS 防护不因重构而失效）', () => {
    const html = buildDiagnosticReport({
      gatewayErrors: ['<script>alert(1)</script>'],
    })
    expect(html).not.toContain('<script>alert(1)</script>')
    expect(html).toContain('&lt;script&gt;')
  })
})

/**
 * 诊断报告「健康检查」分节与界面同源。
 *
 * 背景：报告原先直接把后端状态词原样塞进表格（mqtt / down / degraded），
 * 并且只按「ok / 其它」二分着色 —— degraded、backpressure 全被标成红色「坏」。
 * 界面改成中文词表后，报告会与界面说法不一致。
 *
 * 契约：
 *   - 组件名/状态名显示中文，同时保留后端原始状态词供研发对日志
 */
describe('buildDiagnosticReport 健康检查分节', () => {
  it('组件名与状态名显示中文，并保留后端原始状态词', () => {
    const html = buildDiagnosticReport({
      health: {
        status: 'degraded',
        components: { mqtt: 'down', ota: 'degraded', queue: 'ok' },
      },
    })
    expect(html).toContain('MQTT 客户端')
    expect(html).toContain('不可用（down）')
    expect(html).toContain('降级运行（degraded）')
    expect(html).toContain('采集队列')
    // 原始状态词保留，研发能对着网关日志/后端代码核对
    expect(html).toContain('后端原始状态：degraded')
  })

  it('聚合 status 为 degraded 但组件全 ok 时整体判定正常', () => {
    const html = buildDiagnosticReport({
      health: {
        status: 'degraded',
        components: { process: 'ok', mqtt: 'ok', queue: 'ok', ota: 'ok' },
      },
    })
    // 组件都正常时整体判定为正常（不按后端聚合 status 判降级）
    expect(html).toContain('正常（后端原始状态：degraded）')
  })

  it('组件异常时整体判定为异常', () => {
    const html = buildDiagnosticReport({
      health: { status: 'degraded', components: { mqtt: 'down' } },
    })
    expect(html).toContain('异常（后端原始状态：degraded）')
    expect(html).toContain('bad">不可用（down）')
  })
})
