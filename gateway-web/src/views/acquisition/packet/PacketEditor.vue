<template>
  <div class="pe"><SectionHeader title="报文编辑" icon="Edit" />
    <el-card shadow="never" class="section-card">
      <el-form inline>
        <el-form-item label="总线">
          <el-select v-model="bus" style="width:220px" :loading="targetsLoading" placeholder="选择总线" aria-label="总线" no-data-text="暂无总线，请先在配置管理中添加">
            <el-option v-for="o in busOptions" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
        </el-form-item>
        <el-form-item label="超时(ms)">
          <el-input-number v-model="timeoutMs" :min="100" :max="10000" :step="100" style="width:130px" aria-label="单帧超时毫秒" />
        </el-form-item>
        <el-form-item label="十六进制(HEX)"><HexInput v-model="hex" style="width:360px" @update:invalid="inputInvalid = $event" /></el-form-item>
        <el-form-item><el-button type="primary" @click="send" :loading="sending" :disabled="hexInvalid || !bus" :icon="Promotion">发送</el-button></el-form-item>
      </el-form>
      <!-- 非法帧在本地拦下：抓包页深链 ?hex=ZZ、或输入残留都会走到这里。
           不能静默下发「上一帧合法帧」——否则现场以为发的是屏幕上这段，排查方向全错 -->
      <div v-if="hexInvalid" class="hex-err" role="alert">
        请求帧不是合法的 HEX（需偶数长度的 0-9A-F），已禁止下发
      </div>
      <div class="presets">
        <span class="lbl">请求模板：</span>
        <el-button v-for="p in presets" :key="p.name" size="small" @click="hex = p.hex">{{ p.name }}</el-button>
      </div>
      <!-- E1：总线被采集占用时不再只丢一句「请先暂停采集」，直接给出可点的暂停入口 -->
      <BusPauseBanner :bus="bus" :visible="busBusy" @paused="send" />
      <el-divider />
      <div v-if="result" class="result" aria-live="polite">
        <div class="row"><span>请求:</span><code>{{ result.requestHex }}</code></div>
        <div class="row"><span>响应:</span><code :class="{ ok: result.success, err: !result.success }">{{ result.responseHex || '(无响应帧)' }}</code></div>
        <div class="row"><span>解析:</span><span>{{ result.parsed }}</span></div>
        <div class="row"><span>耗时:</span><span>{{ result.latencyMs }} ms</span></div>
        <el-tag :type="result.success ? 'success' : 'danger'" size="small">
          {{ result.success ? '通信成功' : `通信失败：${failReason}` }}
        </el-tag>
      </div>
    </el-card>

    <!-- C4：在线响应与离线样本共用同一套解析内核与同一个结果组件。
         总线协议可被内核解析时（modbus-rtu / dlt645 / cjt188），把本次真实回帧逐字段展开；
         其它协议（如 modbus-tcp 的 MBAP 结构）不假装解析，仍以上方后端 `parsed` 文案为准。 -->
    <FrameParseResult
      v-if="onlineParse"
      :result="onlineParse"
      title="响应帧解析"
      hint="本次真实下发的回帧，与下方离线帧解析使用同一套内核"
      class="online-parse"
    />

    <!-- A5：帧解析并入报文编辑。离线核对区（纯前端解析，不连网关、不读现场数据），
         与上面的在线下发共用同一页，避免「拷帧回来核对」时在两处页面之间跳来跳去 -->
    <div class="parser-embed"><FrameParser /></div>
  </div>
</template>
<script setup lang="ts">
import { onMounted, ref, computed, watch, watchEffect } from 'vue'
import { useRoute } from 'vue-router'
import { ElMessage } from 'element-plus'
import {Promotion} from '@element-plus/icons-vue'
import { listPresets, sendFrame, type PacketPreset, type SendResult } from '@/api/protocolTools'
import HexInput from '@/components/industrial/HexInput.vue'
import BusPauseBanner from '@/components/BusPauseBanner.vue'
import { withDiagHint } from '@/utils/diagHints'
import { isValidHex } from '@/utils/hexFrame'
import { inferParseProtocol, parseFrame, type ParseResult } from '@/utils/frameParser'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import FrameParser from './FrameParser.vue'
import FrameParseResult from './parts/FrameParseResult.vue'

// 共享调试目标（审查#5）：总线选择跨调试页保持一致
const { busOptions, buses, loading: targetsLoading, selectedBus: bus } = useDebugTargets()

const hex = ref('01 03 00 00 00 02')
const timeoutMs = ref(1500)
const result = ref<SendResult | null>(null)
/** 在线回帧的结构化解析结果（同一内核 parseFrame；协议不支持时保持 null） */
const onlineParse = ref<ParseResult | null>(null)
const sending = ref(false)
/** 输入框当前内容非法（HexInput 不写回非法值，只能靠该事件感知） */
const inputInvalid = ref(false)
/** 当前输入不是合法 HEX：按钮禁用 + 行内提示，send() 里再兜一层（防深链/回车等旁路） */
const hexInvalid = computed(() => inputInvalid.value || !isValidHex(hex.value))
/** 外部改写 hex（模板按钮 / 深链预填）后重置输入态，避免旧错误残留 */
watch(hex, () => { inputInvalid.value = false })
/** 当前总线协议：决定能否用帧解析内核展开回帧（modbus-tcp 等结构不同，不映射） */
const busProtocol = computed(() => buses.value.find((b) => b.port_name === bus.value)?.protocol)
/** E1：最近一次发送因总线被采集占用而失败 → 展示暂停采集入口 */
const busBusy = ref(false)
/** 请求模板：后端唯一定义（P3-4，前端不再硬编码副本）；拉取失败时只是没有快捷按钮 */
const presets = ref<PacketPreset[]>([])
onMounted(async () => {
  presets.value = await listPresets().catch(() => [])
})

/** error 机器码 → 面向现场的中文分类名（契约 §5；处置建议由 diagHints 按 parsed 追加） */
const ERROR_LABELS: Record<string, string> = {
  timeout: '超时无响应',
  crc: 'CRC 校验失败',
  exception: '设备返回异常帧',
  bus_offline: '总线不可用',
  bus_busy: '总线被占用',
  invalid_frame: '请求帧非法',
  bus_not_found: '总线不存在',
}

/** 失败原因：优先 error 分类 + 后端解析文案；响应帧为空说明超时/链路无应答。统一拼诊断建议（审查#6） */
const failReason = computed(() => {
  const r = result.value
  if (!r || r.success) return ''
  const label = r.error ? (ERROR_LABELS[r.error] ?? r.error) : ''
  const detail = r.parsed || (!r.responseHex ? '超时，未收到响应帧' : '未知错误')
  return withDiagHint(label ? `${label}：${detail}` : detail)
})

// 抓包页「发送到报文编辑」通过 ?tab=packet-editor&hex=... 跳转，这里承接预填。
// 用 watchEffect 而非 onMounted：Layout 的 PageErrorBoundary key=route.path，
// query 变化不重挂组件，所以必须响应式读取 route.query.hex，
// 避免同实例下 URL 变了但 hex 不刷新。
const route = useRoute()
watchEffect(() => {
  const q = route.query.hex
  if (typeof q === 'string' && q.trim()) hex.value = q
})

async function send() {
  if (!hex.value.trim() || !bus.value || sending.value) return
  // inputInvalid 时 hex.value 仍是上一帧合法值，必须一并拦住（暂停采集回调也会走到这里）
  if (inputInvalid.value || !isValidHex(hex.value)) {
    ElMessage.warning('请求帧不是合法的 HEX（需偶数长度的 0-9A-F），已阻止下发')
    return
  }
  sending.value = true
  try {
    result.value = await sendFrame(hex.value, bus.value, timeoutMs.value)
    busBusy.value = result.value?.error === 'bus_busy'
    // 真实回帧交给同一个解析内核；解析失败也不会抛（内核内部返回 valid=false）
    const p = inferParseProtocol(busProtocol.value)
    const response = result.value?.responseHex?.trim()
    onlineParse.value = p && response ? parseFrame(p, response) : null
  } finally {
    sending.value = false
  }
}
</script>
<style scoped>
.pe{padding:16px}.section-card{margin-bottom:16px}.presets{display:flex;align-items:center;gap:6px;flex-wrap:wrap;margin-bottom:12px}.lbl{color:var(--el-text-color-secondary);font-size: 0.8125rem}.result{line-height:2.2}.row{display:flex;gap:12px;align-items:center;margin-bottom:6px}.row span{min-width:60px;color:var(--el-text-color-secondary)}code{color:var(--el-color-primary);background:var(--el-fill-color-light);padding:2px 8px;border-radius: var(--radius-small)}code.ok{color: var(--text-success)}code.err{color: var(--text-danger)}
/* 子组件自带卡片，这里只补与上方在线下发区的间距 */
.online-parse,.parser-embed{margin-top:16px}
.hex-err{margin:-4px 0 10px;color:var(--text-danger);font-size:0.75rem}
</style>
