<template>
  <div class="me">
    <SectionHeader title="异常码速查" icon="InfoFilled" />

    <!-- 诚实标注：本页是协议规范知识库，不是本网关的运行期错误统计 -->
    <el-alert
      type="info"
      :closable="false"
      show-icon
      class="protocol-note"
      title="静态知识库：以下码表来自协议规范，用于对照排查；不反映本网关运行期的错误次数与分布。"
    />

    <!-- 协议切换：Modbus 的码表来自后端静态知识库，DLT645/CJT188 来自前端协议知识库 -->
    <el-radio-group v-model="protocol" size="small" class="protocol-switch">
      <el-radio-button value="modbus">Modbus RTU</el-radio-button>
      <el-radio-button value="dlt645">DL/T 645-2007</el-radio-button>
      <el-radio-button value="cjt188">CJ/T 188-2018</el-radio-button>
    </el-radio-group>

    <el-input
      v-model="keyword"
      :placeholder="searchPlaceholder"
      aria-label="搜索异常码"
      clearable
      style="margin-bottom: 12px"
    />

    <el-alert
      v-if="protocolNote"
      type="info"
      :closable="false"
      show-icon
      class="protocol-note"
      :title="protocolNote"
    />

    <el-row :gutter="16">
      <el-col :span="14">
        <el-card shadow="never" class="section-card">
          <template #header><strong>{{ tableTitle }}</strong></template>
          <DataTable
            :data="filtered"
            :columns="errorColumns"
            row-key="code"
            stripe
            empty-text="暂无异常码"
            :caption="`${tableTitle}（共 ${filtered.length} 条）`"
          />
        </el-card>
      </el-col>
      <el-col :span="10">
        <el-card shadow="never" class="section-card" v-if="selected">
          <template #header><strong>详情</strong></template>
          <h3>{{ selected.code }} - {{ selected.name }}</h3>
          <p><b>含义：</b>{{ selected.meaning }}</p>
          <p><b>可能原因：</b>{{ selected.cause }}</p>
          <p><b>建议处理方式：</b>{{ selected.solution }}</p>
        </el-card>
      </el-col>
    </el-row>

    <!-- 常见异常码解析（Modbus 专属，来自后端知识库） -->
    <el-card v-if="protocol === 'modbus'" shadow="never" class="section-card">
      <template #header><strong>常见异常码解析</strong></template>
      <el-collapse>
        <el-collapse-item v-for="(c, i) in issues" :key="i" :title="c.symptom">
          <ul>
            <li v-for="(chk, j) in c.checks" :key="j">{{ chk }}</li>
          </ul>
        </el-collapse-item>
      </el-collapse>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { h, ref, computed, onMounted } from 'vue'
import { ElTag } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { listModbusErrorsView as listErrors, listCommonIssuesView as getCommonIssues, type ModbusErrorView as ModbusError, type CommonIssueView } from '@/api/diagnostics'
import {
  buildProtocolErrorEntries,
  PROTOCOL_ERROR_NOTES,
  PROTOCOL_ERROR_PROTOCOL_LABEL,
  type ProtocolErrorEntry,
  type ProtocolErrorProtocol,
} from '@/utils/protocolErrorCodes'

type ProtocolTab = 'modbus' | ProtocolErrorProtocol

const protocol = ref<ProtocolTab>('modbus')
const errors = ref<ModbusError[]>([])
const keyword = ref('')
const issues = ref<CommonIssueView[]>([])

/**
 * 三种协议的码表来源不同，统一成同一行结构后渲染：
 * - Modbus：后端静态知识库接口（GET /api/diagnostics/modbus-errors）
 * - DLT645 / CJT188：前端协议知识库（utils/protocolErrorCodes.ts）
 *
 * 两类协议的码形态也不同：Modbus 是「一码一条」，DLT645 的 ERR 是位标志、
 * CJT188 的状态 ST 是「位标志 + 诊断码」，因此行键用 code 而非码值本身。
 */
const protocolEntries = computed<ProtocolErrorEntry[]>(() => {
  if (protocol.value === 'modbus') {
    return errors.value.map((e) => ({
      code: e.code,
      name: e.name,
      meaning: e.meaning,
      cause: e.cause,
      solution: e.solution,
    }))
  }
  return buildProtocolErrorEntries(protocol.value)
})

const tableTitle = computed(() =>
  protocol.value === 'modbus'
    ? 'Modbus 异常码表'
    : `${PROTOCOL_ERROR_PROTOCOL_LABEL[protocol.value]} 异常码表`,
)

const searchPlaceholder = computed(() =>
  protocol.value === 'modbus'
    ? '搜索异常码或名称，如 02 / Illegal'
    : '搜索码位或名称，如 bit2 / 密码 / 电池',
)

/** 协议补充说明：DLT645/CJT188 的码机制与 Modbus 差异大，需要先讲清楚 */
const protocolNote = computed(() =>
  protocol.value === 'modbus' ? '' : PROTOCOL_ERROR_NOTES[protocol.value],
)

// 表格统一走 DataTable：搜索后的 filtered 直接作为数据源，行键用异常码
const errorColumns: ColumnDef<ProtocolErrorEntry>[] = [
  {
    key: 'code',
    title: '码',
    width: 110,
    cell: (row) => h(ElTag, { size: 'small', type: 'danger', effect: 'dark' }, () => row.code),
  },
  { key: 'name', title: '名称', minWidth: 180 },
  { key: 'meaning', title: '含义', minWidth: 120 },
]

const filtered = computed(() => {
  const q = keyword.value.trim().toLowerCase()
  if (!q) return protocolEntries.value
  return protocolEntries.value.filter(
    (e) =>
      e.code.toLowerCase().includes(q) ||
      e.name.toLowerCase().includes(q) ||
      e.meaning.toLowerCase().includes(q),
  )
})
const selected = computed(() => filtered.value[0])

onMounted(async () => {
  // Modbus 码表来自后端；后端不可用时不影响另外两个协议的前端码表
  try {
    errors.value = await listErrors()
    issues.value = await getCommonIssues()
  } catch {
    errors.value = []
  }
})
</script>

<style scoped>
.me { padding: 16px; }
.section-card { margin-bottom: 16px; }
/* 协议切换：与表格之间留出间距 */
.protocol-switch { margin-bottom: 12px; }
/* 协议机制说明：DLT645/CJT188 的错误表达方式与 Modbus 差异大，需要先讲清楚 */
.protocol-note { margin-bottom: 12px; }
.protocol-note :deep(.el-alert__title) {
  line-height: 1.7;
  font-size: 0.8125rem;
}
h3 { margin: 0 0 10px; }
p { margin: 6px 0; line-height: 1.6; }
ul { padding-left: 20px; line-height: 2; }
</style>
