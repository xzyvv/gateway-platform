<template>
  <el-card shadow="never" class="section-card attention-card">
    <template #header>
      <SectionHeader title="需要关注" icon="Warning">
        <template #extra>
          <el-tag v-if="errorCount" type="danger" effect="dark" round size="small" class="sev-badge">
            {{ errorCount }} 项异常
          </el-tag>
          <el-tag v-else-if="signals.length" type="warning" effect="plain" round size="small" class="sev-badge">
            {{ signals.length }} 项提醒
          </el-tag>
        </template>
      </SectionHeader>
    </template>

    <!-- 一切正常：明确说「没异常」，而不是留白让人怀疑是加载失败 -->
    <div v-if="signals.length === 0" class="attention-ok">
      <el-icon class="ok-icon"><CircleCheckFilled /></el-icon>
      <span>当前无异常：组件在线、资源占用与采集队列均在阈值内。</span>
    </div>

    <ul v-else class="attention-list">
      <li v-for="s in signals" :key="s.id" class="attention-item" :class="`sev-${s.severity}`">
        <span class="sev-dot" aria-hidden="true"></span>
        <div class="item-body">
          <div class="item-title">
            {{ s.title }}
            <el-tag :type="s.severity === 'error' ? 'danger' : 'warning'" size="small" effect="plain" round>
              {{ s.severity === 'error' ? '异常' : '提醒' }}
            </el-tag>
          </div>
          <div class="item-detail" :title="s.detail">{{ s.detail }}</div>
        </div>
        <el-button size="small" type="primary" link class="item-action" @click="go(s)">
          {{ s.action }}
        </el-button>
      </li>
    </ul>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 仪表盘异常聚合入口。
 *
 * 承接 useHealthSignals 的派生结果：把「哪几处异常」压成一屏，每条直接给出
 * 处理页路由。相比原来的 6 张只读汇总卡片，工程师不需要再自己判断该进哪个
 * 诊断页。无异常时显式给出「一切正常」，避免把空白误读成数据没加载出来。
 */
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { CircleCheckFilled } from '@element-plus/icons-vue'
import type { HealthSignal } from '@/composables/dashboard/useHealthSignals'

const props = defineProps<{ signals: HealthSignal[] }>()

const router = useRouter()

const errorCount = computed(() => props.signals.filter((s) => s.severity === 'error').length)

function go(s: HealthSignal) {
  router.push(s.to)
}
</script>

<style scoped>
.attention-card .sev-badge { margin-left: 4px; }

.attention-ok {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 14px 4px;
  font-size: 0.8125rem;
  color: var(--text-secondary);
}
.attention-ok .ok-icon {
  font-size: 1.25rem;
  color: var(--color-success);
  flex-shrink: 0;
}

.attention-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.attention-item {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 10px 12px;
  border-radius: var(--radius-table);
  border: 1px solid var(--border-base);
  background: var(--bg-card-header);
  transition: border-color 0.2s, background 0.2s;
}
.attention-item:hover { border-color: var(--color-primary); }
.attention-item.sev-error {
  border-left: 3px solid var(--color-danger);
  background: var(--color-danger-bg);
}
.attention-item.sev-warning {
  border-left: 3px solid var(--color-warning);
  background: var(--color-warning-bg);
}

.sev-dot {
  flex: none;
  width: 8px;
  height: 8px;
  border-radius: 50%;
}
.sev-error .sev-dot { background: var(--color-danger); }
.sev-warning .sev-dot { background: var(--color-warning); }

.item-body { flex: 1; min-width: 0; }
.item-title {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 0.875rem;
  font-weight: 600;
  color: var(--text-primary);
}
.item-detail {
  margin-top: 3px;
  font-size: 0.75rem;
  line-height: 1.5;
  color: var(--text-secondary);
}
.item-action { flex: none; white-space: nowrap; }

/* N10: 1366×768 首屏收敛。异常条目在矮屏上占两行（标题 + 明细），2 条就把
 * 卡片撑到 237px。这里改为单行：明细与标题同行、超长省略，完整文案走 title 提示。
 * 只影响 max-height≤820 的视口，大屏仍保留两行完整阅读。 */
@media (min-width: 1200px) and (max-height: 820px) {
  .attention-list { gap: 6px; }
  .attention-item { padding: 6px 10px; gap: 8px; }
  .item-body {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .item-title { flex: none; }
  .item-detail {
    margin-top: 0;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
}
</style>