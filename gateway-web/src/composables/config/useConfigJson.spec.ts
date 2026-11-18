import { describe, expect, it, vi } from 'vitest'
import { nextTick, reactive, ref } from 'vue'
import { useConfigJson } from './useConfigJson'
import { createSeedConfig, type ConfigForm } from '@/utils/configDefaults'

/**
 * 「原始 JSON」标签页的契约。
 *
 * 这是用户直接改配置文本的入口，出错代价最高：
 * - 解析失败**不能**把表单里的现有配置清空（syncRawToConfig 先 parse 再 delete+assign，
 *   顺序一旦写反，用户打错一个逗号就会把整份配置抹成空对象，再点保存就把网关清空了）；
 * - 解析成功但缺节时，必须补后端要求的必填节（web/mqtt/sim_consumer），否则保存必然失败；
 * - nullable 节缺省要显式置 null，不能残留上一轮的旧值（否则「删掉的配置又回来了」）。
 */
function setup() {
  const config = reactive(createSeedConfig()) as ConfigForm
  const activeTab = ref('raw')
  const onUnknownTopKeys = vi.fn()
  const api = useConfigJson({ config, activeTab, onUnknownTopKeys })
  return { config, activeTab, onUnknownTopKeys, ...api }
}

describe('useConfigJson · 解析失败', () => {
  it('非法 JSON：置错误态与错误文案，且不动现有配置', () => {
    const { config, syncRawToConfig, parseError, parseErrorMessage, rawText } = setup()
    config.buses = [{ name: 'keep' }] as never
    config.web = { port: 8123 } as never
    const snapshot = JSON.stringify(config)

    rawText.value = '{"buses": [}'
    syncRawToConfig()

    expect(parseError.value).toBe(true)
    expect(parseErrorMessage.value).not.toBe('')
    // 关键：解析失败后 config 仍是原值，没有被 delete 清空
    expect(JSON.stringify(config)).toBe(snapshot)
  })

  it('顶层是数组 / null / 数字时拒绝，不当作配置对象', () => {
    const { syncRawToConfig, parseError, parseErrorMessage, rawText } = setup()
    for (const bad of ['[]', 'null', '42', '"text"']) {
      rawText.value = bad
      syncRawToConfig()
      expect(parseError.value, bad).toBe(true)
      expect(parseErrorMessage.value, bad).toBeTruthy()
    }
  })
})

describe('useConfigJson · 解析成功', () => {
  it('覆盖配置并补回后端必填节（web / mqtt / sim_consumer）', () => {
    const { config, syncRawToConfig, parseError, rawText } = setup()
    rawText.value = JSON.stringify({ schema_version: 1, buses: [], points: [] })
    syncRawToConfig()

    expect(parseError.value).toBe(false)
    expect(Array.isArray(config.buses)).toBe(true)
    expect(Array.isArray(config.points)).toBe(true)
    // 后端这三节是必填结构，缺了会导致整份配置反序列化失败
    expect(config.web).toBeTruthy()
    expect(config.mqtt).toBeTruthy()
    expect(config.sim_consumer).toBeTruthy()
  })

  it('nullable 节缺省时显式置 null，不残留上一轮的旧值', () => {
    const { config, syncRawToConfig, rawText } = setup()
    config.ntp = { server: 'ntp.old', sync_interval: 60 } as never

    rawText.value = JSON.stringify({ schema_version: 1, buses: [], points: [] })
    syncRawToConfig()

    expect(config.ntp).toBeNull()
  })

  it('未识别顶层键交给回调提示，但不阻断解析', () => {
    const { syncRawToConfig, rawText, onUnknownTopKeys, parseError } = setup()
    rawText.value = JSON.stringify({ schema_version: 1, strange_key: true })
    syncRawToConfig()

    expect(parseError.value).toBe(false)
    expect(onUnknownTopKeys).toHaveBeenCalledTimes(1)
    expect(onUnknownTopKeys.mock.calls[0]![1]).toBe('原始 JSON')
  })

  it('buses 为旧 map 形态时归零为空数组（后端只接受数组）', () => {
    const { config, syncRawToConfig, rawText } = setup()
    rawText.value = JSON.stringify({ schema_version: 1, buses: { a: { name: 'a' } } })
    syncRawToConfig()
    expect(config.buses).toEqual([])
  })
})

describe('useConfigJson · 双向同步', () => {
  it('syncConfigToRaw 输出缩进 JSON 并清除错误态', () => {
    const { syncConfigToRaw, rawText, parseError, config } = setup()
    parseError.value = true
    syncConfigToRaw()

    expect(parseError.value).toBe(false)
    expect(rawText.value).toContain('\n  "buses"')
    expect(JSON.parse(rawText.value)).toMatchObject({ buses: config.buses })
  })

  it('切回 raw 标签时按当前表单重新序列化', async () => {
    const { activeTab, rawText, config } = setup()
    activeTab.value = 'form'
    await nextTick()
    rawText.value = 'stale text'
    config.buses = [{ name: 'bus-1' }] as never

    activeTab.value = 'raw'
    await nextTick()

    expect(rawText.value).toContain('bus-1')
  })

  it('存在解析错误时切回 raw 不覆盖用户输入（否则错误无法修改）', async () => {
    const { activeTab, rawText, syncRawToConfig } = setup()
    rawText.value = '{"buses": [}'
    syncRawToConfig()

    activeTab.value = 'form'
    await nextTick()
    activeTab.value = 'raw'
    await nextTick()

    expect(rawText.value).toBe('{"buses": [}')
  })
})

describe('useConfigJson · 高亮', () => {
  it('转义 HTML 实体后再着色，不把配置内容当标签执行', () => {
    const { rawText, rawJsonHtml } = setup()
    rawText.value = '{"k": "<img src=x onerror=alert(1)>", "n": 1, "b": true, "z": null}'
    const html = rawJsonHtml.value

    expect(html).not.toContain('<img')
    expect(html).toContain('&lt;img')
    expect(html).toContain('json-key')
    expect(html).toContain('json-number')
    expect(html).toContain('json-boolean')
    expect(html).toContain('json-null')
  })
})
