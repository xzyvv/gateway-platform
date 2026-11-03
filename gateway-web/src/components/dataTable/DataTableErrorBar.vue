<template>
  <!--
    错误态下沉到表格：加载失败时页面若只弹 toast，表格只剩空态，
    用户会把「接口失败」误判成「网关没有这条数据」。这里由页面把错误文案传进来，
    表格统一渲染错误条 + 重试按钮。
  -->
  <el-alert
    class="table-error"
    type="error"
    :title="message"
    :closable="false"
    show-icon
  >
    <template #default>
      <el-button link type="primary" @click="emit('retry')">重试</el-button>
    </template>
  </el-alert>
</template>

<script setup lang="ts">
/**
 * DataTable 下沉的加载失败错误条
 *
 * 页面加载失败若只弹 toast，表格会退化成空态「暂无数据」，
 * 用户会误判成「网关没有这条数据」而不是「接口失败」。
 * 这里由页面把错误文案传进来，表格统一渲染错误条 + 重试按钮。
 *
 * Props:
 *   - message: 错误文案（由页面把接口错误信息传进来）
 *
 * Emits:
 *   - retry: 点击「重试」，由页面重新拉数据
 *
 * 典型应用场景：DataTable 内部在 error 非空且非加载中时渲染，页面不直接引用本组件。
 */
interface Props {
  /** 错误文案（由页面把接口错误信息传进来） */
  message: string
}

defineProps<Props>()

const emit = defineEmits<{
  /** 点击「重试」，由页面重新拉数据 */
  (e: 'retry'): void
}>()
</script>

<style scoped>
/* 错误条与表格之间留白，避免紧贴表头 */
.table-error {
  margin-bottom: 12px;
}
</style>
