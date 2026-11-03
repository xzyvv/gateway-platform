<template>
  <div>
    <el-form label-width="104px" style="max-width: 560px">
      <el-form-item label="总线">
        <el-select
          v-model="busPick"
          :placeholder="buses.length ? '选择已有总线' : '暂无总线，请先新建'"
          :disabled="!buses.length"
          style="width: 100%"
        >
          <el-option
            v-for="b in buses"
            :key="b.port_name"
            :label="`${b.port_name} · ${b.protocol} · ${b.baud_rate}`"
            :value="b.port_name"
          />
        </el-select>
        <div class="field-hint">还没有可用总线，用下面三项新建一条（其余参数按工业默认值补齐）。</div>
      </el-form-item>
    </el-form>

    <div v-if="showNewBus" class="new-bus">
      <p class="sub-title">新建总线</p>
      <el-form label-width="104px" style="max-width: 560px">
        <el-form-item label="串口端口">
          <PortSelect v-model="newBus.port_name" />
        </el-form-item>
        <el-form-item label="波特率">
          <BaudrateSelect v-model="newBus.baud_rate" />
        </el-form-item>
        <el-form-item label="协议">
          <ProtocolSelector
            :model-value="newBus.protocol"
            @update:model-value="(v: string) => (newBus.protocol = v)"
          />
        </el-form-item>
      </el-form>
      <div class="step-actions">
        <el-button v-if="buses.length" @click="closeNewBus">取消</el-button>
        <el-button type="primary" :loading="addingBus" @click="emit('add-bus')">新增并选中</el-button>
      </div>
    </div>
    <div v-else class="step-actions">
      <el-button plain @click="openNewBus">新增一条总线</el-button>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 部署向导 · 第 1 步：配置通信总线。
 *
 * 已有总线直接下拉选择；没有可用总线时用「串口端口 / 波特率 / 协议」三项就地建一条。
 *
 * Props:
 *   - buses:      网关现有总线列表（只读渲染下拉项）
 *   - busPick:    当前选中的总线端口名（v-model，选中结果由主组件用于后续步骤）
 *   - showNewBus: 是否展开「新建总线」表单（v-model）
 *   - newBus:     新建总线的表单值（v-model，三个字段由主组件最终组装成 BusConfig）
 *   - addingBus:  新建总线请求进行中（按钮 loading）
 *
 * Emits:
 *   - add-bus:    点击「新增并选中」，由主组件调用真实接口写入配置
 *
 * 典型应用场景：部署向导的第 1 步，只负责渲染与局部交互，不直接发请求。
 */
import PortSelect from '@/components/industrial/PortSelect.vue'
import BaudrateSelect from '@/components/industrial/BaudrateSelect.vue'
import ProtocolSelector from '@/components/industrial/ProtocolSelector.vue'
import type { BusConfig } from '@/api/types'
import type { NewBusForm } from './types'

defineProps<{
  buses: BusConfig[]
  addingBus: boolean
}>()

/** 当前选中的总线端口名 */
const busPick = defineModel<string>('busPick', { required: true })
/** 是否展开新建总线表单 */
const showNewBus = defineModel<boolean>('showNewBus', { required: true })
/** 新建总线的表单值 */
const newBus = defineModel<NewBusForm>('newBus', { required: true })

const emit = defineEmits<{
  'add-bus': []
}>()

/** 展开新建总线表单 */
function openNewBus() {
  showNewBus.value = true
}

/** 收起新建总线表单 */
function closeNewBus() {
  showNewBus.value = false
}
</script>

<style scoped>
.field-hint { font-size: 0.75rem; color: var(--text-secondary); line-height: 1.5; margin-top: 2px; }
.new-bus { margin-top: 8px; padding-top: 12px; border-top: 1px dashed var(--border-light); }
.sub-title { margin: 0 0 8px; font-size: 0.875rem; font-weight: 600; }
.step-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; padding-top: 16px; border-top: 1px solid var(--el-border-color-lighter); }
</style>
