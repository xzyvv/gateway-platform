<template>
  <el-form-item label="上报策略">
    <!-- 4 个预设是「一次选择 + 引导」：实时/变化/统计直接写 report_mode；
         变化与统计的另一半在点位级配置（死区 / 聚合窗口），这里只负责引导与验收提示。
         不新增落盘字段，避免与 mqtt.report_mode 形成双重真相。 -->
    <div v-if="!mqtt?.enabled" class="field-hint disabled-hint" data-disabled-hint>
      MQTT 未启用，上报策略保存后不生效。
    </div>
    <div class="report-strategy">
      <button
        v-for="p in REPORT_PROFILES"
        :key="p.key"
        type="button"
        class="strategy-card"
        :class="{ active: activeProfile === p.key }"
        :aria-pressed="activeProfile === p.key"
        :data-strategy="p.key"
        @click="selectProfile(p.key)"
      >
        <span class="strategy-head">
          <span class="strategy-title">{{ p.label }}</span>
          <span v-if="p.guide" class="strategy-badge">配置向导</span>
        </span>
        <span class="strategy-desc">{{ p.desc }}</span>
      </button>
    </div>
    <div v-if="activeGuide" class="strategy-guide" data-guide>
      <div class="guide-title">{{ activeGuide.title }}</div>
      <ol class="guide-steps">
        <li v-for="(s, i) in activeGuide.steps" :key="i">
          {{ s.text }}
          <button v-if="s.goto" type="button" class="guide-link" @click="go(s.goto)">
            {{ s.linkText }}
          </button>
        </li>
      </ol>
      <div class="field-hint">
        该预设只做配置引导、不写入额外字段：实际生效的是上报模式 + 各点位的死区 / 聚合配置。
        配置完成后请回到本页确认策略与点位一致。
      </div>
    </div>
    <div v-else class="field-hint">
      stream：每个采样值都上行；snapshot：每个上报周期每个点位只发最新值（对齐 OPC UA
      queueSize=1 与主流网关周期快照），中间样本仅保留在本地历史，适合采样快于上报的现场。
    </div>
  </el-form-item>
</template>

<script setup lang="ts">
/**
 * 上报策略预设：把「上报模式 + 点位级过滤/聚合」的组合知识固化成可选卡片。
 *
 * 持久化字段仍然只有 `mqtt.report_mode` 与点位级 `deadband/on_change/aggregation`，
 * 这里不新增落盘字段——两处表达同一件事会形成双重真相，校验与文档都要翻倍。
 * 因此「变化上报」= 逐样本 + 点位级死区（每次变化都是一条事件，不做周期折叠），
 * 「统计上报」= 周期快照 + 点位级窗口聚合，两者都只给出另外一步的引导；
 * 引导选中态仅存在于当前编辑会话，刷新后按 report_mode 回显为实时/快照。
 */
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import type { MqttConfig } from '@/api/types'

const mqtt = defineModel<MqttConfig>({ required: true })

const router = useRouter()

type ProfileKey = 'realtime' | 'snapshot' | 'change' | 'stats'

interface ProfileGoto {
  path: string
  query?: Record<string, string>
}

interface ProfileStep {
  text: string
  linkText?: string
  goto?: ProfileGoto
}

const REPORT_PROFILES: Array<{
  key: ProfileKey
  label: string
  desc: string
  guide?: boolean
}> = [
  {
    key: 'realtime',
    label: '实时逐样本',
    desc: '每个采样值都上行，适合录波、追溯与要求逐样本的对接方。',
  },
  {
    key: 'snapshot',
    label: '周期快照',
    desc: '每个上报周期每个点位只发最新值，适合云端只要当前值的遥测。',
  },
  {
    key: 'change',
    label: '变化上报',
    desc: '逐样本 + 点位级死区，每次变化都完整上行（未配死区时等同逐样本）。',
    guide: true,
  },
  {
    key: 'stats',
    label: '统计上报',
    desc: '周期快照 + 点位级窗口聚合，上行 avg / max / min / sum / last 统计值。',
    guide: true,
  },
]

/** 仅当前编辑会话内的引导态：选了变化/统计就展开步骤，切到实时/快照即收起 */
const guideProfile = ref<'change' | 'stats' | null>(null)

// 父级「放弃修改 / 重新加载配置」会替换整个 mqtt 对象，此时引导态必须一起复位，
// 否则会出现「配置已是逐样本、卡片仍高亮统计上报」的误导性展示。
watch(mqtt, () => {
  guideProfile.value = null
})

const activeProfile = computed<ProfileKey>(() => {
  if (guideProfile.value) return guideProfile.value
  return mqtt.value?.report_mode === 'snapshot' ? 'snapshot' : 'realtime'
})

function selectProfile(key: ProfileKey) {
  const m = mqtt.value
  if (!m) return
  if (key === 'realtime') {
    m.report_mode = 'stream'
    guideProfile.value = null
    return
  }
  if (key === 'change') {
    // 变化上报必须是逐样本：snapshot 会把同一上报周期内的多次变化折叠成最后一次，
    // 开关量 on→off→on 这类事件流水会直接丢失中间态。
    m.report_mode = 'stream'
    guideProfile.value = 'change'
    return
  }
  // 统计上报与周期快照天然配套：每个窗口一条聚合值，同周期的重复输出没有意义
  m.report_mode = 'snapshot'
  guideProfile.value = key === 'snapshot' ? null : key
}

const activeGuide = computed<{ title: string; steps: ProfileStep[] } | null>(() => {
  if (!guideProfile.value) return null
  const interval = mqtt.value?.publish_interval ?? 0
  if (guideProfile.value === 'change') {
    return {
      title: '变化上报 = 逐样本 + 点位级死区，还需两步：',
      steps: [
        {
          text: '到「数据压缩」按设备批量设置死区，并勾选「变化才上传」',
          linkText: '打开数据压缩',
          goto: { path: '/data', query: { tab: 'data-compression' } },
        },
        {
          text: '死区为 0 时仅过滤完全相同的值，基本等价于逐样本；静默点的在线判活当前版本未内置，需另行配置。',
        },
      ],
    }
  }
  return {
    title: '统计上报 = 周期快照 + 点位级窗口聚合，还需两步：',
    steps: [
      {
        text: '在点位页为每个点位配置聚合窗口与函数（avg / max / min / sum / last）',
        linkText: '打开点位配置',
        goto: { path: '/points' },
      },
      {
        text: `聚合窗口必须 ≥ 上报周期（当前 ${interval} 秒），否则保存会被拒绝；未配置聚合的点位按周期快照上行。`,
      },
    ],
  }
})

function go(target: ProfileGoto) {
  void router.push(target)
}
</script>

<style scoped>
/* 4 张卡片一屏内选完；按钮复位为卡片外观，保留原生键盘可达性 */
.report-strategy {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  gap: 8px;
  width: 100%;
}
.strategy-card {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 10px 12px;
  border: 1px solid var(--border-lighter);
  border-radius: var(--radius-button);
  background: transparent;
  color: inherit;
  font: inherit;
  text-align: left;
  cursor: pointer;
}
.strategy-card:hover,
.strategy-card.active {
  border-color: var(--color-primary);
}
/* 选中底色用主题背景色而非 color-mix：后者在旧 Chromium 上整条声明被丢弃，
   且与主色相无关的硬编码 rgba 会在换肤后失去一致性。 */
.strategy-card.active {
  background: var(--bg-fill);
}
.strategy-card:focus-visible {
  outline: 2px solid var(--color-primary);
  outline-offset: 2px;
}
.strategy-head {
  display: flex;
  align-items: center;
  gap: 8px;
}
.strategy-title {
  font-weight: 600;
}
.strategy-badge {
  padding: 0 6px;
  border-radius: var(--radius-tag);
  font-size: 0.6875rem;
  color: var(--text-secondary);
  border: 1px solid var(--border-lighter);
}
.strategy-desc {
  font-size: 0.75rem;
  line-height: 1.5;
  color: var(--text-secondary);
}
.strategy-guide {
  margin-top: 8px;
  padding: 10px 12px;
  width: 100%;
  border-left: 3px solid var(--color-primary);
  background: var(--bg-fill);
}
.guide-title {
  font-size: 0.8125rem;
  font-weight: 600;
}
.guide-steps {
  margin: 6px 0 6px 18px;
  padding: 0;
  font-size: 0.75rem;
  line-height: 1.7;
  color: var(--text-secondary);
}
.guide-link {
  margin-left: 6px;
  padding: 0;
  border: none;
  background: none;
  color: var(--color-primary, #409eff);
  font: inherit;
  cursor: pointer;
  text-decoration: underline;
}
.field-hint {
  margin-top: 4px;
  font-size: 0.75rem;
  line-height: 1.5;
  color: var(--text-secondary);
}
.disabled-hint {
  margin-bottom: 8px;
}
</style>
