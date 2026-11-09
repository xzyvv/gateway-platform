<template>
  <el-dialog v-model="visible" title="新建设备模板" width="460px">
    <el-form label-width="104px">
      <el-form-item label="模板名称" required>
        <el-input v-model="form.name" placeholder="如：三相电表" maxlength="60" />
      </el-form-item>
      <el-form-item label="协议">
        <ProtocolSelector v-model="form.protocol" aria-label="协议" />
      </el-form-item>
      <el-form-item label="采集周期">
        <DurationSelect v-model="form.interval" :min="1" :max="3600" :units="['s', 'min']" />
      </el-form-item>
    </el-form>
    <template #footer>
      <el-button @click="close">取消</el-button>
      <el-button type="primary" :loading="saving" @click="emit('save')">创建并选用</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
/**
 * 部署向导 · 就地新建设备模板对话框。
 *
 * 第 2 步里模板库没有合适的模板时可弹此框直接建一个，建好自动选中。
 *
 * Props:
 *   - saving: 保存请求进行中（按钮 loading）
 *
 * Models:
 *   - modelValue: 对话框可见性（v-model）
 *   - form:       模板表单值（v-model：name / protocol / interval）
 *
 * Emits:
 *   - save: 点击「创建并选用」，由主组件调用真实模板接口
 *
 * 典型应用场景：部署向导第 2 步的模板兜底入口，不直接发请求。
 */
import type { NewTemplateForm } from './types'
import ProtocolSelector from '@/components/industrial/ProtocolSelector.vue'
import DurationSelect from '@/components/industrial/DurationSelect.vue'

defineProps<{
  saving: boolean
}>()

/** 对话框可见性 */
const visible = defineModel<boolean>({ required: true })
/** 模板表单值 */
const form = defineModel<NewTemplateForm>('form', { required: true })

const emit = defineEmits<{
  save: []
}>()

/** 关闭对话框 */
function close() {
  visible.value = false
}
</script>
