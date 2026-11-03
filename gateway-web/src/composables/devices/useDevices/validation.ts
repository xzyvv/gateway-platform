/**
 * 设备表单校验规则
 *
 * 表单校验按协议分语义：Modbus 的地址是从站号、DLT645/CJT188 的地址是表号、
 * IEC104 的地址是 host:port——同一字段的取值范围完全不同，
 * 所以校验器必须读当前协议再判范围，不能写死一套规则。
 */
import type { FormItemRule, FormRules } from 'element-plus'
import type { DeviceState } from './types'

/**
 * 设备表单校验规则
 *
 * @param state 共享状态（form.protocol 决定校验语义）
 */
export function createDeviceValidation(state: DeviceState) {
  /** 当前是否 Modbus 协议（协议相关校验：从站号/地址语义不同）。 */
  const isModbus = () => state.form.protocol === 'modbus'

  /** 地址校验：按协议限定数值范围/格式，避免非法值下发到现场。 */
  const validateAddress: FormItemRule['validator'] = (_rule, value, callback) => {
    const v = String(value ?? '').trim()
    if (!v) { callback(new Error('请输入地址')); return }
    switch (state.form.protocol) {
      case 'modbus':
        if (!/^\d+$/.test(v) || Number(v) < 1 || Number(v) > 247) {
          callback(new Error('Modbus 从站号需为 1-247 的整数')); return
        }
        break
      case 'dlt645':
        if (!/^\d+$/.test(v) || Number(v) < 1 || Number(v) > 65535) {
          callback(new Error('DLT645 表号需为 1-65535 的整数')); return
        }
        break
      case 'cjt188':
        if (!/^\d+$/.test(v) || Number(v) < 1 || Number(v) > 99999999) {
          callback(new Error('CJT188 表号需为 1-99999999 的整数')); return
        }
        break
      case 'iec104':
        if (!/^[^\s/:]+:\d{1,5}$/.test(v)) {
          callback(new Error('IEC104 地址需为 host:port，如 192.168.1.10:2404')); return
        }
        break
      default:
        break
    }
    callback()
  }

  /** 从站号校验：可留空；填了必须是 1-247 的整数（Modbus 规范）。 */
  const validateSlaveId: FormItemRule['validator'] = (_rule, value, callback) => {
    if (value === null || value === undefined || value === '') { callback(); return }
    const n = Number(value)
    if (!Number.isInteger(n) || n < 1 || n > 247) {
      callback(new Error('从站号需为 1-247 的整数')); return
    }
    callback()
  }

  /** 采集间隔校验：可留空（使用模板默认）；填了必须是 ≥1 的整数秒。 */
  const validateInterval: FormItemRule['validator'] = (_rule, value, callback) => {
    if (value === null || value === undefined || value === '') { callback(); return }
    const n = Number(value)
    if (!Number.isInteger(n) || n < 1) {
      callback(new Error('采集间隔需为 ≥1 的整数秒（留空使用模板默认）')); return
    }
    callback()
  }

  const rules: FormRules = {
    name: [{ required: true, message: '请输入设备名', trigger: 'blur' }],
    // S3: 模板必填。原规则缺这一条 → 用户漏填时保存必失败「template 不能为空」，
    // 且输入框没有红星，看不出这是必填项。
    template: [{ required: true, message: '请选择设备模板', trigger: 'change' }],
    bus: [{ required: true, message: '请输入总线', trigger: 'blur' }],
    protocol: [{ required: true, message: '请选择协议', trigger: 'change' }],
    address: [
      { required: true, message: '请输入地址', trigger: 'blur' },
      { validator: validateAddress, trigger: 'blur' },
    ],
    slave_id: [{ validator: validateSlaveId, trigger: 'change' }],
    interval: [{ validator: validateInterval, trigger: 'change' }],
  }

  return { rules, isModbus }
}
