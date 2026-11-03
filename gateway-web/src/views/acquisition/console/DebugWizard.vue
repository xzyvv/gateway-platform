<template>
  <div class="dw">
    <SectionHeader title="现场调试向导" icon="Compass">
      <template #extra>
        <el-tag size="small" type="info">{{ debugWizardFlows.length }} 条排查路径</el-tag>
      </template>
    </SectionHeader>

    <p class="cap">按协议与症状排查，按顺序执行检查项；不确定跳过也可以，最后会给出下一步入口。</p>

    <el-card shadow="never" class="section-card filters">
      <div class="filter-row">
        <span class="lbl">协议</span>
        <el-radio-group v-model="protocolFilter" size="small">
          <el-radio-button value="all">全部</el-radio-button>
          <el-radio-button v-for="p in WIZARD_PROTOCOLS" :key="p" :value="p">
            {{ PROTOCOL_LABELS[p] }}
          </el-radio-button>
        </el-radio-group>
      </div>
      <div class="filter-row">
        <span class="lbl">症状</span>
        <el-select
          v-model="selectedFlowId"
          size="small"
          class="symptom-select"
          aria-label="选择要排查的症状"
          placeholder="选择要排查的症状"
          no-data-text="该协议下暂无排查路径"
        >
          <el-option v-for="f in filteredFlows" :key="f.id" :label="f.symptom" :value="f.id" />
        </el-select>
        <el-tag size="small" type="info">{{ filteredFlows.length }} 条</el-tag>
      </div>
    </el-card>

    <el-card v-if="flow" shadow="never" class="section-card flow">
      <template #header>
        <div class="flow-head">
          <el-tag size="small" effect="plain">{{ PROTOCOL_LABELS[flow.protocol] }}</el-tag>
          <strong>{{ flow.symptom }}</strong>
          <span class="progress">{{ doneCount }}/{{ flow.steps.length }} 已完成</span>
        </div>
      </template>

      <p class="intro">{{ flow.intro }}</p>

      <div class="steps">
        <div
          v-for="(item, index) in flowSteps"
          :key="item.step.id"
          class="step"
          :class="{ done: !!checked[item.step.id] }"
        >
          <el-checkbox
            v-model="checked[item.step.id]"
            class="tick"
            :aria-label="`标记「${item.step.title}」为已完成`"
          />
          <div class="idx">{{ index + 1 }}</div>
          <div class="body">
            <div class="title">
              <span>{{ item.step.title }}</span>
              <el-tag v-if="item.step.probe" size="small" type="info" effect="plain">可自动检查</el-tag>
            </div>
            <div class="detail">{{ item.step.detail }}</div>

            <div class="ops">
              <el-button
                v-if="item.step.action"
                size="small"
                @click="go(item.step.action?.to)"
              >去处理 · {{ item.step.action?.label }}</el-button>
              <el-button
                v-if="item.step.probe"
                size="small"
                type="primary"
                plain
                :loading="loadingStepId === item.step.id"
                @click="runProbe(item.step)"
              >自动检查</el-button>
            </div>

            <div v-if="item.probe" class="probe" :class="item.probe.tone">
              <div class="probe-line">{{ item.probe.message }}</div>
              <ul v-if="item.probe.problems?.length" class="problems">
                <li v-for="p in item.probe.problems" :key="`${p.code}-${p.message}`">
                  <el-tag size="small" :type="levelTagType(p.level)">{{ p.code }}</el-tag>
                  <span>{{ p.message }}</span>
                </li>
              </ul>
              <el-button
                v-if="item.probe.goto"
                link
                type="primary"
                size="small"
                @click="go(item.probe?.goto?.to)"
              >{{ item.probe.goto.label }}</el-button>
            </div>
          </div>
        </div>
      </div>

      <!-- 汇总卡：勾选进度、自动检查结论与下一步入口收成一块。
           现场交接班时看一眼就知道「查到哪、还差什么、该点哪」，
           不必逐条回读全部检查项 -->
      <div class="summary" :class="verdictTone">
        <div class="summary-head">
          <strong>排查汇总</strong>
          <el-tag size="small" :type="summaryTagType" effect="plain">
            {{ doneCount }}/{{ flow.steps.length }} 已完成
          </el-tag>
          <span class="probe-count">自动检查 {{ probeStats.done }}/{{ probeTotal }} 项已执行</span>
        </div>
        <div class="verdict">{{ verdictText }}</div>
        <div v-if="nextActions.length && !allClear" class="next">
          <span class="lbl">下一步</span>
          <el-button
            v-for="a in nextActions"
            :key="a.to"
            size="small"
            @click="go(a.to)"
          >{{ a.label }}</el-button>
        </div>
      </div>
    </el-card>

    <EmptyState v-else description="没有可用的排查路径" :image-size="72" />
  </div>
</template>

<script setup lang="ts">
/**
 * 现场调试向导（C3）
 *
 * 把专家经验整理成「协议 + 症状」的可执行检查清单：流程数据集中在
 * parts/debugWizardFlows.ts，本组件只负责筛选、勾选进度与三类自动检查。
 *
 * 自动检查复用现成资产，不另建数据通道：
 * - bus    → useDebugTargets() 的总线列表 + 共享调试目标，判断协议是否匹配
 * - health → useHealthReport() 的聚合快照，列出与该协议相关的 problems
 * - device → useDebugTargets() 的真实设备列表与当前选中设备
 *
 * 勾选状态仅保存在组件内（切换流程即清空），不引入新的全局单例。
 */
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import type { HealthProblem } from '@/api/status'
import EmptyState from '@/components/EmptyState.vue'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import { useHealthReport } from '@/composables/system/useHealthReport'
import { extractErrorMessage } from '@/utils/errorHandler'
import {
  PROTOCOL_LABELS,
  WIZARD_PROTOCOLS,
  busMatchesProtocol,
  busProtocolName,
  debugWizardFlows,
  type WizardFlow,
  type WizardProtocol,
  type WizardStep,
} from './parts/debugWizardFlows'

/** 单条检查项的自动检查结果 */
interface ProbeResult {
  tone: 'success' | 'warning' | 'danger' | 'info'
  message: string
  problems?: HealthProblem[]
  goto?: { label: string; to: string }
}

const router = useRouter()

const protocolFilter = ref<WizardProtocol | 'all'>('all')
const selectedFlowId = ref(debugWizardFlows[0]?.id ?? '')
const checked = ref<Record<string, boolean>>({})
const probeResults = ref<Record<string, ProbeResult>>({})
const loadingStepId = ref<string | null>(null)

const filteredFlows = computed<WizardFlow[]>(() =>
  protocolFilter.value === 'all'
    ? debugWizardFlows
    : debugWizardFlows.filter((f) => f.protocol === protocolFilter.value),
)

const flow = computed<WizardFlow | null>(
  () => debugWizardFlows.find((f) => f.id === selectedFlowId.value) ?? null,
)

/** 把检查项与已得出的自动检查结果配成对，模板里一次遍历即可 */
const flowSteps = computed(() =>
  (flow.value?.steps ?? []).map((step) => ({
    step,
    probe: probeResults.value[step.id] ?? null,
  })),
)

const doneCount = computed(() => {
  const current = flow.value
  if (!current) return 0
  return current.steps.filter((s) => Boolean(checked.value[s.id])).length
})

/** 当前流程里可自动检查的步骤总数 */
const probeTotal = computed(() => (flow.value?.steps ?? []).filter((s) => s.probe).length)

/** 已执行的自动检查按结论分类：严重问题必须先处理，不能被「进度 5/5」掩盖 */
const probeStats = computed(() => {
  const list = flowSteps.value
    .map((i) => i.probe)
    .filter((p): p is ProbeResult => p != null)
  return {
    done: list.length,
    danger: list.filter((p) => p.tone === 'danger').length,
    warning: list.filter((p) => p.tone === 'warning').length,
    success: list.filter((p) => p.tone === 'success').length,
  }
})

const remaining = computed(() => (flow.value?.steps.length ?? 0) - doneCount.value)

/** 汇总结论的色调：严重 > 异常 > 未完成 > 已确认但自动检查未跑完 > 全部通过 */
const verdictTone = computed<'danger' | 'warning' | 'success' | 'info'>(() => {
  if (probeStats.value.danger) return 'danger'
  if (probeStats.value.warning) return 'warning'
  if (remaining.value > 0) return 'info'
  if (probeStats.value.done < probeTotal.value) return 'info'
  return 'success'
})

const verdictText = computed(() => {
  const stats = probeStats.value
  const total = probeTotal.value
  if (stats.danger) {
    return `自动检查发现 ${stats.danger} 项严重问题，请先按步骤内的提示处置后再继续`
  }
  if (stats.warning) {
    return `自动检查发现 ${stats.warning} 项异常，建议先处理再继续排查`
  }
  if (remaining.value > 0) {
    return `还有 ${remaining.value} 项检查未完成，按顺序逐项确认即可定位问题`
  }
  // 勾选全完成 ≠ 自动检查全跑过：没跑自动检查就说「未发现异常」是假结论
  if (stats.done < total) {
    return `检查项已全部确认，但还有 ${total - stats.done} 项自动检查未执行，结论仅供参考`
  }
  return '全部检查项已完成，自动检查未发现异常'
})

const allClear = computed(() => verdictTone.value === 'success')
const summaryTagType = verdictTone

/** 汇总卡里的下一步入口：按路由去重，避免同一页签重复出现 */
const nextActions = computed(() => {
  const seen = new Set<string>()
  const out: Array<{ label: string; to: string }> = []
  for (const s of flow.value?.steps ?? []) {
    if (!s.action || seen.has(s.action.to)) continue
    seen.add(s.action.to)
    out.push(s.action)
  }
  return out
})

const { buses, devices, selectedBus, selectedDeviceId, reload } = useDebugTargets()
const { report, error: healthError, refresh } = useHealthReport()

// 协议筛选后若当前症状已被过滤掉，自动落到该协议的第一条路径
watch(protocolFilter, () => {
  const list = filteredFlows.value
  if (!list.some((f) => f.id === selectedFlowId.value)) {
    selectedFlowId.value = list[0]?.id ?? ''
  }
})

// 切流程即清空勾选与检查结果，避免把上一症状的结论带到下一条路径
watch(selectedFlowId, () => {
  checked.value = {}
  probeResults.value = {}
  loadingStepId.value = null
})

function go(to: string | undefined) {
  if (!to) return
  void router.push(to)
}

function protocolText(name: string): string {
  return PROTOCOL_LABELS[name as WizardProtocol] ?? name
}

function levelTagType(level: HealthProblem['level']) {
  if (level === 'critical') return 'danger' as const
  if (level === 'warning') return 'warning' as const
  return 'info' as const
}

function setProbe(stepId: string, result: ProbeResult) {
  probeResults.value = { ...probeResults.value, [stepId]: result }
}

/** 健康问题是否与当前排查协议相关：串口协议只看总线/采集类，MQTT 只看上行类 */
function isRelevantProblem(problem: HealthProblem, protocol: WizardProtocol): boolean {
  const text = `${problem.code} ${problem.message}`.toUpperCase()
  if (protocol === 'mqtt') {
    return /MQTT|UPLINK|NORTH|QUEUE|上行|转发|队列/.test(text)
  }
  if (protocol === 'generic') return true
  return /BUS|DEVICE|POINT|COLLECT|MODBUS|DLT|CJT|CRC|TIMEOUT|总线|设备|点位|采集|超时|校验/.test(text)
}

function busProbe(): ProbeResult {
  if (!buses.value.length) {
    return {
      tone: 'warning',
      message: '未配置总线，请先到配置管理创建一条总线',
      goto: { label: '去配置管理', to: '/system/config?tab=wizard' },
    }
  }
  const bus = buses.value.find((b) => b.port_name === selectedBus.value)
  if (!bus) {
    return {
      tone: 'warning',
      message: '尚未选中调试总线，请先选择一条总线再排查',
      goto: { label: '去选择总线', to: '/acquisition/console?tab=packet-editor' },
    }
  }
  const protocol = flow.value?.protocol ?? 'generic'
  const actual = protocolText(busProtocolName(bus))
  if (!busMatchesProtocol(bus, protocol)) {
    return {
      tone: 'warning',
      message: `当前目标总线 ${bus.port_name}（${actual}）与本次排查协议 ${PROTOCOL_LABELS[protocol]} 不匹配，请先切换总线`,
      goto: { label: '去选择总线', to: '/acquisition/console?tab=packet-editor' },
    }
  }
  return {
    tone: 'success',
    message: `当前目标总线 ${bus.port_name}（${actual}）与本次排查协议匹配`,
  }
}

function deviceProbe(): ProbeResult {
  if (!devices.value.length) {
    return {
      tone: 'warning',
      message: '尚未配置任何设备，请先到设备管理添加设备',
      goto: { label: '去设备管理', to: '/devices?tab=devices' },
    }
  }
  const device = devices.value.find((d) => d.id === selectedDeviceId.value)
  if (!device) {
    return {
      tone: 'warning',
      message: `已加载 ${devices.value.length} 台设备，但当前调试设备未选中，请先选择设备`,
      goto: { label: '去选择设备', to: '/acquisition/console?tab=auto-detect' },
    }
  }
  return {
    tone: 'success',
    message: `共 ${devices.value.length} 台设备，当前调试设备：${device.name}（${device.id}）`,
  }
}

async function healthProbe(stepId: string) {
  const protocol = flow.value?.protocol ?? 'generic'
  const next = await refresh(true)
  if (healthError.value && !next) {
    setProbe(stepId, { tone: 'danger', message: `健康报告读取失败：${healthError.value}` })
    return
  }
  const problems = (next?.problems ?? report.value?.problems ?? []).filter((p) =>
    isRelevantProblem(p, protocol),
  )
  setProbe(stepId, {
    tone: problems.length ? 'warning' : 'success',
    message: problems.length
      ? `发现 ${problems.length} 条与本次排查相关的异常`
      : '健康报告未发现相关异常',
    problems,
  })
}

async function runProbe(step: WizardStep) {
  if (!step.probe) return
  loadingStepId.value = step.id
  try {
    if (step.probe === 'health') {
      await healthProbe(step.id)
    } else {
      // 总线/设备检查先重新拉取真实配置，避免使用首次进入页面时的旧快照
      await reload()
      setProbe(step.id, step.probe === 'bus' ? busProbe() : deviceProbe())
    }
  } catch (e: unknown) {
    setProbe(step.id, { tone: 'danger', message: extractErrorMessage(e, '自动检查失败') })
  } finally {
    loadingStepId.value = null
  }
}
</script>

<style scoped>
.dw {
  padding: 16px;
}

.cap {
  margin: 8px 0 12px;
  color: var(--text-secondary);
  font-size: 0.82rem;
}

.section-card {
  margin-bottom: 16px;
}

.filters {
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.filter-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px;
}

.filter-row .lbl {
  color: var(--text-secondary);
  font-size: 0.8rem;
}

.symptom-select {
  width: 340px;
  max-width: 100%;
}

.flow-head {
  display: flex;
  align-items: center;
  gap: 8px;
}

.flow-head .progress {
  margin-left: auto;
  color: var(--text-secondary);
  font-size: 0.8rem;
}

.intro {
  margin: 0 0 4px;
  color: var(--text-secondary);
  font-size: 0.82rem;
  line-height: 1.6;
}

.steps {
  display: flex;
  flex-direction: column;
}

.step {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 12px 0;
  border-bottom: 1px dashed var(--border-base);
}

.step:last-child {
  border-bottom: none;
  padding-bottom: 0;
}

.step.done .title > span:first-child {
  color: var(--text-secondary);
  text-decoration: line-through;
}

.tick {
  margin-top: 2px;
  flex-shrink: 0;
}

.idx {
  width: 22px;
  height: 22px;
  border-radius: 50%;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-size: 0.78rem;
  font-weight: 600;
  color: var(--color-primary);
  background: rgba(var(--color-primary-rgb), 0.1);
}

.body {
  flex: 1;
  min-width: 0;
}

.title {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  font-size: 0.88rem;
  font-weight: 600;
  color: var(--text-primary);
  line-height: 1.5;
}

.detail {
  margin-top: 4px;
  color: var(--text-secondary);
  font-size: 0.8rem;
  line-height: 1.6;
}

.ops {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 8px;
}

.probe {
  margin-top: 8px;
  padding: 8px 10px;
  border-left: 3px solid var(--color-primary);
  border-radius: var(--radius-small);
  background: rgba(var(--color-primary-rgb), 0.06);
  color: var(--text-secondary);
  font-size: 0.8rem;
  line-height: 1.6;
}

.probe.success {
  border-color: var(--color-success);
  background: var(--color-success-bg);
  color: var(--text-success);
}

.probe.warning {
  border-color: var(--color-warning);
  background: var(--color-warning-bg);
  color: var(--text-warning);
}

.probe.danger {
  border-color: var(--color-danger);
  background: var(--color-danger-bg);
  color: var(--text-danger);
}

.problems {
  list-style: none;
  margin: 6px 0 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.problems li {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-primary);
}

/* 汇总卡：结论色调跟随 verdictTone，一眼区分「处理完 / 还有事」 */
.summary {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 16px;
  padding: 12px;
  border: 1px solid var(--border-base);
  border-left: 3px solid var(--color-primary);
  border-radius: var(--radius-small);
  background: var(--bg-fill);
}

.summary-head {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}

.summary-head .probe-count {
  margin-left: auto;
  color: var(--text-secondary);
  font-size: 0.78rem;
}

.summary .verdict {
  color: var(--text-secondary);
  font-size: 0.82rem;
  line-height: 1.6;
}

.summary.danger {
  border-left-color: var(--color-danger);
  background: var(--color-danger-bg);
}

.summary.danger .verdict {
  color: var(--text-danger);
}

.summary.warning {
  border-left-color: var(--color-warning);
  background: var(--color-warning-bg);
}

.summary.warning .verdict {
  color: var(--text-warning);
}

.summary.success {
  border-left-color: var(--color-success);
  background: var(--color-success-bg);
}

.summary.success .verdict {
  color: var(--text-success);
}

.summary .next {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}

.summary .next .lbl {
  color: var(--text-secondary);
  font-size: 0.8rem;
}
</style>
