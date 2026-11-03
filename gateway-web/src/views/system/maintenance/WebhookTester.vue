<template>
  <div class="webhook">
    <SectionHeader title="Webhook 测试" icon="Link" />

    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="请求配置" icon="Edit" />
      </template>
      <el-form label-width="80px">
        <el-form-item label="URL">
          <el-input v-model="req.url" placeholder="https://example.com/api/webhook" />
        </el-form-item>
        <el-form-item label="方法">
          <el-select v-model="req.method" style="width: 120px">
            <el-option label="POST" value="POST" />
            <el-option label="PUT" value="PUT" />
            <el-option label="GET" value="GET" />
          </el-select>
        </el-form-item>
        <el-form-item label="请求头">
          <el-input v-model="headersStr" type="textarea" :rows="2" placeholder='{"Content-Type":"application/json","Authorization":"Bearer xxx"}' />
        </el-form-item>
        <el-form-item label="请求体" v-if="req.method !== 'GET'">
          <el-input v-model="req.body" type="textarea" :rows="6" placeholder='{"event":"data","device":"meter_1","value":220.5}' />
        </el-form-item>
        <el-form-item>
          <el-button type="primary" :loading="sending" @click="send">
            <el-icon><Promotion /></el-icon> 发送请求
          </el-button>
          <el-button @click="loadSample">加载示例</el-button>
        </el-form-item>
      </el-form>
    </el-card>

    <!-- 响应结果 -->
    <el-card v-if="response" shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="响应结果" icon="Document">
          <template #extra>
            <el-tag :type="response.ok ? 'success' : 'danger'" effect="dark" round size="small">
              {{ response.ok ? '成功' : '失败' }}
            </el-tag>
            <el-tag type="info" effect="plain" round size="small">{{ response.status }} {{ response.status_text }}</el-tag>
            <el-tag type="warning" effect="plain" round size="small">{{ response.latency }} ms</el-tag>
          </template>
        </SectionHeader>
      </template>
      <div v-if="response.error" class="error-msg">
        <el-alert :title="response.error" type="error" :closable="false" show-icon />
      </div>
      <div class="response-body">
        <div class="resp-label">响应体：</div>
        <pre>{{ response.response_body || '(空)' }}</pre>
      </div>
      <div v-if="Object.keys(response.response_headers).length" class="response-headers">
        <div class="resp-label">响应头：</div>
        <el-descriptions :column="1" border size="small">
          <el-descriptions-item v-for="(v, k) in response.response_headers" :key="k" :label="k">{{ v }}</el-descriptions-item>
        </el-descriptions>
      </div>
    </el-card>

    <!-- 历史记录 -->
    <el-card shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="请求历史" icon="Clock">
          <el-button size="small" link @click="clearHistory">清空</el-button>
        </SectionHeader>
      </template>
      <DataTable :data="history" :columns="historyColumns" row-key="id" stripe empty-text="暂无测试记录" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted, h } from 'vue'
import { ElMessage, ElTag } from 'element-plus'
import { Promotion } from '@element-plus/icons-vue'
import { testWebhook, type WebhookTestResponse } from '@/api/webhook'
import type { ColumnDef } from '@/components/DataTable.vue'
import { timeColumn } from '@/utils/commonColumns'
import { scopedGet, scopedSet } from '@/utils/gatewayStorage'

interface HistoryItem {
  id: string
  url: string
  method: string
  status: number
  latency: number
  ok: boolean
  createdAt: number
}

const HISTORY_KEY = 'webhook_test_history'

const req = reactive({ url: '', method: 'POST' as 'POST' | 'PUT' | 'GET', body: '' })
const headersStr = ref('{"Content-Type":"application/json"}')
const sending = ref(false)
const response = ref<WebhookTestResponse | null>(null)
const history = ref<HistoryItem[]>([])

/** methodTag 返回 string，h() 渲染 el-tag 需收窄为具体标签色 */
type TagType = 'primary' | 'success' | 'info' | 'warning' | 'danger'

// 表格统一走 DataTable：历史记录最多 20 条，默认走 el-table
const historyColumns: ColumnDef<HistoryItem>[] = [
  timeColumn<HistoryItem>('createdAt'),
  {
    key: 'method',
    title: '方法',
    width: 70,
    cell: (row) => h(ElTag, { size: 'small', type: methodTag(row.method) as TagType }, () => row.method),
  },
  { key: 'url', title: 'URL', ellipsis: true },
  {
    key: 'status',
    title: '状态',
    width: 80,
    cell: (row) => h(ElTag, { size: 'small', type: row.ok ? 'success' : 'danger' }, () => String(row.status)),
  },
  { key: 'latency', title: '耗时', width: 80, cell: (row) => `${row.latency} ms` },
]

function methodTag(m: string) {
  return ({ POST: 'primary', PUT: 'warning', GET: 'success' } as Record<string, string>)[m] || 'info'
}

function loadSample() {
  req.url = 'https://example.com/api/webhook'
  req.method = 'POST'
  headersStr.value = '{"Content-Type":"application/json","Authorization":"Bearer test-token"}'
  req.body = JSON.stringify({ event: 'data_report', device: 'meter_1', value: 220.5, timestamp: Date.now() }, null, 2)
}

function loadHistory() {
  try {
    const raw = scopedGet(HISTORY_KEY)
    history.value = raw ? JSON.parse(raw) : []
  } catch {
    history.value = []
  }
}

function saveHistory() {
  scopedSet(HISTORY_KEY, JSON.stringify(history.value))
}

function clearHistory() {
  history.value = []
  saveHistory()
}

async function send() {
  if (!req.url) { ElMessage.warning('请输入 URL'); return }
  let headers: Record<string, string>
  try { headers = JSON.parse(headersStr.value) } catch { ElMessage.warning('请求头格式不正确'); return }

  sending.value = true
  response.value = null
  try {
    const r = await testWebhook({ ...req, headers, body: req.body })
    response.value = r
    const item: HistoryItem = {
      id: `wh_${Date.now()}`,
      url: req.url,
      method: req.method,
      status: r.status,
      latency: r.latency,
      ok: r.ok,
      createdAt: Date.now(),
    }
    history.value.unshift(item)
    if (history.value.length > 20) history.value.pop()
    saveHistory()
  } catch {
    ElMessage.error('请求发送失败')
  } finally {
    sending.value = false
  }
}

onMounted(loadHistory)
</script>

<style scoped>
.webhook { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.error-msg { margin-bottom: 12px; }
.response-body { margin-top: 12px; }
.resp-label { font-size: 0.8125rem; color: var(--el-text-color-secondary); margin-bottom: 6px; }
.response-body pre { background: var(--el-fill-color-light); padding: 12px; border-radius: var(--radius-button); font-size: 0.75rem; overflow-x: auto; max-height: 300px; }
.response-headers { margin-top: 12px; }
</style>