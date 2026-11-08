<template>
  <!-- 骨架分支：与真实布局等宽等高占位，避免整页 v-loading 蒙层 -->
  <el-tabs v-if="loading" :model-value="'status'" class="system sk-system">
    <el-tab-pane label="运行状态" name="status">
      <!-- 骨架块本身是灰白色，首屏网络稍慢时容易被当成「页面空白没内容」，补一行文字说明 -->
      <div class="sk-hint">正在加载网关运行信息…</div>
      <div class="system-grid">
        <el-card shadow="never" class="section-card">
          <el-skeleton animated>
            <template #template>
              <el-skeleton-item variant="rect" class="sk-block-lg" />
            </template>
          </el-skeleton>
        </el-card>
        <el-card shadow="never" class="section-card">
          <el-skeleton animated>
            <template #template>
              <el-skeleton-item variant="rect" class="sk-block-lg" />
            </template>
          </el-skeleton>
        </el-card>
      </div>
      <el-card shadow="never" class="section-card">
        <el-skeleton animated>
          <template #template>
            <el-skeleton-item variant="rect" class="sk-block-md" />
          </template>
        </el-skeleton>
      </el-card>
      <el-card shadow="never" class="section-card sk-metrics-card">
        <el-skeleton animated>
          <template #template>
            <el-skeleton-item variant="rect" class="sk-block-xl" />
          </template>
        </el-skeleton>
      </el-card>
    </el-tab-pane>
    <el-tab-pane label="通知与会话" name="notify">
      <el-card v-for="i in 4" :key="i" shadow="never" class="section-card">
        <el-skeleton animated>
          <template #template>
            <el-skeleton-item variant="rect" class="sk-block-md" />
          </template>
        </el-skeleton>
      </el-card>
    </el-tab-pane>
    <el-tab-pane label="审计与维护" name="maintain">
      <el-card v-for="i in 3" :key="i" shadow="never" class="section-card">
        <el-skeleton animated>
          <template #template>
            <el-skeleton-item variant="rect" class="sk-block-md" />
          </template>
        </el-skeleton>
      </el-card>
    </el-tab-pane>
  </el-tabs>

  <div v-else class="system">
    <!-- 原单页堆叠 10 区块过于拥挤，按职责拆为 3 个页内 tab：运行状态 / 通知与会话 / 审计与维护 -->
    <el-tabs v-model="activeTab">
      <el-tab-pane label="运行状态" name="status">
        <div class="system-grid">
          <el-card shadow="never" class="section-card">
            <template #header>
              <SectionHeader title="网关信息" icon="InfoFilled" />
            </template>
            <!-- S9: 7 项信息单列排布在 1366×768 上占掉近 280px，是系统页需滚动 744px 的主因。
                 改为两列（信息量不变），配合下面 NTP 卡的两列排布把首屏压回一屏内。 -->
            <!-- N11: 卡宽约 553px，两列时每个 label+value 只有 260px。默认 140px 的 label
                 宽度会把「构建时间」挤成两行、Git Hash 切成 3 行。这里把 label 收到 100px，
                 并让 Git Hash 独占整行（40 位十六进制本就不该硬折）。 -->
            <el-descriptions :column="2" border label-width="100px">
              <el-descriptions-item label="版本">{{ version.version }}</el-descriptions-item>
              <el-descriptions-item label="构建时间">{{ version.build_time }}</el-descriptions-item>
              <el-descriptions-item label="Git Hash" :span="2">{{ version.git_hash }}</el-descriptions-item>
              <el-descriptions-item label="CPU 使用率">{{ fmtPct(status.cpu_usage) }}</el-descriptions-item>
              <el-descriptions-item label="内存使用率">{{ fmtPct(status.mem_usage) }}</el-descriptions-item>
              <el-descriptions-item label="在线设备">{{ status.online_devices }}</el-descriptions-item>
              <el-descriptions-item label="队列积压">{{ status.queue_length }}</el-descriptions-item>
            </el-descriptions>
          </el-card>

          <el-card shadow="never" class="section-card">
            <template #header>
              <SectionHeader title="NTP 时间同步" icon="Clock" />
            </template>
            <!-- S9: 与左侧系统信息同口径的两列排布，避免同排两张卡高度差过大 -->
            <el-descriptions :column="2" border label-width="100px">
              <el-descriptions-item label="状态">
                <el-tag :type="ntpTagType(ntp.status)" effect="dark" round>
                  {{ ntpStatusMap[ntp.status] || ntp.status || '未同步' }}
                </el-tag>
              </el-descriptions-item>
              <el-descriptions-item label="时钟偏移">{{ ntp.offset_ms != null ? `${ntp.offset_ms} ms` : '-' }}</el-descriptions-item>
              <el-descriptions-item label="当前时间" :span="2">
                <span class="live-clock">{{ nowTime }}</span>
              </el-descriptions-item>
            </el-descriptions>
            <el-button class="ntp-btn" type="primary" @click="syncNtp">
              <el-icon><Refresh /></el-icon> 立即同步
            </el-button>
          </el-card>
        </div>

        <!-- 组件健康状态：与运行总览 / 健康报告 / 自诊断同一份 /api/health/report 快照（B1）。
             报告不含 NTP 与磁盘余量，故 NTP 卡与上方系统信息仍走各自的既有接口。
             R2：网关级结论收敛到「运维 › 健康报告」唯一结论页 —— 这里只留一行摘要 + 深链，
             综合状态不再由本页重复渲染（原先 HealthStatus 头部的状态标签已移除）。 -->
        <HealthConclusionBar :problems="healthProblems" :status="healthOverallStatus" />
        <HealthStatus :health-rows="healthRows" :loading="healthLoading" />
        <HealthProblemList :problems="healthProblems" title="健康问题" />

        <el-card shadow="never" class="section-card">
          <template #header>
            <SectionHeader title="运行指标" icon="DataAnalysis" />
          </template>
          <pre class="metrics" v-html="metricsHtml"></pre>
        </el-card>
      </el-tab-pane>

      <el-tab-pane label="通知与会话" name="notify">
        <el-card shadow="never" class="section-card">
          <template #header>
            <SectionHeader title="变更通知（Webhook）" icon="Connection">
              <template #extra>
                <el-tag v-if="webhook.isEnabled()" type="success" effect="dark" round size="small">已启用</el-tag>
                <el-tag v-else type="info" effect="plain" round size="small">未启用</el-tag>
              </template>
            </SectionHeader>
          </template>
          <div class="webhook-desc">
            配置 Webhook URL 后，关键事件（配置保存、OTA 完成、日志告警）将自动 POST 到此地址。
          </div>
          <div class="webhook-form">
            <el-input v-model="webhookUrl" placeholder="https://example.com/webhook" aria-label="Webhook 地址" clearable>
              <template #prefix><el-icon><Link /></el-icon></template>
            </el-input>
            <el-button type="primary" @click="saveWebhook">
              <el-icon><Check /></el-icon> 保存
            </el-button>
            <el-button @click="testWebhook" :disabled="!webhookUrl">
              <el-icon><Promotion /></el-icon> 测试
            </el-button>
          </div>
        </el-card>

        <!-- 设备端真实死信：数据在网关本地，换电脑仍然存在（与下方本机通知队列严格区分） -->
        <DeviceDeadLetterPanel />

        <!-- 本机浏览器通知重试项：不是设备数据死信，不随设备迁移 -->
        <LocalNotifyQueuePanel
          :items="notifyQueueRows"
          @retry="webhook.retryNotify"
          @retry-all="retryAllNotify"
          @clear="webhook.clearNotifyQueue()"
        />

        <el-card shadow="never" class="section-card">
          <template #header>
            <SectionHeader title="上行转发监控" icon="Connection" />
          </template>
          <!-- P0-1：上行通道的连接状态、消息积压与成功失败统计原先在此处另查一遍
               /api/north/status，与「上行转发 › 上行服务状态」页重复。现统一由该页呈现，
               本页只保留入口，避免同一份数据两处口径不一致。 -->
          <p class="diag-tip">
            上行通道的连接状态、消息积压与成功失败统计统一在
            <strong>「上行转发 › 上行服务状态」</strong> 查看，本页不再重复一份转发表。
          </p>
          <div class="backup-actions">
            <el-button type="primary" @click="goNorthStatus">
              <el-icon><Connection /></el-icon> 打开上行服务状态
            </el-button>
          </div>
        </el-card>

        <el-card shadow="never" class="section-card">
          <template #header>
            <SectionHeader title="登录会话管理" icon="User">
              <template #extra><LocalOnlyBadge tip="网关仅支持单 token，会话列表为前端本地模拟" /></template>
            </SectionHeader>
          </template>
          <DataTable :data="sessionList" :columns="sessionColumns" row-key="id" stripe empty-text="暂无会话数据" />
        </el-card>
      </el-tab-pane>

      <el-tab-pane label="审计与维护" name="maintain">
        <!-- B3：审计只有一条链路 —— 后端 /api/audit/logs（字段 ts/action/target/detail/source）。
             后端无删除端点，故这里只保留刷新，不再提供「清空」。 -->
        <el-card shadow="never" class="section-card">
          <template #header>
            <SectionHeader title="操作审计日志" icon="Document">
              <template #extra>
                <el-tag type="info" effect="plain" round size="small">{{ auditEntries.length }} 条</el-tag>
              </template>
              <el-button size="small" @click="refreshAudit">
                <el-icon><Refresh /></el-icon> 刷新
              </el-button>
            </SectionHeader>
          </template>
          <DataTable :data="auditEntries" :columns="auditColumns" row-key="ts" stripe :max-height="400" empty-text="后端暂无审计记录" />
        </el-card>

        <el-card shadow="never" class="section-card">
          <template #header>
            <SectionHeader title="一键诊断报告" icon="FirstAidKit" />
          </template>
          <p class="diag-tip">采集系统状态、健康检查、NTP、设备连通性、WS 连接态、<strong>网关采集错误与网关侧失败的 API 请求</strong>，生成可导出的 HTML 诊断报告。</p>
          <div class="backup-actions">
            <el-button type="primary" :loading="diagLoading" @click="genDiagnostic">
              <el-icon><Download /></el-icon> 生成并下载诊断报告
            </el-button>
            <el-button :loading="bundleLoading" @click="genDiagnosticBundle">
              <el-icon><Files /></el-icon> 导出诊断包(.zip)
            </el-button>
          </div>
          <p class="diag-tip diag-tip-sub">诊断包 = 报告 + 请求 HAR + 连接状态与 traceId 清单，适合完整转交研发。</p>
        </el-card>

        <el-card shadow="never" class="section-card">
          <template #header>
            <SectionHeader title="系统备份 / 还原" icon="FolderOpened" />
          </template>
          <p class="diag-tip">将当前配置、配置模板、审计日志打包为 JSON 备份文件，可下载或从文件还原。</p>
          <div class="backup-actions">
            <el-button type="primary" :loading="backupLoading" @click="doBackup">
              <el-icon><Download /></el-icon> 生成并下载备份
            </el-button>
            <el-upload :auto-upload="false" :show-file-list="false" accept=".json" :on-change="onRestoreFile">
              <el-button type="warning">
                <el-icon><Upload /></el-icon> 从备份文件还原
              </el-button>
            </el-upload>
          </div>
          <el-descriptions v-if="lastBackup" :column="2" border style="margin-top: 12px">
            <el-descriptions-item label="备份时间">{{ formatDateTime(lastBackup.createdAt) }}</el-descriptions-item>
            <el-descriptions-item label="网关版本">{{ lastBackup.gatewayVersion || '—' }}</el-descriptions-item>
            <el-descriptions-item label="总线数">{{ lastBackup.meta.busCount }}</el-descriptions-item>
            <el-descriptions-item label="设备数">{{ lastBackup.meta.deviceCount }}</el-descriptions-item>
            <el-descriptions-item label="点位数">{{ lastBackup.meta.pointCount }}</el-descriptions-item>
            <el-descriptions-item label="审计日志">{{ lastBackup.auditLogs.length }} 条</el-descriptions-item>
          </el-descriptions>
        </el-card>

        <el-card shadow="never" class="section-card">
          <template #header>
            <SectionHeader title="密钥脱敏状态" icon="Lock" />
          </template>
          <!-- F-09：原「数据加密」页用 btoa 冒充加密、轮换是空操作，已删除。
               这里只如实反映 GET /api/config 的脱敏结果，不再伪造「已加密」状态。 -->
          <p class="diag-tip">
            网关侧的凭据由服务端自身保管。<code>GET /api/config</code> 返回的密钥字段已被服务端脱敏，
            <strong>Web 面拿不到明文或密文</strong>，也不会在此做「加密 / 密钥轮换」——这两件事只在网关本机完成。
          </p>
          <DataTable :data="secretRows" :columns="secretColumns" row-key="key" stripe empty-text="暂无凭据配置" />
        </el-card>
      </el-tab-pane>
    </el-tabs>
  </div>
</template>

<script setup lang="ts">
import { computed, h, ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ElButton, ElTag } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn, timeColumn } from '@/utils/commonColumns'
import { getConfig } from '@/api/config'
import type { AuditLogEntry } from '@/api/auditLog'
import type { LoginSession } from '@/composables/system/useSessions'
import { useSystem } from '@/composables/system/useSystem'
import { formatDateTime } from '@/utils/datetime'
import HealthStatus from '@/views/overview/HealthStatus.vue'
import HealthProblemList from '@/views/ops/health/HealthProblemList.vue'
import HealthConclusionBar from '@/components/HealthConclusionBar.vue'
import DeviceDeadLetterPanel from '@/views/uplink/components/DeviceDeadLetterPanel.vue'
import LocalNotifyQueuePanel from './components/LocalNotifyQueuePanel.vue'

const activeTab = ref('status')

/**
 * 组件健康区块的数据源在 useSystem 内：复用 /api/health/report 的模块级快照
 * （与运行总览 / 健康报告 / 自诊断同一口径），不再读 Layout 的 /health/ready 副本。
 */
const router = useRouter()
/** 跳转到唯一的上行状态页（上行服务状态）；本页不再自持一份转发表 */
function goNorthStatus() {
  router.push('/uplink?tab=north-status')
}

const {
  auditEntries, refreshAudit,
  webhook, webhookUrl, saveWebhook, testWebhook, retryAllNotify,
  sessionList, revokeSession,
  status, version, ntp, loading,
  healthRows, healthProblems, healthLoading, healthOverallStatus,
  ntpStatusMap, ntpTagType, nowTime, metricsHtml, fmtPct,
  syncNtp,
  diagLoading, genDiagnostic, bundleLoading, genDiagnosticBundle,
  backupLoading, lastBackup, doBackup, onRestoreFile,
} = useSystem()

/** 审计列：后端字段 ts/action/target/detail/source 直接展示，不额外加工 */
const auditColumns: ColumnDef<AuditLogEntry>[] = [
  timeColumn<AuditLogEntry>('ts', { width: 180 }),
  { key: 'source', title: '操作来源', width: 120, ellipsis: true },
  { key: 'action', title: '操作', width: 160, ellipsis: true },
  { key: 'target', title: '对象', minWidth: 160, ellipsis: true },
  { key: 'detail', title: '详情', minWidth: 220, ellipsis: true },
]

/**
 * 本机通知重试队列（localStorage）：只做展示，重试/清空仍由 useWebhook 负责。
 * 与设备端死信（DeviceDeadLetterPanel）是两件不同的事，勿混用文案。
 */
const notifyQueueRows = computed(() => webhook.notifyRetryQueue.value)

const sessionColumns: ColumnDef<LoginSession>[] = [
  { key: 'loginAt', title: '登录时间', width: 170, cell: (row) => formatDateTime(row.loginAt) },
  { key: 'ip', title: '来源 IP', width: 140 },
  { key: 'userAgent', title: '浏览器', minWidth: 200, ellipsis: true },
  { key: 'lastActiveAt', title: '最近活跃', width: 170, cell: (row) => formatDateTime(row.lastActiveAt) },
  actionColumn<LoginSession>({
    width: 110,
    cell: (row) =>
      row.current
        ? h(ElTag, { size: 'small', type: 'success', effect: 'dark' }, () => '当前')
        : h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => revokeSession(row.id) }, () => '强制下线'),
  }),
]

/**
 * F-09：密钥脱敏状态（替代原「数据加密」页）。
 * 只读取后端已经脱敏的 GET /api/config，不展示、也不尝试还原任何密文。
 */
type SecretState = 'masked' | 'empty' | 'plain'
interface SecretRow { key: string; name: string; state: SecretState }
const SECRET_STATE_LABEL: Record<SecretState, string> = {
  masked: '已脱敏',
  empty: '未配置',
  plain: '未脱敏（异常）',
}
/** 服务端与 HTTP 拦截器使用同一组占位符；出现占位符即视为已脱敏 */
const MASKED_MARKERS = new Set(['***', 'REDACTED_SECRET'])

const secretRows = ref<SecretRow[]>([])
const secretColumns: ColumnDef<SecretRow>[] = [
  { key: 'name', title: '凭据', minWidth: 200 },
  {
    key: 'state',
    title: '状态',
    width: 140,
    cell: (row) =>
      h(
        ElTag,
        {
          size: 'small',
          effect: 'dark',
          round: true,
          type: row.state === 'masked' ? 'success' : row.state === 'empty' ? 'info' : 'danger',
        },
        () => SECRET_STATE_LABEL[row.state],
      ),
  },
]

async function loadSecretState() {
  try {
    const cfg = await getConfig() as {
      mqtt?: { username?: string; password?: string }
      web?: { api_token?: string; password_hash?: string }
    }
    const picks: { key: string; name: string; value?: string }[] = [
      { key: 'mqtt_username', name: 'MQTT Broker 用户名', value: cfg.mqtt?.username },
      { key: 'mqtt_password', name: 'MQTT Broker 密码', value: cfg.mqtt?.password },
      { key: 'web_api_token', name: 'Web API Token', value: cfg.web?.api_token },
      { key: 'web_password', name: 'Web 管理员密码', value: cfg.web?.password_hash },
    ]
    secretRows.value = picks.map(({ key, name, value }) => {
      const v = (value ?? '').trim()
      return { key, name, state: v === '' ? 'empty' : MASKED_MARKERS.has(v) ? 'masked' : 'plain' }
    })
  } catch {
    // 读取配置失败（未登录/网络异常）时不臆造状态，留空由表格 empty-state 提示
    secretRows.value = []
  }
}

onMounted(loadSecretState)
</script>

<style scoped src="./System.css"></style>
