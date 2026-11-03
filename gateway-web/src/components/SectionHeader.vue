<template>
  <div v-if="showWrap" class="section-header-wrap">
    <div v-if="!isDuplicate" class="section-header">
      <el-icon v-if="icon">
        <component :is="icon" />
      </el-icon>
      <component :is="as" class="section-title-text">{{ title }}</component>
      <slot name="extra" />
    </div>
    <!-- 标题与当前 Hub 页签名相同时：页签名已由 HubShell 渲染成全页唯一 h1，
         这里不再重复标题本身，只保留徽标/计数等附加信息，避免信息丢失 -->
    <div v-else-if="$slots.extra" class="header-extra" :class="{ 'is-solo': !$slots.default }">
      <slot name="extra" />
    </div>
    <div v-if="$slots.default" class="header-actions">
      <slot />
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * 区块标题公共组件
 *
 * 遵循 VISUAL_SPEC §3.1 / §3.2 / §4：
 *   - 左侧渐变装饰条：linear-gradient(90deg, var(--color-primary) 0%, var(--color-accent) 100%)，4×16px，圆角 --radius-bar
 *   - 图标徽章：28×28px，rgba(var(--color-primary-rgb), 0.1) 背景，圆角 --radius-section-icon
 *   - 标题文字：16px / 600
 *
 * Props:
 *   - title: 标题文本（必填）
 *   - icon:  Element Plus 图标组件名（字符串），如 "Cpu"、"Key"
 *
 * Slots:
 *   - default: 右侧操作区（按钮等），不传则不渲染
 *   - extra:   标题右侧附加内容（如数量徽标 el-tag）
 *
 * ## 与 Hub 页签同名时自动省掉标题
 *
 * 侧栏模式的 Hub（页签 ≥6）页头已显示页签名作为唯一 h1，页内再用同名
 * SectionHeader 会造成同屏双标题（「运行参数」上下各一次）。HubShell 通过
 * hubTabLabelKey 注入当前页签名，本组件与之比对，相同则只渲染操作区。
 * 非 Hub 页面、或标题与页签不同名（页面内的小节标题）不受影响。
 */
import { computed, inject, useSlots } from 'vue'
import { hubTabLabelKey } from '@/composables/core/useHubContext'

const props = withDefaults(defineProps<{
  title: string
  icon?: string
  /** 语义标签：默认 h2（页面内区块标题）；页面级标题可传 h1 */
  as?: 'h1' | 'h2' | 'h3' | 'span'
}>(), {
  as: 'h2',
})

const slots = useSlots()

/** 当前 Hub 页签名；不在 Hub 内为 null */
const hubTabLabel = inject(hubTabLabelKey, null)

/** 标题与页签名重复：标题本体不再渲染 */
const isDuplicate = computed(() => !!hubTabLabel?.value && hubTabLabel.value === props.title)

/** 标题被省掉后若既无徽标也无按钮，整块不渲染（避免留下空壳） */
const showWrap = computed(() => !isDuplicate.value || !!(slots.extra || slots.default))
</script>

<style scoped>
.section-header-wrap {
  display: flex;
  justify-content: space-between;
  align-items: center;
  flex-wrap: wrap;
  gap: 12px;
}

.section-header {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 1rem;
  font-weight: 600;
  color: var(--text-primary);
  position: relative;
  padding-left: 12px;
}

/* VISUAL_SPEC §3.1：渐变方向统一 90deg */
.section-header::before {
  content: '';
  position: absolute;
  left: 0;
  top: 50%;
  transform: translateY(-50%);
  width: 4px;
  height: 16px;
  border-radius: var(--radius-bar);
  background: linear-gradient(90deg, var(--color-primary) 0%, var(--color-accent) 100%);
}

/* 语义标题默认带外边距，显式清零，保持与原 span 版视觉一致 */
.section-title-text {
  margin: 0;
  font: inherit;
  line-height: inherit;
  color: inherit;
}

.section-header .el-icon {
  color: var(--color-primary);
  font-size: 1rem;
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: rgba(var(--color-primary-rgb), 0.1);
  border-radius: var(--radius-section-icon);
  flex-shrink: 0;
}

/* 标题被省掉时保留的附加信息（数量徽标等） */
.header-extra {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}

/* 只剩徽标、没有操作按钮时靠右，避免在左侧孤零零地留一个药丸 */
.header-extra.is-solo {
  margin-left: auto;
}

.header-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
</style>
