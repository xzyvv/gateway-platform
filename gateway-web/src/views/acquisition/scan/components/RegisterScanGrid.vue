<template>
  <el-card shadow="never" class="section-card" style="margin-top: 16px">
    <template #header>
      <SectionHeader :title="`地址分布图（${scan.startAddr} - ${scan.endAddr}）`" icon="Grid">
        <el-button size="small" @click="emit('export-csv')">
          <el-icon><Download /></el-icon> 导出 CSV
        </el-button>
        <el-button size="small" type="primary" :loading="suggesting" :disabled="suggesting" @click="emit('suggest')">
          <el-icon><MagicStick /></el-icon> 生成点位建议
        </el-button>
      </SectionHeader>
    </template>
    <div v-if="grid.stride > 1" class="ds-hint">
      地址数较多（共 {{ scan.cells.length }} 个），已按每 {{ grid.stride }} 个地址聚合显示，格内为「有响应数/总数」
    </div>
    <div class="addr-grid" :class="{ 'ds-mode': grid.stride > 1 }" role="group" aria-label="地址分布图">
      <div
        v-for="cell in grid.cells"
        :key="cell.address"
        class="addr-cell"
        :class="{ responded: cell.responded, 'no-response': !cell.responded }"
        role="img"
        :aria-label="cellTitle(cell, grid.stride)"
        :title="cellTitle(cell, grid.stride)"
      >
        <div class="addr">{{ cellLabel(cell, grid.stride) }}</div>
        <div v-if="cell.responded" class="val">{{ cellSub(cell, grid.stride) }}</div>
      </div>
    </div>
    <div class="legend">
      <span><i class="dot responded-dot"></i>有响应</span>
      <span><i class="dot no-response-dot"></i>无响应</span>
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 寄存器扫描「地址分布图」卡片（从 RegisterScan.vue 拆出）
 *
 * 职责：渲染地址响应分布（有响应/无响应配色与图例）、大区间降采样提示，
 * 以及导出 CSV / 生成点位建议两个入口。降采样与格内文案由
 * useRegisterScan 的 gridInfo + cellLabel/cellSub/cellTitle 统一计算。
 *
 * 为什么单独拆出：地址格最多可达 1000 个 DOM 节点，是本页模板里最大的一块；
 * 拆开后可独立调整配色/降采样展示，不影响扫描参数与历史表。
 *
 * Props:
 * - scan：当前扫描结果（标题里的地址区间与地址数）
 * - grid：降采样后的显示格
 * - suggesting：点位建议生成中（按钮 loading 并禁用）
 *
 * Emits:
 * - export-csv：导出 CSV
 * - suggest：生成点位建议
 */
import { Download, MagicStick } from '@element-plus/icons-vue'
import type { ScanResult } from '@/api/protocolTools'
import { cellLabel, cellSub, cellTitle, type GridInfo } from '@/composables/acquisition/useRegisterScan'

defineProps<{
  /** 当前扫描结果 */
  scan: ScanResult
  /** 降采样后的显示格 */
  grid: GridInfo
  /** 点位建议生成中 */
  suggesting: boolean
}>()

const emit = defineEmits<{
  /** 导出 CSV */
  (e: 'export-csv'): void
  /** 生成点位建议 */
  (e: 'suggest'): void
}>()
</script>

<style scoped>
.section-card { border-radius: var(--radius-card); }
.addr-grid {
  display: grid;
  grid-template-columns: repeat(10, 1fr);
  gap: 3px;
  margin-top: 12px;
}
.addr-cell {
  aspect-ratio: 1;
  border-radius: var(--radius-small);
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  font-size: 0.625rem;
  border: 1px solid var(--border-light);
}
.addr-cell.responded { background: var(--color-success-bg); border-color: var(--color-success); color: var(--text-success); }
.addr-cell.no-response { background: var(--bg-fill); color: var(--text-placeholder); }
.addr-cell.ds-mode .addr { font-size: 0.5rem; }
.addr { font-weight: 600; }
.val { font-size: 0.5625rem; }
.ds-hint { margin-top: 12px; font-size: 0.75rem; color: var(--text-secondary); }
.legend { display: flex; gap: 20px; margin-top: 12px; font-size: 0.75rem; color: var(--text-secondary); }
.dot { display: inline-block; width: 12px; height: 12px; border-radius: var(--radius-bar); margin-right: 4px; vertical-align: middle; }
.responded-dot { background: var(--color-success-bg); border: 1px solid var(--color-success); }
.no-response-dot { background: var(--bg-fill); border: 1px solid var(--border-light); }
</style>
