<template>
  <el-config-provider :locale="zhCn">
    <!-- 顶层兜底：包裹整棵 router-view，捕获顶层渲染错误（如 Login 页） -->
    <PageErrorBoundary name="app-root" boundary="app">
      <router-view />
    </PageErrorBoundary>

    <!--
      新版本提示：index.html 里的 Service Worker 发现新构建后派发 gateway:update-available。
      不自动刷新（现场可能正在填配置表单），由用户选择刷新时机。
    -->
    <div v-if="updateReady" class="update-banner" role="status">
      <span>检测到新版本，刷新后生效</span>
      <button type="button" class="update-btn" @click="reloadNow">立即刷新</button>
      <button type="button" class="update-close" aria-label="关闭更新提示" @click="updateReady = false">×</button>
    </div>
  </el-config-provider>
</template>

<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import PageErrorBoundary from '../components/PageErrorBoundary.vue'
import zhCn from 'element-plus/es/locale/lang/zh-cn'

const updateReady = ref(false)

function onUpdateAvailable() {
  updateReady.value = true
}

function reloadNow() {
  window.location.reload()
}

onMounted(() => window.addEventListener('gateway:update-available', onUpdateAvailable))
onUnmounted(() => window.removeEventListener('gateway:update-available', onUpdateAvailable))
</script>

<style scoped>
.update-banner {
  position: fixed;
  left: 50%;
  bottom: 20px;
  z-index: 3000;
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 14px;
  transform: translateX(-50%);
  border: 1px solid var(--border-base);
  border-radius: var(--radius-small);
  background: var(--bg-card);
  box-shadow: var(--shadow-dialog);
  color: var(--text-primary);
  font-size: 0.8125rem;
}

.update-btn {
  padding: 4px 12px;
  border: none;
  border-radius: var(--radius-small);
  background: var(--color-primary);
  color: var(--text-inverse);
  font-size: 0.8125rem;
  cursor: pointer;
}

.update-btn:hover {
  background: var(--color-primary-light);
}

.update-close {
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: 1rem;
  line-height: 1;
  cursor: pointer;
}

.update-close:hover {
  color: var(--color-danger);
}
</style>
