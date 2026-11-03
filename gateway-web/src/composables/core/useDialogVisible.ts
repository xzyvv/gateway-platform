import { computed } from 'vue'

/**
 * Dialog v-model 可见性封装
 *
 * 统一 el-dialog 的 v-model 双向绑定样板代码。
 * 每个 Dialog 组件原本都要写：
 *   const visible = computed({
 *     get: () => props.modelValue,
 *     set: (v) => emit('update:modelValue', v),
 *   })
 * 使用本 composable 后简化为：
 *   const visible = useDialogVisible(props, emit)
 *
 * @param props 组件 props，需包含 modelValue: boolean
 * @param emit 组件 emit，需支持 'update:modelValue' 事件
 * @returns 可读可写的 visible computed，直接绑定到 el-dialog 的 v-model
 */
export function useDialogVisible(
  props: { modelValue: boolean },
  emit: (event: 'update:modelValue', value: boolean) => void,
) {
  return computed({
    get: () => props.modelValue,
    set: (v: boolean) => emit('update:modelValue', v),
  })
}
