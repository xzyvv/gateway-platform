<template>
  <div class="cert-view">
    <div class="top-bar">
      <el-tag v-if="loaded" size="small" :type="cert ? 'success' : 'danger'" effect="plain" round>
        {{ cert ? '证书可用' : '证书不可用' }}
      </el-tag>
      <el-button type="primary" @click="load" :loading="loading">
        <el-icon><Refresh /></el-icon> 刷新
      </el-button>
    </div>

    <div>
      <!-- 首屏骨架：按证书卡片的「label 条 + 值条」结构占位，数据到达时布局不跳 -->
      <el-card v-if="loading && !loaded" shadow="never" class="cert-card">
        <el-skeleton animated>
          <template #template>
            <div class="cert-field" v-for="i in 6" :key="`sk-field-${i}`">
              <el-skeleton-item variant="text" class="sk-label" />
              <el-skeleton-item
                variant="text"
                class="sk-value"
                :class="{ 'sk-fingerprint': i === 6 }"
              />
            </div>
          </template>
        </el-skeleton>
      </el-card>

      <template v-else>
        <empty-state v-if="loaded && !cert" description="证书不可用" />

        <template v-else-if="cert">
          <el-card shadow="never" class="cert-card">
            <div class="cert-field">
              <span class="field-label">Subject</span>
              <span class="field-value">{{ cert.subject }}</span>
            </div>
            <div class="cert-field">
              <span class="field-label">Issuer</span>
              <span class="field-value">{{ cert.issuer }}</span>
            </div>
            <div class="cert-field">
              <span class="field-label">有效期起始</span>
              <span class="field-value">{{ formatDateTime(cert.valid_from * 1000) }}</span>
            </div>
            <div class="cert-field">
              <span class="field-label">有效期截止</span>
              <span class="field-value">{{ formatDateTime(cert.valid_to * 1000) }}</span>
            </div>
            <div class="cert-field">
              <span class="field-label">剩余天数</span>
              <el-tag
                :type="cert.days_remaining < 30 ? 'danger' : 'success'"
                effect="dark"
                round
              >
                {{ cert.days_remaining }} 天
              </el-tag>
            </div>
            <div class="cert-field fingerprint-field">
              <span class="field-label">指纹 (SHA256)</span>
              <code class="fingerprint">{{ cert.fingerprint }}</code>
            </div>
          </el-card>

          <el-alert
            v-if="cert.days_remaining < 30"
            :title="`证书即将过期！余${cert.days_remaining} 天`"
            type="error"
            :closable="false"
            show-icon
            class="warn-alert"
          />

          <div v-if="cert.cert_path" class="cert-path">
            <el-icon><Folder /></el-icon>
            <span class="path-text">{{ cert.cert_path }}</span>
          </div>
        </template>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import {Refresh, Folder} from '@element-plus/icons-vue'
import { listCertificates, type Certificate } from '@/api/certificates'
import { formatDateTime } from '@/utils/datetime'
import { logError } from '@/utils/logger'

/**
 * 第 1 期冗余收口：`GET /api/cert/status` 已删除（与 `/api/certificates` 同源），
 * 改为读取证书列表首项；列表为空即「证书不可用」。
 */
const cert = ref<Certificate | null>(null)
/** 是否已完成首次加载（区分「加载中骨架」与「加载完成但无证书」） */
const loaded = ref(false)
const loading = ref(false)

async function load() {
  loading.value = true
  try {
    const list = await listCertificates()
    cert.value = list[0] ?? null
  } catch (e) {
    showToast(e, '加载证书状态失败')
    logError('cert-view', e)
  } finally {
    loaded.value = true
    loading.value = false
  }
}

onMounted(load)
</script>

<style scoped src="./CertView.css"></style>
