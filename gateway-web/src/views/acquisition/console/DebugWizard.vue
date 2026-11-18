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
import EmptyState from '@/components/EmptyState.vue'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import { useHealthReport } from '@/composables/system/useHealthReport'
import { extractErrorMessage } from '@/utils/errorHandler'
import {
  PROTOCOL_LABELS,
  WIZARD_PROTOCOLS,
  debugWizardFlows,
  type WizardFlow,
  type WizardProtocol,
  type WizardStep,
} from './parts/debugWizardFlows'
import {
  levelTagType,
  runBusProbe,
  runDeviceProbe,
  runHealthProbe,
  type ProbeResult,
} from './parts/debugWizardProbes'
import {
  computeNextActions,
  computeProbeStats,
  computeVerdictText,
  computeVerdictTone,
} from './parts/debugWizardVerdict'

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
const probeStats = computed(() =>
  computeProbeStats(
    flowSteps.value.map((i) => i.probe).filter((p): p is ProbeResult => p != null),
  ),
)

/** 汇总结论的输入快照：勾选进度 + 检查项总数 + 自动检查统计 */
const verdictInput = computed(() => ({
  doneCount: doneCount.value,
  stepTotal: flow.value?.steps.length ?? 0,
  probeTotal: probeTotal.value,
  probes: flowSteps.value.map((i) => i.probe).filter((p): p is ProbeResult => p != null),
}))

/** 汇总结论的色调：严重 > 异常 > 未完成 > 已确认但自动检查未跑完 > 全部通过 */
const verdictTone = computed(() => computeVerdictTone(verdictInput.value, probeStats.value))

const verdictText = computed(() => computeVerdictText(verdictInput.value, probeStats.value))

const allClear = computed(() => verdictTone.value === 'success')
const summaryTagType = verdictTone

/** 汇总卡里的下一步入口：按路由去重，避免同一页签重复出现 */
const nextActions = computed(() => computeNextActions(flow.value?.steps ?? []))

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

function setProbe(stepId: string, result: ProbeResult) {
  probeResults.value = { ...probeResults.value, [stepId]: result }
}

async function runProbe(step: WizardStep) {
  if (!step.probe) return
  loadingStepId.value = step.id
  try {
    if (step.probe === 'health') {
      const protocol = flow.value?.protocol ?? 'generic'
      const next = await refresh(true)
      setProbe(
        step.id,
        runHealthProbe(next, report.value, protocol, healthError.value),
      )
    } else {
      // 总线/设备检查先重新拉取真实配置，避免使用首次进入页面时的旧快照
      await reload()
      const protocol = flow.value?.protocol ?? 'generic'
      setProbe(
        step.id,
        step.probe === 'bus'
          ? runBusProbe({ buses: buses.value, selectedBus: selectedBus.value, protocol })
          : runDeviceProbe({ devices: devices.value, selectedDeviceId: selectedDeviceId.value }),
      )
    }
  } catch (e: unknown) {
    setProbe(step.id, { tone: 'danger', message: extractErrorMessage(e, '自动检查失败') })
  } finally {
    loadingStepId.value = null
  }
}
</script>

<style scoped src="./DebugWizard.css"></style>
