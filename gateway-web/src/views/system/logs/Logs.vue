<template>
  <div class="logs">
    <el-card shadow="never" class="section-card log-card">
      <template #header>
        <SectionHeader title="实时日志" icon="Document">
          <template #extra>
            <el-tag :type="connected ? 'success' : (reconnectAttempts > 0 ? 'warning' : 'danger')" effect="dark" round size="small" class="conn-tag">
              <span class="dot" :class="{ active: connected, reconnecting: reconnectAttempts > 0 && !connected }"></span>
              {{ connected ? '已连接' : (reconnectAttempts > 0 ? `重连中 (${reconnectAttempts})` : '未连接') }}
            </el-tag>
            <el-tag
              type="info"
              effect="plain"
              round
              size="small"
              class="count-tag"
              title="左侧为当前过滤后的条数，右侧为本次会话收到的总条数"
            >
              {{ filteredLogs.length }} / {{ logs.length }} 条
            </el-tag>
          </template>
          <el-button size="small" @click="exportLogs"><el-icon><Download /></el-icon> 导出日志</el-button>
          <el-button size="small" @click="clearLogs"><el-icon><Delete /></el-icon> 清空</el-button>
          <el-button size="small" :type="paused ? 'success' : 'warning'" @click="togglePause">
            <el-icon><VideoPause v-if="!paused" /><VideoPlay v-else /></el-icon>
            {{ paused ? '继续' : '暂停' }}
          </el-button>
        </SectionHeader>
      </template>

      <div class="filter-bar">
        <el-input v-model="filter" placeholder="过滤关键词（支持正则，如: ERROR|WARN）" aria-label="过滤日志" clearable class="filter-input">
          <template #prefix><el-icon><Search /></el-icon></template>
        </el-input>
        <div class="level-filters">
          <el-button v-for="lv in levels" :key="lv.key" size="small" :type="activeLevel === lv.key ? lv.type : 'default'" :plain="activeLevel !== lv.key" round @click="activeLevel = lv.key">
            {{ lv.label }}
          </el-button>
        </div>
        <div class="alert-rule">
          <el-input v-model="alertKeywords" placeholder="告警关键词（逗号分隔，含任一即通知）" aria-label="告警关键词" clearable size="small" class="alert-input" @keyup.enter="updateAlertRules" @blur="updateAlertRules">
            <template #prefix><el-icon><Warning /></el-icon></template>
          </el-input>
          <el-tag v-if="alertCount > 0" type="danger" effect="dark" round size="small" class="alert-count">
            {{ alertCount }} 条告警
          </el-tag>
        </div>
      </div>

      <div ref="logBox" class="log-box" @scroll="onScroll">
        <!-- 超过阈值走窗口化渲染：上下占位撑出真实滚动条，中间只画可视窗口 -->
        <div class="log-spacer" :style="{ height: `${topPad}px` }" aria-hidden="true"></div>
        <div
          v-for="item in renderItems"
          :key="item.id"
          class="log-line"
          :class="logLevelClass(item.text)"
          :data-lid="item.id"
          v-html="highlightLine(item.text)"
        ></div>
        <div class="log-spacer" :style="{ height: `${bottomPad}px` }" aria-hidden="true"></div>
        <div v-if="filteredLogs.length === 0" class="log-empty">
          <el-icon><Document /></el-icon>
          <!-- 空态要区分「过滤掉了」和「真的没日志」：否则 0/0 会被误读成日志功能坏了 -->
          <span v-if="filter.trim() || activeLevel !== 'all'">
            当前过滤条件下没有日志，清空关键词或切回「全部」级别试试
          </span>
          <span v-else>
            暂无日志：网关只在新事件发生时推送，此处空白通常表示服务运行平稳；可用下方诊断命令确认进程状态
          </span>
        </div>
      </div>
      <transition name="jump-fade">
        <el-button v-show="!atBottom && filteredLogs.length > 0" class="jump-to-bottom" type="primary" size="small" circle @click="scrollToBottom" title="回到最新" aria-label="回到最新日志">
          <el-icon><Bottom /></el-icon>
        </el-button>
      </transition>
      <!-- 读屏专用播报区（视觉隐藏）：ERROR 新增时 polite 播报，不打断当前朗读 -->
      <div class="sr-only" role="status" aria-live="polite">{{ errorAnnouncement }}</div>
    </el-card>

    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="诊断命令" icon="Tools" />
      </template>
      <div class="quick-cmds">
        <span class="quick-cmds-label">常用命令：</span>
        <el-button v-for="c in quickCommands" :key="c" size="small" plain type="primary" :loading="cmdLoading && command === c" @click="runQuick(c)">{{ c }}</el-button>
      </div>
      <el-input v-model="command" placeholder="输入命令（如: df -h, free -m, uptime）" aria-label="执行命令" clearable @keyup.enter="execCommand" class="cmd-input">
        <template #append>
          <el-button type="primary" :loading="cmdLoading" :disabled="!command.trim()" @click="execCommand">
            <el-icon><Right /></el-icon> 执行
          </el-button>
        </template>
      </el-input>
      <!-- B2 修复：实时显示预校验结果，让用户在按下"执行"前就能看到是否被白名单拒 -->
      <div v-if="command.trim() && cmdPrecheck && !cmdPrecheck.ok" class="cmd-precheck-error">
        <el-icon><Warning /></el-icon>
        <span>{{ cmdPrecheck.error }}</span>
      </div>
      <div v-if="cmdOutput" class="cmd-output-wrap">
        <el-button size="small" plain class="cmd-copy-btn" @click="copyOutput">
          <el-icon><CopyDocument /></el-icon> 复制
        </el-button>
        <pre class="cmd-output">{{ cmdOutput }}</pre>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useLogs } from '@/composables/system/useLogs'
import { useQueryState } from '@/composables/core/useQueryState'

const {
  connected, reconnectAttempts, paused, logs, filter,
  alertKeywords, alertCount, command, cmdOutput, cmdLoading, atBottom, activeLevel,
  filteredLogs, cmdPrecheck,
  logBox, renderItems, topPad, bottomPad,
  levels, quickCommands,
  logLevelClass, highlightLine,
  updateAlertRules, clearLogs, exportLogs, togglePause,
  onScroll, scrollToBottom, execCommand, runQuick, copyOutput,
} = useLogs()

// 模板里的 ref="logBox" 是按名字绑定 setup 变量的，vue-tsc 的 noUnusedLocals 看不到这层引用
void logBox

// 过滤关键词与级别与 URL 同步：切 tab 返回后还原（?filter=x&level=error）
const qsFilter = useQueryState('filter')
const qsLevel = useQueryState('level')
if (qsFilter.value) filter.value = qsFilter.value
if (qsLevel.value && levels.some((l) => l.key === qsLevel.value)) activeLevel.value = qsLevel.value
watch([filter, activeLevel], ([f, lv]) => {
  qsFilter.value = f
  qsLevel.value = lv === 'all' ? '' : lv
})

// ===== ERROR 计数播报（无障碍 P2-8）=====
// 读屏/挂机场景不逐行读日志流，只对"新增 ERROR"做 polite 播报：文本更新才触发
// 播报且 5s 节流，日志风暴时不会连环打断；文案只报数量，不泄漏日志内容。
const errorTotal = computed(() => {
  let n = 0
  for (const l of logs.value) if (logLevelClass(l.text) === 'log-error') n++
  return n
})
const errorAnnouncement = ref('')
let lastAnnounceAt = 0
watch(errorTotal, () => {
  const now = Date.now()
  if (now - lastAnnounceAt < 5000) return
  lastAnnounceAt = now
  errorAnnouncement.value = `日志出现 ERROR，累计 ${errorTotal.value} 条`
})
</script>

<style scoped src="./Logs.css"></style>
