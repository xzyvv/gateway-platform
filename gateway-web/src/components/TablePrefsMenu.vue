<template>
  <!--
    表格显示设置：列显隐（按表记忆）+ 行密度（全局共享）。
    两项偏好都是"人体工学"设置，放在页面顶部工具区最容易被发现，
    且都不影响数据正确性，误改也能立刻改回来。
  -->
  <el-popover trigger="click" :width="240" placement="bottom-end">
    <template #reference>
      <!-- 不加 tooltip 包裹：popover 的 reference 需要单一元素，嵌套会让浮层定位漂移 -->
      <slot name="reference">
        <el-button size="small" aria-label="表格显示设置" title="表格显示设置">
          <el-icon><Setting /></el-icon>
        </el-button>
      </slot>
    </template>

    <div class="tp">
      <div class="tp-title">行密度</div>
      <el-radio-group
        :model-value="density"
        size="small"
        @update:model-value="(v: string | number | boolean | undefined) => setDensity(v === 'compact' ? 'compact' : 'default')"
      >
        <el-radio-button value="default">标准</el-radio-button>
        <el-radio-button value="compact">紧凑</el-radio-button>
      </el-radio-group>
      <div class="tp-hint">紧凑一屏能多看约三分之一行，全站表格同步生效</div>

      <template v-if="columns.length">
        <el-divider style="margin: 10px 0" />
        <div class="tp-title">
          显示列
          <el-button size="small" link class="tp-reset" @click="reset">全部显示</el-button>
        </div>
        <!-- 至少保留一列可见：全隐藏会让表格只剩表头，用户以为页面坏了 -->
        <el-checkbox
          v-for="c in columns"
          :key="c.key"
          :model-value="!isHidden(c.key)"
          :disabled="isHidden(c.key) && visibleCount <= 1"
          @change="toggle(c.key)"
        >{{ c.title }}</el-checkbox>
      </template>
    </div>
  </el-popover>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { Setting } from '@element-plus/icons-vue'
import { useColumnPrefs, useTableDensity } from '@/composables/core/useTablePrefs'

/** 表的唯一标识：列显隐按它分表记忆，多个页面复用同一组件时用不同 id 即可隔离 */
const props = defineProps<{
  tableId: string
  /** 可隐藏的列；不传则只显示密度切换（窄表无需列显隐） */
  columns?: { key: string; title: string }[]
}>()

const { density, setDensity } = useTableDensity()
const { hidden, isHidden, toggle, reset } = useColumnPrefs(props.tableId)

const columns = computed(() => props.columns ?? [])
const visibleCount = computed(() => columns.value.length - hidden.value.length)
</script>

<style scoped>
.tp-title {
  font-size: 0.75rem;
  color: var(--text-secondary);
  margin-bottom: 6px;
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.tp-reset { font-size: 0.75rem; }
.tp-hint {
  font-size: 0.6875rem;
  color: var(--text-secondary);
  margin-top: 4px;
  line-height: 1.4;
}
</style>