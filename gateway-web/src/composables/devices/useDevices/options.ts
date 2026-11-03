/**
 * 设备表单的关联选项加载（模板 / 总线）
 *
 * 设备表单里的「模板」和「总线」都不是自由文本，而是从网关已有配置里选的：
 * 模板来自 /api/templates，总线来自 /api/config 的 buses。
 * 写错名字后端也能建出记录，但永远采不到数据，所以这里强制走下拉。
 */
import { listTemplates } from '@/api/templates'
import { getConfig } from '@/api/config'
import { extractErrorMessage } from '@/utils/errorHandler'
import type { DeviceConfig } from '@/api/types'
import type { DeviceState } from './types'

/**
 * 设备表单的模板 / 总线选项
 *
 * @param state 共享状态（templates / buses / form）
 */
export function createDeviceOptions(state: DeviceState) {
  /**
   * S3: 可选设备模板。此前「模板」是自由文本输入，placeholder 写的是**文件名**
   * （`dlt645_3phase_generic`），而后端按模板 `name` 查找（实际是中文显示名
   * `DLT645_三相电表通用`）——用户照着 placeholder 填必然失败，且失败发生在保存后。
   * 现改为下拉选择，直接展示后端模板库里的合法名称。
   */
  async function loadTemplates() {
    state.templatesLoading.value = true
    state.templateLoadError.value = ''
    try {
      state.templates.value = await listTemplates()
      if (!state.templates.value.length) {
        state.templateLoadError.value = '网关未加载到任何设备模板，请检查模板目录后重启网关'
      }
    } catch (e) {
      state.templateLoadError.value = extractErrorMessage(e)
    } finally {
      state.templatesLoading.value = false
    }
  }

  /**
   * N1: 可选总线。此前「总线」是自由文本框，写错名字仍能保存成功，
   * 后端按 bus 建不出采集器（只留一行 warn 日志），设备永远采不到数据。
   * 现改为从网关现有总线里选，列表来自 /api/config 的 buses。
   */
  async function loadBuses() {
    state.busesLoading.value = true
    state.busLoadError.value = ''
    try {
      const cfg = await getConfig()
      state.buses.value = cfg.buses || []
      if (!state.buses.value.length) {
        state.busLoadError.value =
          '当前网关还没有配置任何总线，请先到「系统设置 → 运行参数」新增一条总线，设备才能挂上去采集'
      }
    } catch (e) {
      state.busLoadError.value = extractErrorMessage(e)
    } finally {
      state.busesLoading.value = false
    }
  }

  /**
   * N2: 设备协议由模板决定，选择模板后自动带出并置只读。
   * 模板展开点位时用的是设备自身的 protocol，若与模板协议不一致，
   * 会按错误协议读寄存器（DLT645 表当 Modbus 读），整台设备全点位失败。
   */
  function applyTemplateProtocol(name: string) {
    const tpl = state.templates.value.find((t) => t.name === name)
    if (tpl?.protocol) state.form.protocol = tpl.protocol as DeviceConfig['protocol']
  }

  return { loadTemplates, loadBuses, applyTemplateProtocol }
}
