<template>
  <div class="certs">
    <SectionHeader title="证书签发" icon="Lock" />

    <!-- 当前生效证书（原「证书详情」页签，并入本页）：数据源是网关运行时 TLS 证书，
        与下方证书库列表不同，故保留独立区块而不是塞进表格 -->
    <el-card shadow="never" class="gateway-cert-card">
      <template #header>
        <SectionHeader title="当前生效证书" icon="Lock" />
      </template>
      <CertView />
    </el-card>

    <div class="top-bar">
      <el-button type="primary" @click="showUpload = true"><el-icon><Upload /></el-icon> 上传证书</el-button>
      <el-button type="warning" @click="doRenew"><el-icon><Refresh /></el-icon> 重新生成自签名</el-button>
      <el-button @click="load"><el-icon><Refresh /></el-icon> 刷新</el-button>
      <TablePrefsMenu table-id="certificates" :columns="certColumns.map(c => ({ key: c.key, title: c.title }))" />
    </div>

    <DataTable
      :data="certs"
      :columns="certColumns"
      :hidden-columns="hiddenColumns"
      row-key="id"
      :loading="loading"
      stripe
      empty-text="暂无证书，可上传 PEM 证书或重新生成自签名"
      empty-action-text="上传证书"
      @empty-action="showUpload = true"
    />

    <!-- 上传证书对话框 -->
    <el-dialog :close-on-click-modal="false" v-model="showUpload" title="上传证书" width="560px" :before-close="uploadGuard.beforeClose">
      <el-form label-width="100px">
        <el-form-item label="证书名称"><el-input v-model="form.name" placeholder="请输入证书名称" /></el-form-item>
        <el-form-item label="证书 PEM">
          <el-input v-model="form.certPem" type="textarea" :rows="5" placeholder="粘贴证书 PEM 内容（以 -----BEGIN CERTIFICATE----- 开头）" />
        </el-form-item>
        <el-form-item label="私钥 PEM">
          <el-input v-model="form.keyPem" type="textarea" :rows="5" placeholder="粘贴私钥 PEM 内容" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="uploadGuard.close">取消</el-button>
        <el-button type="primary" @click="upload" :loading="uploading" :disabled="uploading">上传</el-button>
      </template>
    </el-dialog>

    <!-- 证书详情 -->
    <el-dialog v-model="showDetailDialog" title="证书详情" width="560px">
      <el-descriptions v-if="detail" :column="1" border>
        <el-descriptions-item label="名称">{{ detail.name }}</el-descriptions-item>
        <el-descriptions-item label="类型">{{ typeLabel[detail.cert_type] }}</el-descriptions-item>
        <el-descriptions-item label="主题">{{ detail.subject }}</el-descriptions-item>
        <el-descriptions-item label="颁发者">{{ detail.issuer }}</el-descriptions-item>
        <el-descriptions-item label="序列号">{{ detail.serial_number }}</el-descriptions-item>
        <el-descriptions-item label="生效时间">{{ formatDateTime(detail.valid_from * 1000) }}</el-descriptions-item>
        <el-descriptions-item label="失效时间">{{ formatDateTime(detail.valid_to * 1000) }}</el-descriptions-item>
        <el-descriptions-item label="指纹(SHA256)">{{ detail.fingerprint }}</el-descriptions-item>
        <el-descriptions-item label="绑定服务">{{ detail.bound_to || '无' }}</el-descriptions-item>
      </el-descriptions>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted, h } from 'vue'
import { ElButton, ElMessage, ElTag } from 'element-plus'
import { confirmAction } from '@/utils/confirm'
import { Upload, Refresh } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
import {
  listCertificates,
  uploadCertificate,
  renewCertificate,
  type Certificate,
} from '@/api/certificates'
import { formatDate, formatDateTime } from '@/utils/datetime'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import CertView from './CertView.vue'

const certs = ref<Certificate[]>([])
const loading = ref(false)
const uploading = ref(false)
const showUpload = ref(false)
const showDetailDialog = ref(false)
const detail = ref<Certificate | null>(null)
const typeLabel: Record<string, string> = { server: '服务器', client: '客户端', ca: 'CA' }
const form = reactive({ name: '', certPem: '', keyPem: '' })

// 上传证书弹窗未保存修改守卫：X/ESC/取消时若有改动先确认（证书详情为只读弹窗，不接）
const uploadGuard = useDialogDirtyGuard(showUpload, () => form)

function isExpiring(ts: number) {
  return ts * 1000 > Date.now() && ts * 1000 - Date.now() < 86400000 * 30
}

/** 列定义：顺序/宽度与原表格一致；有效期与状态的过期判断保持原有语义 */
const certColumns: ColumnDef<Certificate>[] = [
  { key: 'name', title: '证书名称', width: 160, ellipsis: true },
  {
    key: 'cert_type',
    title: '类型',
    width: 90,
    cell: (row) =>
      h(ElTag, {
        size: 'small',
        effect: 'dark',
        round: true,
        type: row.cert_type === 'ca' ? 'warning' : 'primary',
      }, () => typeLabel[row.cert_type]),
  },
  { key: 'subject', title: '主题', minWidth: 200, ellipsis: true },
  { key: 'issuer', title: '颁发者', minWidth: 160, ellipsis: true },
  {
    key: 'valid_to',
    title: '有效期',
    width: 170,
    cell: (row) =>
      h('span', {
        class: { expiring: isExpiring(row.valid_to), expired: row.valid_to * 1000 < Date.now() },
      }, formatDate(row.valid_to * 1000)),
  },
  {
    key: 'status',
    title: '状态',
    width: 100,
    cell: (row) => {
      if (row.valid_to * 1000 < Date.now()) {
        return h(ElTag, { size: 'small', effect: 'dark', round: true, type: 'danger' }, () => '已过期')
      }
      if (isExpiring(row.valid_to)) {
        return h(ElTag, { size: 'small', effect: 'dark', round: true, type: 'warning' }, () => '即将过期')
      }
      return h(ElTag, { size: 'small', effect: 'dark', round: true, type: 'success' }, () => '有效')
    },
  },
  {
    key: 'bound_to',
    title: '绑定',
    width: 120,
    cell: (row) => h('span', row.bound_to || '无'),
  },
  actionColumn<Certificate>({
    width: 80,
    fixed: 'right',
    cell: (row) =>
      h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => showDetail(row) }, () => '查看'),
  }),
]

const { hidden: hiddenColumns } = useColumnPrefs('certificates')

async function load() {
  loading.value = true
  try {
    certs.value = await listCertificates()
  } catch {
    ElMessage.error('加载证书列表失败')
  } finally {
    loading.value = false
  }
}

async function upload() {
  if (!form.name.trim()) { ElMessage.warning('请输入证书名称'); return }
  if (!form.certPem.trim() || !form.keyPem.trim()) { ElMessage.warning('请输入证书和私钥 PEM'); return }
  uploading.value = true
  try {
    await uploadCertificate(form.name, form.certPem, form.keyPem)
    ElMessage.success('证书已上传，重启网关后生效')
    showUpload.value = false
    form.name = ''; form.certPem = ''; form.keyPem = ''
    load()
  } catch {
    ElMessage.error('上传失败')
  } finally {
    uploading.value = false
  }
}

async function doRenew() {
  if (!(await confirmAction({ message: '确认重新生成自签名证书？现有证书将被覆盖。', title: '重新生成' }))) return
  try {
    await renewCertificate()
    ElMessage.success('证书已重新生成，重启网关后生效')
    load()
  } catch {
    // POST 重新生成：全局拦截器只留痕不弹窗，需在此给出动作语境提示
    ElMessage.error('重新生成证书失败，现有证书未被替换')
  }
}

function showDetail(c: Certificate) {
  detail.value = c
  showDetailDialog.value = true
}

onMounted(load)
</script>

<style scoped>
.certs { padding: 0px; }
.top-bar { display: flex; gap: 8px; margin-bottom: 12px; }
.gateway-cert-card { margin-bottom: 16px; border-radius: var(--radius-card); }
.expiring { color: var(--text-warning); }
.expired { color: var(--text-danger); text-decoration: line-through; }
</style>