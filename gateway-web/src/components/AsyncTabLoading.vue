<template>
  <div class="async-tab-loading" role="status" aria-label="加载中">
    <el-icon class="atb-spin"><Loading /></el-icon>
    <span>加载中…</span>
  </div>
</template>

<script setup lang="ts">
/**
 * 异步页面 / Hub tab 的统一加载占位。
 * 供 defineAsyncComponent({ loadingComponent }) 使用：懒加载组件下载期间
 * 显示轻量 spinner，替代白屏瞬间。delay 由使用方传入（建议 200ms，
 * 快速网络下秒开不闪 loading）。
 */
</script>

<style scoped>
.async-tab-loading {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 56px 0;
  color: var(--text-secondary);
  font-size: 0.8125rem;
}
.atb-spin { animation: atb-rotate 1s linear infinite; }
@keyframes atb-rotate { to { transform: rotate(360deg); } }
</style>
