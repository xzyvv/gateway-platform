// WebConfigTab.vue 单元测试：高级（性能与限流）折叠区。
// 关键语义：rate_limit_per_sec 为 null 表示不限流，开关必须能写回 null；
// 实时推送上限为热更项，限流为重启项，提示文案要区分开。
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick, reactive } from 'vue'
import { ElSwitch } from 'element-plus'
import WebConfigTab from './WebConfigTab.vue'
import { createSeedConfig } from '@/utils/configDefaults'
import type { WebConfig } from '@/api/types'

function mountTab() {
  // 种子配置里 web 段必然存在（后端读配置时会补默认值），这里显式断言类型
  const web = reactive(createSeedConfig().web as WebConfig)
  return {
    web,
    wrapper: mount(WebConfigTab, {
      props: { modelValue: web, originalToken: '', tokenStatusText: '已配置' },
    }),
  }
}

describe('WebConfigTab.vue 高级（性能与限流）', () => {
  it('默认收起，展开后可见实时推送上限与限流开关', async () => {
    const { wrapper } = mountTab()
    const wrap = wrapper.find('.el-collapse-item__wrap')
    expect(wrap.isVisible(), '高级分组默认应收起').toBe(false)

    await wrapper.find('.el-collapse-item__header').trigger('click')
    await nextTick()

    const text = wrapper.text()
    expect(text).toContain('实时推送并发上限')
    expect(text).toContain('接口限流（次/秒）')
    expect(text).toContain('保存后对新连接立即生效')
    expect(text).toContain('改动需重启网关后生效')
    wrapper.unmount()
  })

  it('限流开关关闭时写回 null，开启时回落到默认 60', async () => {
    const { web, wrapper } = mountTab()
    await wrapper.find('.el-collapse-item__header').trigger('click')
    await nextTick()

    expect(web.rate_limit_per_sec).toBeNull()

    // 直接驱动组件的 update:modelValue，避免依赖 jsdom 对原生 checkbox 的模拟；
    // 页面里还有「匿名读/禁止测试内网」开关，必须限定在限流那一行内查找
    const switchComp = wrapper.find('.rate-limit-field').findComponent(ElSwitch)
    switchComp.vm.$emit('update:modelValue', true)
    await nextTick()
    expect(web.rate_limit_per_sec).toBe(60)

    switchComp.vm.$emit('update:modelValue', false)
    await nextTick()
    expect(web.rate_limit_per_sec).toBeNull()
    wrapper.unmount()
  })

  it('跨域白名单三态：关闭=null，开启=仅同源，填写按行拆分', async () => {
    const { web, wrapper } = mountTab()
    await wrapper.find('.el-collapse-item__header').trigger('click')
    await nextTick()

    const corsSwitch = wrapper.find('.cors-field').findComponent(ElSwitch)
    corsSwitch.vm.$emit('update:modelValue', true)
    await nextTick()
    // 开启但未填来源 = 仅同源，绝不能回落成 null（null 代表允许任意来源）
    expect(web.cors_origins).toEqual([])

    const textarea = wrapper.find('textarea')
    await textarea.setValue('http://a.local\nhttp://b.local,http://c.local')
    await nextTick()
    expect(web.cors_origins).toEqual(['http://a.local', 'http://b.local', 'http://c.local'])

    corsSwitch.vm.$emit('update:modelValue', false)
    await nextTick()
    expect(web.cors_origins).toBeNull()
    wrapper.unmount()
  })
})
