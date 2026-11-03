<template>
  <el-result :icon="icon" :title="title">
    <template v-if="subTitle" #sub-title>
      {{ subTitle }}
    </template>
    <template v-if="$slots.extra" #extra>
      <slot name="extra" />
    </template>
  </el-result>
</template>

<script setup lang="ts">
/**
 * 批量导入 · 共用「确认导入 / 导入结果」结论块。
 *
 * 设备分支与点位分支的结论块结构一致：一个大图标 + 标题 + 一句统计说明 +
 * 操作按钮。差异只在文案与按钮，因此这里只渲染骨架，具体操作由调用方
 * 通过 extra 插槽传入。
 *
 * Props:
 *   - icon:     结果图标（success / info / warning / error）
 *   - title:    结果标题
 *   - subTitle: 补充说明（为空则不渲染副标题）
 *
 * Slots:
 *   - extra:    结论块下方的操作按钮（不传则不渲染该区域）
 *
 * 典型应用场景：批量导入页的设备分支「确认导入」步与点位分支「确认导入」步。
 */
defineProps<{
  icon: 'success' | 'info' | 'warning' | 'error'
  title: string
  subTitle?: string
}>()
</script>
