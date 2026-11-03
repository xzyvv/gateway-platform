<template>
  <!-- 单一根节点：调用方常在组件上传 style/margin，多根节点会让 attrs 无法自动继承 -->
  <div v-if="visible" class="import-outcome-wrap">
    <el-alert
      class="import-outcome"
      :type="success ? 'success' : 'error'"
      :title="headline"
      :description="success ? subtitle : undefined"
      :closable="false"
      show-icon
    />
    <div v-if="$slots.extra" class="import-outcome-actions">
      <slot name="extra" />
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 导入结果面板（R1：结果口径统一）。
 *
 * 设备批量导入、配置导入导出、片段导入三条路径此前各自拼「导入成功／导入失败：原因」，
 * 失败原因有时只在 toast 里、有时只显示汇总文案，同一次导入在不同页面看到的反馈不同。
 * 这里统一成「成功标题 + 可选补充说明 / 失败标题 + 后端原文原因」，附加动作走 extra 槽。
 *
 * 注意：只统一展示，不合并入口 —— 整份覆盖、增量合并、批量导入是三种风险等级不同的操作，
 * 入口分散反而能降低误操作。
 */
import { computed } from 'vue'

const props = withDefaults(
  defineProps<{
    /** 是否展示：导入尚未发生时不占位 */
    visible: boolean
    /** 是否成功 */
    success: boolean
    /** 成功标题（如「配置已热重载」）；失败标题统一由原因拼出，避免各页文案漂移 */
    successTitle?: string
    /** 成功时的补充说明（如「新增 2 条、覆盖 1 条」） */
    subtitle?: string
    /** 失败原因：直接透传后端中文原文，不做二次包装 */
    error?: string
  }>(),
  { successTitle: '导入成功', subtitle: '', error: '' },
)

const headline = computed(() => (props.success ? props.successTitle : `导入失败：${props.error || '未知错误'}`))
</script>

<style scoped>
.import-outcome-actions {
  margin-top: 12px;
  display: flex;
  gap: 8px;
}
</style>
