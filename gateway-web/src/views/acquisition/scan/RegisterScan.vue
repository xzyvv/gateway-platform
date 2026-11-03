<template>
  <div class="scan">
    <SectionHeader title="寄存器扫描" icon="Search" />

    <!-- 扫描参数 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="扫描参数" icon="Setting" />
      </template>
      <el-form :inline="true" label-width="80px" ref="formRef">
        <el-form-item label="总线">
          <el-select v-model="bus" style="width: 220px" :loading="targetsLoading" placeholder="选择总线" aria-label="总线" no-data-text="暂无总线，请先在配置管理中添加">
            <el-option v-for="o in busOptions" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
        </el-form-item>
        <el-form-item label="从站 ID">
          <el-input-number v-model="slaveId" :min="1" :max="247" controls-position="right" />
        </el-form-item>
        <el-form-item label="寄存器类型">
          <el-select v-model="regType" style="width: 160px">
            <el-option label="保持寄存器 (03)" value="holding" />
            <el-option label="输入寄存器 (04)" value="input" />
            <el-option label="线圈 (01)" value="coils" />
            <el-option label="离散量 (02)" value="discrete" />
          </el-select>
        </el-form-item>
        <el-form-item label="起始地址" prop="startAddr">
          <el-input-number v-model="startAddr" :min="0" :max="65535" controls-position="right" />
        </el-form-item>
        <el-form-item label="结束地址" prop="endAddr">
          <el-input-number v-model="endAddr" :min="0" :max="65535" controls-position="right" />
        </el-form-item>
        <el-form-item>
          <el-button type="primary" :loading="scanning" :disabled="!scanSupported" @click="startScanFn">
            <el-icon><VideoPlay /></el-icon> 开始扫描
          </el-button>
        </el-form-item>
      </el-form>
    </el-card>

    <el-alert
      v-if="capsFailed"
      class="section-card"
      type="warning"
      show-icon
      :closable="false"
      title="协议能力元数据加载失败"
      description="暂时无法判断当前总线是否支持寄存器扫描，按钮已禁用；请刷新页面重试。"
    />
    <el-alert
      v-else-if="capsLoaded && selectedBusProtocol && !scanSupported"
      class="section-card"
      type="info"
      show-icon
      :closable="false"
      :title="`当前总线协议（${selectedBusProtocol}）不支持寄存器扫描`"
      description="该协议没有可逐地址探测的寄存器空间，寄存器扫描不适用（不是尚未实现）；如需扫描寄存器，请切换到 Modbus 等支持寄存器扫描的总线。"
    />

    <el-alert
      v-if="scanError"
      class="section-card"
      type="error"
      show-icon
      :closable="true"
      :title="scanError"
      @close="scanError = ''"
    />

    <!-- 扫描结果 -->
    <el-card v-if="currentScan" shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="扫描结果" icon="Loading">
          <template #extra>
            <el-tag size="small" :type="currentScan.status === 'done' ? 'success' : 'warning'" effect="dark" round>
              {{ currentScan.status === 'done' ? '完成' : '扫描中...' }}
            </el-tag>
          </template>
        </SectionHeader>
      </template>
      <el-progress :percentage="currentScan.progress" :status="currentScan.status === 'done' ? 'success' : ''" />
      <div class="scan-stats" aria-live="polite">
        <el-tag type="info">总数 {{ currentScan.total }}</el-tag>
        <el-tag type="success">响应 {{ currentScan.responded }}</el-tag>
        <el-tag type="info">未响应 {{ currentScan.total - currentScan.responded }}</el-tag>
        <el-tag type="primary">响应率 {{ currentScan.total ? Math.round(currentScan.responded / currentScan.total * 100) : 0 }}%</el-tag>
      </div>
    </el-card>

    <!-- 扫描结果图 -->
    <RegisterScanGrid
      v-if="currentScan && currentScan.cells.length"
      :scan="currentScan"
      :grid="gridInfo"
      :suggesting="suggesting"
      @export-csv="exportScan"
      @suggest="viewSuggestions"
    />

    <!-- 点位建议 -->
    <el-dialog v-model="showSuggestions" title="点位建议" width="560px">
      <p class="tip">以下是根据扫描结果自动识别的连续寄存器区域，可作为点位配置参考</p>
      <DataTable :data="suggestions" :columns="suggestionColumns" row-key="id" stripe empty-text="暂无建议" />
    </el-dialog>

    <!-- 历史扫描 -->
    <el-card shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="扫描历史" icon="Clock" />
      </template>
      <DataTable :data="history" :columns="historyColumns" row-key="id" stripe empty-text="暂无历史记录" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
/**
 * 寄存器扫描（薄编排层）
 *
 * 扫描状态机（参数校验/启动/进度轮询/历史增删/CSV 导出/点位建议）在
 * composables/acquisition/useRegisterScan.ts；
 * 地址分布图（本页最大的模板区块）在 scan/parts/RegisterScanGrid.vue。
 *
 * 本文件只保留参数表单、结果概要卡片、建议弹窗与历史表的布局与事件接线。
 */
import { ref } from 'vue'
import type { FormInstance } from 'element-plus'
import { VideoPlay } from '@element-plus/icons-vue'
import { useRegisterScan } from '@/composables/acquisition/useRegisterScan'
import RegisterScanGrid from './parts/RegisterScanGrid.vue'

/** el-form 实例引用，供跨字段校验高亮（校验逻辑在 composable） */
const formRef = ref<FormInstance>()

const {
  busOptions,
  targetsLoading,
  bus,
  capsLoaded,
  capsFailed,
  selectedBusProtocol,
  scanSupported,
  slaveId,
  regType,
  startAddr,
  endAddr,
  scanning,
  scanError,
  currentScan,
  history,
  showSuggestions,
  suggesting,
  suggestions,
  suggestionColumns,
  historyColumns,
  gridInfo,
  startScanFn,
  exportScan,
  viewSuggestions,
} = useRegisterScan(formRef)
</script>

<style scoped>
.scan { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.scan-stats { display: flex; gap: 8px; margin-top: 12px; }
.tip { font-size: 0.8125rem; color: var(--text-secondary); margin-bottom: 12px; }
</style>
