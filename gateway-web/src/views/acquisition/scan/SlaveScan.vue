<template>
  <div class="ss"><SectionHeader title="从站扫描" icon="Search" />
    <el-alert
      class="section-card"
      type="info"
      show-icon
      :closable="false"
      title="用途：找出未知的从站号"
      description="按地址区间逐个探测有无应答，只回答「这个地址上有没有设备」，不识别波特率/校验位等参数。若已知设备地址、需要确认通信参数，请到「参数探测」。"
    />
    <el-card shadow="never" class="section-card">
      <el-form inline>
        <el-form-item label="总线">
          <el-select v-model="bus" style="width:220px" :loading="targetsLoading" placeholder="选择总线" aria-label="总线" no-data-text="暂无总线，请先在配置管理中添加">
            <el-option v-for="o in busOptions" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
        </el-form-item>
        <el-form-item label="起始ID"><el-input-number v-model="start" :min="1" :max="247" /></el-form-item>
        <el-form-item label="结束ID"><el-input-number v-model="end" :min="1" :max="247" /></el-form-item>
        <el-form-item>
          <el-button :type="scanning ? 'danger' : 'primary'" @click="onScanButton" :icon="scanning ? undefined : Search" :disabled="!scanSupported">
            {{ scanning ? '停止' : '扫描' }}
          </el-button>
        </el-form-item>
      </el-form>
    </el-card>
    <el-alert
      v-if="capsFailed"
      class="section-card"
      type="warning"
      show-icon
      :closable="false"
      title="协议能力元数据加载失败"
      description="暂时无法判断当前总线是否支持从站扫描，按钮已禁用；请刷新页面重试。"
    />
    <el-alert
      v-else-if="capsLoaded && selectedBusProtocol && !scanSupported"
      class="section-card"
      type="info"
      show-icon
      :closable="false"
      :title="`当前总线协议（${selectedBusProtocol}）不支持从站扫描`"
      description="该协议不存在「从站地址」概念，从站扫描不适用（不是尚未实现）；如需扫描设备，请切换到 Modbus 等从站寻址型总线。"
    />
    <el-alert
      v-if="scanError"
      class="section-card"
      type="error"
      show-icon
      :closable="true"
      :title="scanError"
      @close="scanError = ''"
    />
    <el-card v-if="results.length" shadow="never" class="section-card">
      <template #header>
        <strong>扫描结果</strong>（发现 {{ found }} 台设备<template v-if="scanning">，已扫 {{ scannedCount }} / {{ totalCount }}</template>）
      </template>
      <div class="grid" role="list" aria-label="从站扫描结果">
        <div v-for="(r, idx) in results" :key="r.slaveId" class="cell" :class="{ on: r.responded, pending: scanning && idx >= scannedCount }" role="listitem" :aria-label="`从站 ${r.slaveId} ${r.responded ? `有响应 ${r.latencyMs}ms` : '无响应'}`" :title="r.responded ? `${r.deviceName ?? '在线 · 未识别'} ${r.latencyMs}ms` : '无响应'">
          <div class="id">{{ r.slaveId }}</div>
          <el-icon v-if="r.responded" class="ic"><CircleCheck /></el-icon>
          <el-icon v-else class="ic off"><Close /></el-icon>
        </div>
      </div>
    </el-card>
  </div>
</template>
<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { ElMessage } from 'element-plus'
import { Search, CircleCheck, Close } from '@element-plus/icons-vue'
import { scanBus, scanBusStream, canStreamScan, type SlaveScanResult } from '@/api/protocolTools'
import { listProtocolCapabilities } from '@/api/protocols'
import type { ProtocolCapabilities } from '@/api/generated/ProtocolCapabilities'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'

// 共享调试目标（审查#5）：总线选择跨调试页保持一致，选项加载后失效值自动回退第一项
const { buses, busOptions, loading: targetsLoading, selectedBus: bus } = useDebugTargets()

// 能力门控（审查#3b）：DLT645/CJT188 等协议未实现从站扫描，按钮应禁用并提示，
// 而非等后端返回错误。能力元数据缺失（未加载/加载失败）时同样禁用：
// 后端仅对 Modbus 总线开放扫描，盲目放行等于给用户一个「点了必然失败」的按钮。
const protocolCaps = ref<Record<string, ProtocolCapabilities>>({})
const capsLoaded = ref(false)
const capsFailed = ref(false)
const selectedBusProtocol = computed(() =>
  buses.value.find((b) => b.port_name === bus.value)?.protocol ?? '',
)
const selectedCaps = computed(() =>
  selectedBusProtocol.value ? protocolCaps.value[selectedBusProtocol.value] ?? null : null,
)
const scanSupported = computed(() => {
  const caps = selectedCaps.value
  // 能力元数据未加载 / 加载失败时**不放行**：后端「从站扫描」只对 Modbus 总线开放，
  // 其余协议请求必然返回 UnsupportedProtocol。旧实现降级为保守放行，
  // 会让 BACnet 等总线出现「按钮可点但一定失败」的错觉。
  return caps ? caps.supports_scan_slaves : false
})

async function loadProtocolCaps() {
  try {
    const list = await listProtocolCapabilities()
    const map: Record<string, ProtocolCapabilities> = {}
    for (const e of list) map[e.name] = e.capabilities
    protocolCaps.value = map
  } catch {
    // 能力元数据获取失败时保持为空 —— 门控按「不支持」处理（不放行），
    // 避免用户在能力未知的情况下点了扫描才失败。
    capsFailed.value = true
  } finally {
    capsLoaded.value = true
  }
}
onMounted(loadProtocolCaps)
const start = ref(1); const end = ref(20); const scanning = ref(false)
/** 扫描失败原因（带处置建议）：常驻页面，避免失败后只剩空白结果区 */
const scanError = ref('')
const { message } = useErrorDisplay()
const results = ref<SlaveScanResult[]>([])
const found = computed(() => results.value.filter((r) => r.responded).length)
// P0-2 流式进度：逐格点亮 + 「已扫 N / total」
const scannedCount = ref(0)
const totalCount = ref(0)
let controller: AbortController | null = null

function onScanButton() {
  // 扫描中点击 = 停止（abort 属正常收尾：后端在下一地址前停止，前端保留已扫部分）
  if (scanning.value) { controller?.abort(); return }
  void scan()
}

// 修复(审查#13)：原实现是 `scanning = true; await scanBus(); scanning = false` 的顺序语句，
// scanBus 抛错时 `scanning` 永远停在 true —— 按钮持续 loading、无法再次点击重试。
// 改为 try/finally 保证任何路径都复位；失败原因常驻在结果区上方（带处置建议）。
async function scan() {
  if (!bus.value) { ElMessage.warning('请先选择总线'); return }
  scanning.value = true
  scanError.value = ''
  scannedCount.value = 0
  totalCount.value = end.value - start.value + 1
  const streaming = canStreamScan()
  // 流式模式预填占位网格（pending 态淡显），事件逐格点亮；阻塞模式拿到结果一次性渲染
  results.value = streaming
    ? Array.from({ length: totalCount.value }, (_, i) => ({ slaveId: start.value + i, responded: false }))
    : []
  const ctl = streaming ? new AbortController() : null
  controller = ctl
  try {
    if (streaming && ctl) {
      await scanBusStream(bus.value, start.value, end.value, {
        signal: ctl.signal,
        onSlave: (r) => {
          const idx = r.slaveId - start.value
          if (idx >= 0 && idx < results.value.length) results.value[idx] = r
          scannedCount.value++
        },
        onSummary: (s) => {
          results.value = s.results
          ElMessage.success(`扫描完成，发现 ${s.found} 台设备${s.noise > 0 ? `（${s.noise} 次线路噪声，请检查接线/屏蔽）` : ''}`)
        },
        onAborted: (info) => {
          ElMessage.warning(`扫描提前停止（${info.reason === 'duration_cap' ? '达到时长上限' : info.reason}），已扫 ${info.scanned}/${info.total}`)
        },
        onFailed: (info) => { scanError.value = info.message },
      })
      if (ctl.signal.aborted) ElMessage.info('已停止，显示已扫部分')
    } else {
      results.value = await scanBus(bus.value, start.value, end.value)
      ElMessage.success(`扫描完成，发现 ${found.value} 台设备`)
    }
  } catch (e) {
    // 请求层已弹过 toast；这里常驻展示原因 + 处置建议，便于现场对照
    if (!streaming) results.value = []
    scanError.value = message(e, '从站扫描失败')
  } finally {
    scanning.value = false
    controller = null
  }
}
</script>
<style scoped>
.ss{padding:16px}.section-card{margin-bottom:16px}.grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(60px,1fr));gap:8px}.cell{height:60px;border-radius: var(--radius-table);display:flex;flex-direction:column;align-items:center;justify-content:center;border:2px solid var(--el-border-color);background:var(--el-fill-color-light);position:relative}.cell.on{border-color:var(--el-color-success);background:var(--el-color-success-light-9)}.cell.pending{opacity:.45}.id{font-weight:600}.ic{color:var(--el-color-success);position:absolute;top:2px;right:4px;font-size: 0.875rem}.ic.off{color:var(--el-text-color-secondary)}
</style>
