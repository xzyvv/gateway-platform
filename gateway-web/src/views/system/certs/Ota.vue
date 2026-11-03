<template>
  <div class="ota">
    <el-card shadow="never" class="section-card">
      <div class="ota-grid">
        <div class="ota-panel">
          <div class="panel-title">
            <el-icon><Monitor /></el-icon>
            <span>当前状态</span>
          </div>
          <el-descriptions :column="1" border>
            <el-descriptions-item label="升级状态">
              <el-tag :type="statusType" effect="dark" round>{{ statusLabel }}</el-tag>
            </el-descriptions-item>
            <el-descriptions-item label="进度">{{ ota.progress }}%</el-descriptions-item>
            <el-descriptions-item label="错误信息" v-if="ota.error">
              <span class="error-text">{{ ota.error }}</span>
            </el-descriptions-item>
          </el-descriptions>
          <el-progress
            v-if="showProgress"
            :percentage="ota.progress"
            :status="ota.status === 'error' ? 'exception' : ota.progress === 100 ? 'success' : undefined"
            :striped="ota.status === 'upgrading'"
            :striped-flow="ota.status === 'upgrading'"
            class="ota-progress"
            :stroke-width="10"
          />
        </div>

        <div v-if="canOtaUpgrade" class="ota-panel">
          <div class="panel-title">
            <el-icon><FolderOpened /></el-icon>
            <span>上传固件</span>
          </div>
          <el-upload
            ref="uploadRef"
            drag
            action="#"
            :auto-upload="false"
            :http-request="() => {}"
            :on-change="onFileChange"
            :on-remove="onFileRemove"
            :limit="1"
            :on-exceed="onFileExceed"
            accept=".bin,.img,.fw"
            class="ota-upload"
          >
            <el-icon class="el-icon--upload" :size="40"><UploadFilled /></el-icon>
            <div class="el-upload__text">将固件文件拖到此处，或<em>点击上传</em></div>
            <template #tip>
              <div class="el-upload__tip">支持 .bin / .img / .fw 格式</div>
            </template>
          </el-upload>
          <div v-if="firmware" class="file-info">
            <el-icon class="file-icon"><Document /></el-icon>
            <div class="file-detail">
              <div class="file-name">{{ firmware.name }}</div>
              <div class="file-meta">{{ formatSize(firmware.size) }} · {{ firmware.type || 'firmware' }}</div>
            </div>
          </div>
          <el-button
            type="primary"
            class="upload-btn"
            :loading="uploading"
            :disabled="!firmware"
            @click="doUpload"
          >
            <el-icon><Upload /></el-icon> 开始升级
          </el-button>
        </div>
      </div>

      <div v-if="canOtaRollback" class="rollback-panel">
        <div class="rollback-info">
          <el-icon class="rollback-icon"><Warning /></el-icon>
          <div>
            <div class="rollback-title">固件回滚</div>
            <div class="rollback-desc">回滚到上一个固件版本。仅在升级失败或新版本有问题时使用。</div>
          </div>
        </div>
        <el-button type="warning" :disabled="uploading || ota.status === 'upgrading'" @click="doRollback">
          <el-icon><RefreshLeft /></el-icon> 回滚固件
        </el-button>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, computed, onUnmounted } from 'vue'
import { usePolling } from '@/composables/core/usePolling'
import { ElMessage } from 'element-plus'
import { confirmAction } from '@/utils/confirm'
import { getOtaStatus, otaUpload, otaRollback } from '@/api/ota'
import { useWebhook } from '@/composables/uplink/useWebhook'
import { usePermission } from '@/composables/auth/usePermission'
const webhook = useWebhook()
// 按钮级权限：升级需 ota.upgrade，回滚需 ota.rollback
const { can } = usePermission()
const canOtaUpgrade = computed(() => can('ota.upgrade'))
const canOtaRollback = computed(() => can('ota.rollback'))

const ota = reactive<{ progress: number; status: string; error?: string }>({
  progress: 0,
  status: 'idle',
})
const firmware = ref<File | null>(null)
const uploading = ref(false)
const uploadRef = ref()

const statusType = computed(() => {
  // 仅以 status 字段为准，避免 progress=100 但 status 仍为 idle 时误判
  if (ota.status === 'error') return 'danger'
  if (ota.status === 'success') return 'success'
  if (ota.status === 'upgrading') return 'warning'
  return 'info'
})

const statusLabel = computed(() => {
  const map: Record<string, string> = {
    idle: '空闲',
    upgrading: '升级中',
    success: '升级成功',
    error: '升级失败',
  }
  return map[ota.status || 'idle'] || ota.status || '空闲'
})

// 进度条展示条件：进度 > 0 或正在升级中时显示；后端 idle+progress=0 时自动隐藏
// 避免升级完成后 success 状态残留进度条
const showProgress = computed(() => {
  return ota.progress > 0 || ota.status === 'upgrading'
})

// on-change 在文件增删时都会触发，以 fileList 为准避免移除后残留引用
function onFileChange(_file: unknown, fileList: { raw: File }[]) {
  firmware.value = fileList.length > 0 ? fileList[fileList.length - 1]!.raw : null
}

function onFileRemove() {
  firmware.value = null
}

function onFileExceed() {
  ElMessage.warning('只能选择一个固件文件')
}

// 格式化文件大小
function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`
}

async function doUpload() {
  if (!firmware.value) {
    ElMessage.warning('请选择固件文件')
    return
  }
  // W88: 固件大小上限校验——避免运维误传超大文件（如几个 GB）撑爆浏览器内存
  // 或后端接收体。64MB 是工业网关常见固件上限（含 kernel+rootfs+app）。
  const MAX_FIRMWARE_SIZE = 64 * 1024 * 1024
  if (firmware.value.size > MAX_FIRMWARE_SIZE) {
    ElMessage.warning(`固件文件过大（${formatSize(firmware.value.size)} > 64MB），请检查`)
    return
  }
  if (!(await confirmAction({
    message: `确认升级固件 "${firmware.value.name}"? 升级期间请勿断电。`,
    title: '确认升级',
  }))) return
  try {
    if (unmounted) return
    uploading.value = true
    const buf = await firmware.value.arrayBuffer()
    if (unmounted) return
    await otaUpload(buf)
    if (unmounted) return
    ElMessage.success('固件上传成功，正在升级...')
    // Webhook 变更通知
    webhook.notify('ota_started', '固件升级已开始').catch(() => { /* ignore */ })
  } catch (e) {
    // 写操作失败全局不再弹窗（拦截器只留痕），无论 HTTP 错误还是客户端错误都在此提示
    ElMessage.error(`固件升级失败：${(e as Error)?.message || '未知错误'}`)
  } finally {
    if (!unmounted) {
      uploading.value = false
      firmware.value = null
      uploadRef.value?.clearFiles()
    }
  }
}

async function doRollback() {
  if (!(await confirmAction({ message: '确认回滚到上一个固件版本?', title: '确认回滚' }))) return
  try {
    if (unmounted) return
    await otaRollback()
    if (unmounted) return
    ElMessage.success('回滚已触发')
  } catch {
    // POST 回滚：全局拦截器只留痕不弹窗，需在此给出动作语境提示
    ElMessage.error('固件回滚失败，请确认设备状态后重试')
  }
}

let unmounted = false
let refreshSeq = 0

async function refresh() {
  const seq = ++refreshSeq
  try {
    const s = await getOtaStatus()
    if (unmounted || seq !== refreshSeq) return
    Object.assign(ota, s)
  } catch {
    // 忽略
  }
}

// 常驻轮询：usePolling 统一提供隐藏暂停 / 卸载清理（refresh 内部吞错，不触发退避）
usePolling(refresh, 2000)

onUnmounted(() => {
  unmounted = true
  refreshSeq++
})
</script>

<style scoped src="./Ota.css"></style>
