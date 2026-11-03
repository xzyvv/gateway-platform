<template>
  <el-card shadow="never" class="stat-card" :class="variantClass">
    <div class="stat-card-content">
      <div v-if="icon || $slots.icon" class="stat-icon">
        <slot name="icon">
          <el-icon><component :is="icon" /></el-icon>
        </slot>
      </div>
      <div class="stat-body">
        <div class="stat-label">{{ label }}</div>
        <div class="stat-value" :style="valueStyle">
          <slot>{{ value }}<span v-if="unit" class="stat-unit">{{ unit }}</span></slot>
        </div>
        <div v-if="progress !== undefined || $slots.footer" class="stat-bar">
          <slot name="footer">
            <div class="stat-bar-inner" :style="{ width: `${progress}%` }">
              <span class="stat-bar-tip">{{ progressText ?? `${(progress ?? 0).toFixed(1)}%` }}</span>
            </div>
          </slot>
        </div>
      </div>
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 统计卡片
 *
 * 统一封装"标签 + 数值"的统计卡片，消除 6 套重复 CSS（Dashboard/MultiUplinkStatus/
 * NorthStatus/History/DataBuffering/ConnectionTest 等）。
 *
 * Props:
 *   - label:        卡片标签（如 "在线设备"）
 *   - value:        主数值（如 12）；若需富文本用默认 slot
 *   - unit:         单位/趋势文字（如 "台"、"%"）
 *   - icon:         Element Plus 图标组件名（如 "Cpu"、"DataLine"）；可用 #icon slot 自定义
 *   - color:        自定义 value 颜色（如 "var(--el-color-success)"）
 *   - variant:      颜色变体，控制 icon + 进度条颜色（default/blue/orange/green/purple）
 *   - progress:     0-100 进度条；不传则不显示
 *   - progressText: 进度条内提示文字；默认 "{progress}%"
 *
 * Slots:
 *   - default: 自定义 value 内容（覆盖 value prop）
 *   - icon:    自定义图标（覆盖 icon prop）
 *   - footer:  自定义底部内容（覆盖 progress 进度条）
 *
 * 示例：
 *   <StatCard label="在线设备" :value="12" unit="台" icon="Cpu" variant="blue" />
 *   <StatCard label="CPU" :value="`${cpu}%`" :progress="cpu" variant="green" />
 *   <StatCard label="积压" :value="status.queue_length" unit="条" :color="statusColor" />
 */
import { computed } from 'vue'

interface StatCardProps {
  label: string
  value?: string | number
  unit?: string
  icon?: string
  color?: string
  variant?: 'default' | 'blue' | 'orange' | 'green' | 'purple'
  progress?: number
  progressText?: string
}

const props = withDefaults(defineProps<StatCardProps>(), {
  variant: 'default',
})

const variantClass = computed(() => `stat-${props.variant}`)
const valueStyle = computed(() => (props.color ? { color: props.color } : {}))
</script>

<style scoped>
.stat-card { border-radius: var(--radius-card); text-align: center; }
.stat-card-content { display: flex; align-items: center; gap: 12px; }
.stat-icon { font-size: 1.75rem; color: var(--el-color-primary); flex-shrink: 0; }
.stat-body { flex: 1; min-width: 0; }
.stat-label { font-size: 0.8125rem; color: var(--el-text-color-secondary); margin-bottom: 4px; }
.stat-value {
  font-size: 1.75rem; font-weight: 700; line-height: 1.4;
  font-variant-numeric: tabular-nums;
}
.stat-unit { font-size: 0.8125rem; font-weight: 400; margin-left: 4px; color: var(--el-text-color-secondary); }
.stat-bar {
  height: 8px; background: var(--el-fill-color-light);
  border-radius: var(--radius-small); overflow: hidden; margin-top: 8px;
}
.stat-bar-inner {
  height: 100%; background: var(--el-color-primary);
  border-radius: var(--radius-small); transition: width 0.3s; position: relative;
}
.stat-bar-tip {
  position: absolute; right: 4px; top: 50%; transform: translateY(-50%);
  font-size: 0.6875rem; color: var(--text-inverse);
}
/* variant 颜色覆盖 */
.stat-blue .stat-icon { color: var(--el-color-primary); }
.stat-blue .stat-bar-inner { background: var(--el-color-primary); }
.stat-orange .stat-icon { color: var(--el-color-warning); }
.stat-orange .stat-bar-inner { background: var(--el-color-warning); }
.stat-green .stat-icon { color: var(--el-color-success); }
.stat-green .stat-bar-inner { background: var(--el-color-success); }
.stat-purple .stat-icon { color: var(--color-purple); }
.stat-purple .stat-bar-inner { background: var(--color-purple); }
</style>