<template>
  <!-- 页面名（地址换算）由 HubShell 统一渲染；页签也同名，这里不再重复一个区块标题 -->
  <div class="ac">
    <el-card shadow="never" class="section-card">
      <el-form inline>
        <el-form-item label="Modbus 地址">
          <ModbusAddressInput v-model="manualAddr" />
        </el-form-item>
        <el-form-item label="功能码"><el-select v-model="funcCode" style="width:180px"><el-option v-for="f in fcs" :key="f.code" :label="`${f.code} - ${f.name}`" :value="f.code" /></el-select></el-form-item>
        <el-form-item label="从站ID"><el-input-number v-model="slaveId" :min="1" :max="247" /></el-form-item>
        <el-form-item label="数量"><el-input-number v-model="count" :min="1" :max="125" /></el-form-item>
        <el-form-item><el-button type="primary" :loading="calcLoading" @click="calc">计算</el-button></el-form-item>
      </el-form>
      <el-divider />
      <div v-if="result" class="result">
        <div class="row"><span>十进制地址:</span><code>{{ result.manualAddr }}</code></div>
        <div class="row"><span>系统地址(0-based):</span><code class="hl">{{ result.systemAddr }}</code></div>
        <div class="row"><span>功能码:</span><code>{{ result.funcCode }} ({{ result.funcName }})</code></div>
        <div class="row"><span>请求报文:</span><code class="hex">{{ result.requestHex }}</code></div>
        <el-alert :title="result.note" type="info" :closable="false" show-icon />
      </div>
    </el-card>
  </div>
</template>
<script setup lang="ts">
import { ref } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { calculate, listFuncCodes, type CalcResult } from '@/utils/addrCalculator'
import ModbusAddressInput from '@/components/industrial/ModbusAddressInput.vue'
const fcs = listFuncCodes()
const manualAddr = ref('40001'); const funcCode = ref('03'); const slaveId = ref(1); const count = ref(2)
const result = ref<CalcResult | null>(null)
const calcLoading = ref(false)
async function calc() { calcLoading.value = true; try { result.value = await calculate(manualAddr.value, funcCode.value, count.value, slaveId.value) } catch (e) { showToast(e, '计算失败，请检查参数') } finally { calcLoading.value = false } }
</script>
<style scoped>
.ac{padding:16px}.section-card{margin-bottom:16px}.result{line-height:2.2}.row{display:flex;gap:12px;align-items:center}.row span{min-width:140px;color:var(--el-text-color-secondary)}code{color:var(--el-color-primary)}code.hl{font-size: 1.125rem;font-weight:600}code.hex{background:var(--el-fill-color-light);padding:2px 8px;border-radius: var(--radius-small)}
</style>