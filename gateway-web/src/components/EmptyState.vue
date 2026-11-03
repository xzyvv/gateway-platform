<template>
  <el-empty :description="description" :image-size="size">
    <!-- 统一 CTA：列表空态不只说"没有数据"，还要给出下一步能做什么 -->
    <el-button v-if="actionText" :type="actionType" size="small" @click="emit('action')">
      {{ actionText }}
    </el-button>
    <slot />
  </el-empty>
</template>

<script setup lang="ts">
import { computed } from 'vue'

// `imageSize` 同时接受 number 与数字字符串：`el-empty` 内部用 addUnit 补单位，
// 字符串 "80" 最终也会变成 "80px"，功能上等价——但 element-plus 的 prop 校验
// 会为每个实例打一条 "Invalid prop: type check failed" 警告（实测 30+ 处调用点），
// 在控制台刷屏并掩盖真正的告警。此处统一归一为 number，把警告消在源头，
// 避免逐个改 30+ 处调用点（那是纯写法偏好，不是行为修复）。
const props = withDefaults(defineProps<{
  description?: string
  imageSize?: number | `${number}`
  /** 空态主行动文案；不传则不渲染按钮（如纯信息空态） */
  actionText?: string
  actionType?: 'primary' | 'default'
}>(), { actionType: 'primary', imageSize: 80 })

const size = computed(() => {
  const v = props.imageSize
  if (v === undefined) return undefined
  const n = typeof v === 'number' ? v : Number(v)
  return Number.isFinite(n) ? n : undefined
})

const emit = defineEmits<{ action: [] }>()
</script>
