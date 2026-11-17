<template>
  <el-card class="log-panel" shadow="never">
    <template #header>
      <SectionHeader :title="title" :icon="icon">
        <el-button v-if="copyable" size="small" text :disabled="empty" @click="emit('copy-all')">
          <el-icon><CopyDocument /></el-icon> {{ copyLabel }}
        </el-button>
        <el-button v-if="exportable" size="small" text :disabled="empty" @click="emit('export')">
          <el-icon><Download /></el-icon> {{ exportLabel }}
        </el-button>
        <el-button v-if="clearable" size="small" text :disabled="empty" @click="emit('clear')">
          <el-icon><Delete /></el-icon> {{ clearLabel }}
        </el-button>
      </SectionHeader>
    </template>

    <div class="log-panel-scroll" :class="{ 'is-empty': empty }" :style="scrollStyle">
      <EmptyState v-if="empty" :description="emptyText" :image-size="80" />
      <slot v-else />
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 日志 / 结果面板（公共件）
 *
 * 这类面板在协议调试台里出现了三份（执行日志、操作记录、测试日志），
 * 卡片壳、标题栏、空态、滚动区完全同构，但「复制 / 导出 / 清空」三件套
 * 各缺一块，现场直接受影响：
 * - 单步调试的「执行日志」能复制全部，**不能导出**——要比对 20 步的耗时分布
 *   只能一条条复制；
 * - 点位读写的「操作记录」能导出，**不能复制全部**——要贴给群里看的时候
 *   只能导出文件再传；
 * - 读取性能的「测试日志」只有清空，既不能复制也不能导出——P50/P99 那一轮
 *   跑完就只剩一张截图。
 *
 * 现在三件套由同一处渲染：传 `copyable / exportable / clearable` 决定显示哪几个，
 * 具体动作仍由页面自己实现（数据形状各页不同，抽不出通用的序列化）。
 *
 * 有意不含：系统设置的「日志归档」。那是磁盘上真实日志文件的只读尾部浏览，
 * 没有「清空」语义、导出就是下载文件本身（列表里已有「下载」按钮），
 * 硬套本组件只会多一层卡片壳而没有减少任何重复。
 *
 * Props:
 * - title / icon：标题与图标（沿用 SectionHeader 的视觉规范）
 * - empty / emptyText：内容是否为空、空态文案（空态下保持最小高度，避免卡片塌成一条）
 * - copyable / exportable / clearable：是否显示对应按钮（默认全关，按需打开）
 * - maxHeight：滚动区最大高度（px），超过即内部滚动，页面不被日志撑长
 *
 * Emits: copy-all / export / clear（实际动作在父组件）
 */
import { computed } from 'vue'
import { CopyDocument, Delete, Download } from '@element-plus/icons-vue'
import EmptyState from './EmptyState.vue'
import SectionHeader from './SectionHeader.vue'

const props = withDefaults(defineProps<{
  title: string
  icon?: string
  /** 内容为空：显示空态并禁用复制/导出（没内容可导） */
  empty: boolean
  emptyText: string
  copyable?: boolean
  exportable?: boolean
  clearable?: boolean
  copyLabel?: string
  exportLabel?: string
  clearLabel?: string
  /** 滚动区最大高度（px） */
  maxHeight?: number
  /** 空态下的最小高度（px）：避免卡片塌成一条把页面布局抖一下 */
  emptyMinHeight?: number
}>(), {
  icon: 'Document',
  copyable: false,
  exportable: false,
  clearable: false,
  copyLabel: '复制全部',
  exportLabel: '导出 CSV',
  clearLabel: '清空',
  maxHeight: 480,
  emptyMinHeight: 200,
})

const emit = defineEmits<{
  (e: 'copy-all'): void
  (e: 'export'): void
  (e: 'clear'): void
}>()

const scrollStyle = computed(() => ({
  maxHeight: `${props.maxHeight}px`,
  minHeight: props.empty ? `${props.emptyMinHeight}px` : undefined,
}))
</script>

<style scoped>
.log-panel {
  display: flex;
  flex-direction: column;
}
.log-panel-scroll {
  overflow-y: auto;
}
/* 空态垂直居中：没有内容时也要让人看清「这里本来该有东西」 */
.log-panel-scroll.is-empty {
  display: flex;
  align-items: center;
  justify-content: center;
}
</style>
