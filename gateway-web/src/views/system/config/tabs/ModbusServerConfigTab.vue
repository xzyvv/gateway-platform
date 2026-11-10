<template>
  <div class="section-toggle">
    <el-switch
      :model-value="mb !== null"
      @update:model-value="(val: boolean) => (mb = val ? { enabled: true, endpoint: 'tcp://0.0.0.0:502', base_register: 0, mode: 'auto' } : null)"
      active-text="启用 Modbus Server"
      inactive-text="禁用"
    />
  </div>
  <el-form v-if="mb" :model="mb" label-width="160px" class="modbus-server-form">
    <el-form-item label="启用对外服务">
      <el-switch v-model="form.enabled" />
    </el-form-item>
    <el-form-item label="监听端点">
      <el-input v-model="form.endpoint" placeholder="tcp://0.0.0.0:502">
        <template #prefix><el-icon><Cpu /></el-icon></template>
      </el-input>
      <div class="form-hint">格式 tcp://host:port，默认 502；与 Modbus 透传的监听端口需错开。</div>
    </el-form-item>
    <el-form-item label="起始寄存器地址">
      <el-input-number v-model="form.base_register" :min="0" :max="65535" />
      <div class="form-hint">
        auto 模式下第一个点位占用的寄存器地址，后续点位按顺序递增。
        按协议地址填写（从 0 起），不是 40001 这类显示编号：填 0 时主站读 40001，填 100 时主站读 40101（保持寄存器）。
      </div>
    </el-form-item>
    <el-form-item label="映射模式">
      <el-select v-model="form.mode" style="width: 220px">
        <el-option label="自动分配（auto）" value="auto" />
        <el-option label="按点位地址（manual）" value="manual" />
      </el-select>
    </el-form-item>
  </el-form>
</template>

<script setup lang="ts">
/** Modbus Server（对外服务）配置页签（F-14）。
 *  后端 north/server/modbus_server.rs 已接入 bootstrap；寄存器映射由 ModbusServerAdapter 按
 *  base_register + mode 生成，本页只负责配置节本身。 */
import { computed } from 'vue'
import { Cpu } from '@element-plus/icons-vue'
import type { ModbusServerConfig } from '@/api/types'

const mb = defineModel<ModbusServerConfig | null>({ required: true })
/** 表单渲染只在 mb 非空时发生；此视图仅用于让模板拿到非空类型 */
const form = computed(() => mb.value as ModbusServerConfig)
</script>

<style scoped>
.form-hint {
  margin-top: 4px;
  font-size: 0.72rem;
  color: var(--text-secondary);
  line-height: 1.4;
}
</style>
