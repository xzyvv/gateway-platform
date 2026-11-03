<template>
  <el-tooltip :content="tip" placement="top" effect="light">
    <el-tag type="info" size="small" effect="plain" class="local-only-badge">{{ label }}</el-tag>
  </el-tooltip>
</template>

<script setup lang="ts">
/**
 * 本地演示徽标
 *
 * 标注「数据仅保存在当前浏览器 / 未接入网关后端」的功能，
 * 避免用户（尤其是不懂编程的现场工程师）把本地占位数据误当成网关真实状态。
 * 双轨数据源（src/mock 与 src/api）的背景说明见 [src/mock/mocks.spec.ts]。
 *
 * 用法：放进 SectionHeader 的 extra 插槽即可显示在标题右侧。
 */
withDefaults(
  defineProps<{
    /** 徽标文案 */
    label?: string
    /** 悬停提示，说明该功能为何是本地演示 */
    tip?: string
  }>(),
  {
    label: '本地演示',
    tip: '本页数据仅保存在当前浏览器，未接入网关后端，不代表网关真实状态',
  },
)
</script>

<style scoped>
.local-only-badge {
  margin-left: 4px;
  font-weight: 400;
}
</style>