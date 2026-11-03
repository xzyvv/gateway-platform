<template>
  <div class="console-target">
    <div class="console-target-row">
      <label class="console-target-label" for="console-target-bus">调试目标</label>
      <el-select
        id="console-target-bus"
        v-model="selectedBus"
        size="small"
        class="console-target-select"
        :loading="loading"
        placeholder="选择总线"
        aria-label="调试目标总线"
        no-data-text="暂无总线，请先在配置管理中添加"
      >
        <el-option v-for="o in busOptions" :key="o.value" :label="o.label" :value="o.value" />
      </el-select>
      <el-tag v-if="currentProtocol" size="small" effect="plain" type="info">{{ currentProtocol }}</el-tag>
      <el-tag v-else-if="!loading && !busOptions.length" size="small" effect="plain" type="warning">未配置总线</el-tag>
      <span v-else-if="selectedBus" class="console-target-current">{{ selectedBus }}</span>
    </div>
    <p class="console-target-hint">
      <template v-if="busOptions.length">
        以下所有动作共用此目标；在任一页改选，其余页跟随
      </template>
      <template v-else-if="loading">正在加载总线…</template>
      <template v-else>
        还没有可用总线，请先到
        <router-link to="/system/config?tab=runtime-config">配置管理</router-link>
        添加
      </template>
    </p>
  </div>
</template>

<script setup lang="ts">
/**
 * 调试台目标条：统一调试台的「选目标」入口。
 *
 * 直接复用 useDebugTargets 的模块级共享状态（selectedBus 跨页面跟随、持久化到
 * localStorage），这里不再自己请求 devices/config：选项与加载态都来自 composable，
 * 各动作页内改选总线后本条的展示立即同步。
 */
import { computed } from 'vue'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'

const { buses, busOptions, selectedBus, loading } = useDebugTargets()

/** 当前总线的协议名：以协议 tag 形式固定展示，避免用户只看到端口号 */
const currentProtocol = computed(
  () => buses.value.find((b) => b.port_name === selectedBus.value)?.protocol ?? '',
)
</script>

<style scoped>
/* 紧凑条状容器：只占一行（+ 一行说明），不把内容区首屏往下推 */
.console-target {
  margin-bottom: 12px;
  padding: 8px 12px;
  border: 1px solid var(--border-light);
  border-radius: var(--radius-small);
  background: var(--el-fill-color-lighter);
}
.console-target-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 8px;
}
.console-target-label {
  font-size: 0.8125rem;
  color: var(--text-secondary);
  white-space: nowrap;
}
.console-target-select {
  width: 220px;
}
.console-target-current {
  font-size: 0.8125rem;
  color: var(--text-regular);
}
.console-target-hint {
  margin: 4px 0 0;
  font-size: 0.75rem;
  color: var(--text-secondary);
}
</style>
