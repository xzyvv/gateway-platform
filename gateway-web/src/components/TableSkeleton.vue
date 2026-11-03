<template>
  <!--
    表格骨架屏：首屏加载时用与表格同构的占位行替代居中转圈。
    用户能提前看到「这里将出现一张几列的表格」，数据到达时布局不跳动。
  -->
  <div
    class="table-skeleton"
    role="status"
    aria-live="polite"
    aria-busy="true"
    :aria-label="label"
  >
    <el-skeleton animated>
      <template #template>
        <div class="sk-row sk-head">
          <el-skeleton-item
            v-for="i in columns"
            :key="`h-${i}`"
            variant="text"
            class="sk-cell"
            :style="{ width: `${widthOf(i)}%` }"
          />
        </div>
        <div
          v-for="r in rows"
          :key="`r-${r}`"
          class="sk-row"
          :class="{ 'sk-dense': dense }"
        >
          <el-skeleton-item
            v-for="i in columns"
            :key="`c-${r}-${i}`"
            variant="text"
            class="sk-cell"
            :style="{ width: `${widthOf(i)}%` }"
          />
        </div>
      </template>
    </el-skeleton>
  </div>
</template>

<script setup lang="ts">
import { ElSkeleton, ElSkeletonItem } from 'element-plus'

withDefaults(
  defineProps<{
    /** 占位行数：接近一屏可见行数即可，过多会让人误以为数据量很大 */
    rows?: number
    /** 列数：通常与真实表格可见列数一致 */
    columns?: number
    /** 紧凑行（配合表格 size=small） */
    dense?: boolean
    /** 无障碍播报文案 */
    label?: string
  }>(),
  { rows: 8, columns: 5, dense: false, label: '正在加载数据' },
)

// 各列宽度按固定节奏错开，避免所有灰条等宽显得像加载条而不是表格
const WIDTHS = [22, 16, 12, 20, 10, 14]
function widthOf(i: number): number {
  return WIDTHS[(i - 1) % WIDTHS.length]!
}
</script>

<style scoped>
.table-skeleton {
  width: 100%;
  border: 1px solid var(--border-light);
  border-radius: var(--radius-button);
  overflow: hidden;
  background: var(--bg-card);
}

.sk-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 16px;
}
.sk-row.sk-dense {
  padding: 6px 16px;
}

/* 表头比数据行略重，暗示「这是一张表的头」 */
.sk-head {
  background: var(--bg-card-header);
  opacity: 0.9;
}
.sk-head .sk-cell {
  height: 12px;
}

.sk-cell {
  height: 14px;
  border-radius: var(--radius-small);
}
</style>
