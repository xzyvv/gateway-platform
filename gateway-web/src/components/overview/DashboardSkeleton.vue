<template>
  <!--
    仪表盘首屏骨架：按真实布局（6 张汇总卡 + 两个区块）占位，
    加载完成时尺寸一致，避免"转圈 → 内容突然撑开"的跳动。
  -->
  <div class="dash-skeleton" role="status" aria-live="polite" aria-busy="true" aria-label="正在加载仪表盘">
    <el-row :gutter="12" class="sk-summary">
      <el-col
        v-for="i in 6"
        :key="i"
        :xs="12"
        :sm="8"
        :md="6"
        :lg="4"
      >
        <div class="sk-card">
          <el-skeleton animated>
            <template #template>
              <div class="sk-card-inner">
                <el-skeleton-item variant="circle" class="sk-icon" />
                <div class="sk-lines">
                  <el-skeleton-item variant="h3" style="width: 55%" />
                  <el-skeleton-item variant="text" style="width: 80%; margin-top: 8px" />
                </div>
              </div>
            </template>
          </el-skeleton>
        </div>
      </el-col>
    </el-row>

    <div class="sk-section">
      <el-skeleton :rows="2" animated />
    </div>
    <div class="sk-section">
      <el-skeleton :rows="3" animated />
    </div>
  </div>
</template>

<script setup lang="ts">
import { ElCol, ElRow, ElSkeleton, ElSkeletonItem } from 'element-plus'
</script>

<style scoped>
.dash-skeleton {
  display: flex;
  flex-direction: column;
  gap: 20px;
}

.sk-summary {
  margin-bottom: 4px;
}

.sk-card {
  padding: 16px;
  border-radius: 12px;
  background: var(--bg-card);
  border: 1px solid var(--border-light);
  margin-bottom: 12px;
}

.sk-card-inner {
  display: flex;
  align-items: center;
  gap: 12px;
}

.sk-icon {
  width: 40px;
  height: 40px;
  flex-shrink: 0;
}

.sk-lines {
  flex: 1;
  min-width: 0;
}

.sk-section {
  padding: 20px;
  border-radius: var(--radius-card);
  background: var(--bg-card);
  border: 1px solid var(--border-light);
}
</style>
