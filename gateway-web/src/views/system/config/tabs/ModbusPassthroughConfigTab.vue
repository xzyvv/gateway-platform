<template>
  <div class="section-toggle">
    <el-switch
      :model-value="pt !== null"
      @update:model-value="(val: boolean) => (pt = val ? { enabled: true, listen_addr: '0.0.0.0:5020', target_bus: busOptions[0] ?? '', timeout_ms: 2000 } : null)"
      active-text="启用 Modbus 透传"
      inactive-text="禁用"
    />
  </div>
  <el-form v-if="pt" :model="pt" label-width="160px" class="modbus-passthrough-form">
    <el-form-item label="启用透传">
      <el-switch v-model="pt.enabled" />
    </el-form-item>
    <el-form-item label="TCP 监听地址">
      <el-input v-model="pt.listen_addr" placeholder="0.0.0.0:5020">
        <template #prefix><el-icon><Link /></el-icon></template>
      </el-input>
      <div class="form-hint">默认 5020，故意避开 Modbus Server 的 502，避免两个服务抢同一端口。</div>
    </el-form-item>
    <el-form-item label="目标串口总线">
      <el-select v-model="pt.target_bus" filterable allow-create default-first-option style="width: 260px" placeholder="选择总线端口名">
        <el-option v-for="name in busOptions" :key="name" :label="name" :value="name" />
      </el-select>
      <div class="form-hint">对应「总线」页签中已配置的 port_name；透传请求会转到该 RTU 总线。</div>
    </el-form-item>
    <el-form-item label="请求超时">
      <el-input-number v-model="pt.timeout_ms" :min="100" :max="60000" :step="100" />
      <span class="unit-suffix">毫秒</span>
    </el-form-item>
  </el-form>
</template>

<script setup lang="ts">
/** Modbus TCP↔RTU 透传配置页签（F-14）。
 *  后端 north/server/mqtt_packet_passthrough.rs 的同层能力已接入 bootstrap；
 *  目标串口总线从当前配置的 buses 里选，避免手填错 port_name。 */
import { computed } from 'vue'
import { Link } from '@element-plus/icons-vue'
import type { BusConfig, ModbusPassthroughConfig } from '@/api/types'

const props = defineProps<{ buses: BusConfig[] }>()
const pt = defineModel<ModbusPassthroughConfig | null>({ required: true })

const busOptions = computed(() => props.buses.map((b) => b.port_name).filter(Boolean))
</script>

<style scoped>
.form-hint {
  margin-top: 4px;
  font-size: 0.72rem;
  color: var(--text-secondary);
  line-height: 1.4;
}
.unit-suffix {
  margin-left: 8px;
  font-size: 0.78rem;
  color: var(--text-secondary);
}
</style>
