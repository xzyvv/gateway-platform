<template>
  <div v-if="visible" class="conn-banner" :class="`conn-${tone}`" role="alert" aria-live="assertive">
    <span class="conn-icon" aria-hidden="true">!</span>
    <div class="conn-body">
      <div class="conn-title">{{ title }}</div>
      <div class="conn-detail">
        {{ detail }}
        <router-link
          v-if="hasQueueBacklog"
          class="conn-link"
          to="/data?tab=data-buffering"
        >查看断连续传 ›</router-link>
      </div>
    </div>
    <el-button
      v-if="unreachable"
      class="conn-retry"
      size="small"
      :loading="retrying"
      @click="emit('retry')"
    >
      立即重试
    </el-button>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'

const props = withDefaults(
  defineProps<{
    /** 连不上网关（断网 / 进程退出 / 地址错误） */
    unreachable?: boolean
    /** 网关可达但组件降级 */
    degraded?: boolean
    /** 距上次成功通信的秒数；null 表示从未成功 */
    staleSeconds?: number | null
    /** 降级组件明细 */
    components?: string[]
    /** 队列积压条数（健康快照自带）；有值时拼入队列积压文案 */
    queueLen?: number | null
    /** 连续失败次数（用于抑制单次抖动闪烁） */
    failStreak?: number
    /** 重试进行中 */
    retrying?: boolean
  }>(),
  {
    unreachable: false,
    degraded: false,
    staleSeconds: null,
    components: () => [],
    queueLen: null,
    failStreak: 0,
    retrying: false,
  },
)

const emit = defineEmits<{ retry: [] }>()

/** 单次失败不提示，连续 2 次（约 10s）才认为真的断了 */
const visible = computed(
  () => (props.unreachable && props.failStreak >= 2) || props.degraded,
)

const tone = computed(() => (props.unreachable ? 'danger' : 'warning'))

const title = computed(() =>
  props.unreachable ? '无法连接网关' : '网关部分组件异常',
)

function relative(secs: number): string {
  if (secs < 60) return `${secs} 秒前`
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟前`
  return `${Math.floor(secs / 3600)} 小时前`
}

const detail = computed(() => {
  if (props.unreachable) {
    // 明确告知「看到的是历史数据」，这是现场最危险的误读场景
    return props.staleSeconds == null
      ? '尚未成功连接过网关，当前页面没有实时数据。'
      : `页面数据停留在 ${relative(props.staleSeconds)}，不是实时值。`
  }
  if (props.components.length) {
    // 队列积压附上具体条数，现场无需跳转即可判断严重程度
    const items = props.components.map((c) =>
      c.includes('队列积压') && props.queueLen != null
        ? `${c} ${props.queueLen.toLocaleString()} 条`
        : c,
    )
    return `以下组件未就绪：${items.join('、')}`
  }
  return '部分组件未就绪，数据可能不完整。'
})

/** 是否有队列积压组件：是则提供「查看断连续传」直达链接 */
const hasQueueBacklog = computed(() => props.components.some((c) => c.includes('队列积压')))
</script>

<style scoped>
.conn-banner {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  margin-bottom: 8px;
  border-radius: var(--radius-button);
  font-size: 0.8125rem;
  line-height: 1.5;
}
.conn-danger {
  /* 文字用 --text-danger(#b91c1c)：直接拿 --color-danger(#f56c6c) 当字色在
   * danger-bg 上只有 2.6:1，越是要看清的断连告警越模糊 */
  color: var(--text-danger);
  background: var(--color-danger-bg);
  border: 1px solid var(--color-danger);
}
.conn-warning {
  color: var(--text-warning);
  background: var(--color-warning-bg);
  border: 1px solid var(--color-warning);
}
.conn-icon {
  flex: none;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  color: var(--text-inverse);
  /* 不能用 currentColor：.conn-icon 自身已把 color 设为 --text-inverse，
   * 取 currentColor 会得到白色底 + 白色感叹号，图标等于不可见 */
  background: var(--text-danger);
  font-size: 0.75rem;
  font-weight: 700;
  display: inline-flex;
  align-items: center;
  justify-content: center;
}
.conn-warning .conn-icon {
  background: var(--text-warning);
}
.conn-body {
  flex: 1;
  min-width: 0;
}
.conn-title {
  font-weight: 600;
}
.conn-detail {
  opacity: 0.85;
  font-size: 0.75rem;
}
.conn-link {
  margin-left: 8px;
  color: inherit;
  font-weight: 600;
  text-decoration: underline;
  text-underline-offset: 2px;
}
.conn-retry {
  flex: none;
}
</style>
