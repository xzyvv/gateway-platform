<template>
  <div class="uplink-tester">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="上行测试" icon="Promotion">
          <el-button size="small" @click="loadFromConfig"><el-icon><Document /></el-icon> 从已保存配置加载</el-button>
          <el-button size="small" type="primary" @click="goConfig"><el-icon><Setting /></el-icon> 配置页</el-button>
        </SectionHeader>
      </template>

      <!-- 能力边界明示：本页模板是调试沙盒，与「上送模板」页编辑的生效模板不是同一份数据 -->
      <el-alert
        type="info"
        :closable="false"
        show-icon
        title="本页模板是调试沙盒"
        description="在这里编辑和预览的模板只用于本次渲染与测试，不会写入网关的生效配置。生效模板请到「上送模板」页修改；需要基于生效模板做试验时，点右上角「从已保存配置加载」。"
        style="margin-bottom: 12px"
      />

      <el-row :gutter="16">
        <el-col :xs="24" :md="12">
          <div class="template-section">
            <SectionHeader title="模板内容" icon="EditPen" class="section-header-block">
              <template #extra>
                <el-tag size="small" :type="compileError ? 'danger' : 'success'" effect="dark" round>
                  {{ compileError ? '编译失败' : '就绪' }}
                </el-tag>
              </template>
              <span class="meta">字节数：{{ templateBytes }}</span>
            </SectionHeader>
            <el-input
              v-model="state.template"
              type="textarea"
              :rows="16"
              spellcheck="false"
              class="template-textarea"
              placeholder='{"ts":${timestamp_ms},"device":"${device_id}","value":${value}}'
              aria-label="上行报文模板"
            />
            <div v-if="compileError" class="compile-error">
              <el-alert :title="compileError" type="error" :closable="false" show-icon />
            </div>
            <div class="quick-templates">
              <div class="quick-label">快速应用预设：</div>
              <el-button v-for="tpl in BUILTIN_TEMPLATES" :key="tpl.id" size="small" plain @click="applyTemplate(tpl.id)">
                {{ tpl.name }}
              </el-button>
            </div>
          </div>
        </el-col>

        <el-col :xs="24" :md="12">
          <div class="preview-section">
            <SectionHeader title="测试数据 / 渲染预览" icon="DataLine" class="section-header-block">
              <el-button text size="small" @click="randomizeData"><el-icon><Refresh /></el-icon> 随机示例</el-button>
            </SectionHeader>

            <el-form label-width="90px" size="small">
              <el-form-item label="device_id">
                <el-input v-model="state.vars.device_id" placeholder="电表1" />
              </el-form-item>
              <el-form-item label="device_group">
                <el-input v-model="state.vars.device_group" placeholder="车间A" />
              </el-form-item>
              <el-form-item label="point_id">
                <el-input v-model="state.vars.point_id" placeholder="voltage_a" />
              </el-form-item>
              <el-form-item label="value">
                <el-input-number v-model="valueNum" :precision="2" :step="0.1" style="width: 100%" />
              </el-form-item>
              <el-form-item label="unit">
                <el-input v-model="state.vars.unit" placeholder="V" />
              </el-form-item>
              <el-form-item label="quality">
                <el-select v-model="state.vars.quality_text" style="width: 100%">
                  <el-option label="Good" value="good" />
                  <el-option label="Uncertain" value="uncertain" />
                  <el-option label="Bad" value="bad" />
                  <el-option label="Stale" value="stale" />
                </el-select>
              </el-form-item>
              <el-form-item label="stale_count">
                <el-input-number v-model="staleNum" :min="0" :max="100" style="width: 100%" />
              </el-form-item>
            </el-form>

            <div class="preview-result">
              <div class="preview-label">渲染结果（{{ renderedBytes }} 字节）：</div>
              <pre class="preview-code">{{ rendered || '(请输入模板与测试数据)' }}</pre>
            </div>
          </div>
        </el-col>
      </el-row>

      <div class="footer-actions">
        <el-button @click="copyRendered" :disabled="!rendered"><el-icon><CopyDocument /></el-icon> 复制渲染结果</el-button>
        <el-button type="warning" @click="testSend(false)" :loading="testing" :disabled="!rendered"><el-icon><View /></el-icon> 仅渲染（不下发）</el-button>
        <el-button type="primary" @click="testSend(true)" :loading="sending" :disabled="!rendered"><el-icon><Promotion /></el-icon> 真实上行测试</el-button>
      </div>
    </el-card>

    <el-card v-if="lastResult" shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="发送结果" icon="Document">
          <template #extra>
            <el-tag :type="lastResult.ok ? 'success' : 'danger'" effect="dark" round size="small">
              {{ lastResult.ok ? '成功' : '失败' }}
            </el-tag>
          </template>
        </SectionHeader>
      </template>
      <el-descriptions :column="2" border size="small">
        <el-descriptions-item label="渲染字节">{{ lastResult.bytes }}</el-descriptions-item>
        <el-descriptions-item v-if="lastResult.publish" label="延迟">{{ lastResult.publish.latency_ms }} ms</el-descriptions-item>
        <el-descriptions-item v-if="lastResult.publish" label="主题">{{ lastResult.publish.topic }}</el-descriptions-item>
        <el-descriptions-item v-if="lastResult.publish" label="QoS">{{ lastResult.publish.qos }}</el-descriptions-item>
        <el-descriptions-item v-if="lastResult.publish" label="送达">
          <el-tag :type="lastResult.publish.delivered ? 'success' : 'danger'" effect="plain" round size="small">
            {{ lastResult.publish.delivered ? '已送达' : '未送达' }}
          </el-tag>
        </el-descriptions-item>
      </el-descriptions>
      <div v-if="lastResult.error" style="margin-top: 12px">
        <el-alert :title="lastResult.error" type="error" :closable="false" show-icon />
      </div>
      <div style="margin-top: 12px">
        <div class="preview-label">实际 payload：</div>
        <pre class="preview-code">{{ lastResult.rendered }}</pre>
      </div>
    </el-card>

    <el-card shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="调试历史" icon="Clock">
          <template #extra>
            <el-tag v-if="history.length" type="info" effect="plain" round size="small">{{ history.length }} 条</el-tag>
          </template>
          <el-button text size="small" @click="loadHistory" :loading="historyLoading">
            <el-icon><Refresh /></el-icon> 刷新
          </el-button>
          <el-button text size="small" type="danger" @click="confirmClearHistory" :disabled="!history.length">
            <el-icon><Delete /></el-icon> 清空
          </el-button>
        </SectionHeader>
      </template>
      <empty-state v-if="!history.length && !historyLoading" description="暂无测试历史，真实发送后会自动记录" image-size="80" />
      <DataTable
        v-else
        :data="history"
        :columns="historyColumns"
        :row-key="historyRowKey"
        :loading="historyLoading"
        stripe
      />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { h } from 'vue'
import { ElButton, ElTag } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import type { UplinkTestHistoryItem } from '@/api/uplink'
import { useUplinkTester } from '@/composables/uplink/useUplinkTester'
import { formatDateTime } from '@/utils/datetime'

const {
  testing, sending, lastResult, history, historyLoading,
  state, valueNum, staleNum,
  rendered, templateBytes, renderedBytes, compileError,
  BUILTIN_TEMPLATES,
  applyTemplate, randomizeData, loadFromConfig, goConfig,
  copyRendered, loadHistory, confirmClearHistory, reuseHistory,
  testSend, formatTime,
} = useUplinkTester()

/** 计算字符串字节长度（兼容模板上下文，避免直接使用 Blob） */
function byteSize(s: string | undefined | null): number {
  return new Blob([s ?? '']).size
}

/** DataTable 行键：历史记录 id 唯一，缺失时用随机值兜底 */
const historyRowKey = (row: UplinkTestHistoryItem) => String(row.id ?? Math.random())

// DataTable 列定义：cell 复刻原模板的自定义渲染，宽度与顺序保持原样
const historyColumns: ColumnDef<UplinkTestHistoryItem>[] = [
  {
    key: 'time',
    title: '时间',
    width: 170,
    cell: (row) => h('span', { title: formatDateTime(row.timestamp) }, formatTime(row.timestamp)),
  },
  {
    key: 'send',
    title: '模式',
    width: 100,
    cell: (row) =>
      h(ElTag, { type: row.send ? 'warning' : 'info', effect: 'plain', round: true, size: 'small' }, () =>
        row.send ? '真实发送' : '仅渲染',
      ),
  },
  {
    key: 'delivered',
    title: '结果',
    width: 100,
    cell: (row) =>
      h(ElTag, { type: row.delivered ? 'success' : 'danger', effect: 'dark', round: true, size: 'small' }, () =>
        row.delivered ? '已送达' : '失败',
      ),
  },
  {
    key: 'latency_ms',
    title: '延迟',
    width: 80,
    cell: (row) => h('span', `${row.latency_ms} ms`),
  },
  {
    key: 'bytes',
    title: '字节',
    width: 80,
    cell: (row) => h('span', String(byteSize(row.rendered))),
  },
  {
    key: 'payload',
    title: 'Payload',
    ellipsis: true,
    cell: (row) => h('code', { class: 'history-payload' }, row.rendered),
  },
  actionColumn<UplinkTestHistoryItem>({
    width: 120,
    cell: (row) => h(ElButton, { text: true, size: 'small', onClick: () => reuseHistory(row) }, () => '复用'),
  }),
]
</script>

<style scoped src="./UplinkTester.css"></style>
