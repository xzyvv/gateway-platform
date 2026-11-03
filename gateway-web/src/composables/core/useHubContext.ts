/**
 * Hub 页当前子页签上下文
 *
 * HubShell 在 tab 切换时写入当前 tab 名称，顶栏面包屑读取后补一层，
 * 让“报文调试 → 单步调试”这类三层导航在地址栏之外也有可见定位。
 * 单例即可：同一时刻只有一个 Hub 页挂载（路由级）。
 */
import { readonly, ref, type ComputedRef, type InjectionKey } from 'vue'

// 有意用模块级单例：同一时刻只有一个 Hub 页挂载，HubShell 写入、Layout 读取，
// 不需要 Pinia store 那套 devtools/持久化能力。
// eslint-disable-next-line no-restricted-syntax
const hubTabLabel = ref('')

/** HubShell 调用：写入/清空当前 tab 名称 */
export function setHubTabLabel(label: string): void {
  hubTabLabel.value = label
}

/**
 * HubShell 通过 provide 注入的「当前页签名」。
 *
 * 用途：HubShell 把「当前页签名」写入全局，全站统一用左侧分组导航，
 * 页内若再出现同名 SectionHeader 就是同屏双标题。SectionHeader 注入本 key
 * 后与自己的 title 比对，相同则省掉标题本体，只保留操作按钮。
 *
 * 页签很少的 Hub 也用左侧导航（P2-6 全站单一范式），因此这里不再有
 * 「顶部标签条模式注入空串」的分支。
 */
export const hubTabLabelKey: InjectionKey<ComputedRef<string>> = Symbol('hubTabLabel')

export function useHubContext() {
  return { hubTabLabel: readonly(hubTabLabel) }
}
