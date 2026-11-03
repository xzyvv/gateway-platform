<template>
  <div class="cfg-ver">
    <SectionHeader title="版本历史" icon="Files">
      <template #extra>
        <el-tag size="small" type="info" effect="plain" round>共 {{ versions.length }} 个版本</el-tag>
      </template>
    </SectionHeader>

    <div class="top-bar">
      <el-button type="primary" @click="load" :loading="loading">
        <el-icon><Refresh /></el-icon> 刷新
      </el-button>
      <el-button type="success" @click="backup" :loading="backingUp">
        <el-icon><FolderAdd /></el-icon> 立即备份
      </el-button>
      <el-button @click="openCompare">
        <el-icon><DocumentCopy /></el-icon> 版本对比
      </el-button>
      <el-button
        type="warning"
        plain
        :disabled="versions.length < 2"
        @click="undoToPrevious"
      >
        <el-icon><RefreshLeft /></el-icon> 撤销到上一版本
      </el-button>
    </div>

    <empty-state
      v-if="!loading && versions.length === 0"
      description="保存配置时会自动产生版本快照，方便随时回滚"
      action-text="刷新一下"
      @action="load"
    />

    <el-timeline v-else>
      <el-timeline-item
        v-for="v in versions"
        :key="v.id"
        :timestamp="formatTime(v.created_at)"
        placement="top"
        :type="v.is_current ? 'success' : 'primary'"
        :hollow="!v.is_current"
      >
        <el-card shadow="never" class="version-card">
          <div class="version-head">
            <div class="version-title">
              <el-tag size="large" type="primary" effect="dark" round>v{{ v.id }}</el-tag>
              <el-tag v-if="v.is_current" size="small" type="success" effect="dark" round>当前</el-tag>
              <span class="remark">{{ v.message || '(无备注)' }}</span>
            </div>
            <div class="version-ops">
              <el-button size="small" type="primary" link @click="viewSnapshot(v)">
                <el-icon><Document /></el-icon> 查看快照
              </el-button>
              <el-button size="small" type="info" link :disabled="v.is_current" @click="compareWithCurrent(v)">
                <el-icon><Connection /></el-icon> 与当前对比
              </el-button>
              <el-button size="small" type="warning" link :disabled="v.is_current" @click="rollback(v)">
                <el-icon><RefreshLeft /></el-icon> 回滚到此版本
              </el-button>
            </div>
          </div>
          <div class="version-meta">
            <span><el-icon><Clock /></el-icon> {{ formatTime(v.created_at) }}</span>
            <span v-if="v.size"><el-icon><Coin /></el-icon> {{ formatSize(v.size) }}</span>
          </div>
        </el-card>
      </el-timeline-item>
    </el-timeline>

    <el-dialog v-model="showSnapshot" :title="`版本 v${snapshotVersion?.id ?? ''} 配置快照`" width="800px" top="6vh">
      <div class="snapshot-body">
        <!-- 快照加载中用代码行骨架占位，避免用「加载中...」空态伪装 loading -->
        <el-skeleton v-if="snapshotLoading && !snapshotJson" animated class="snapshot-skeleton">
          <template #template>
            <el-skeleton-item
              variant="text"
              v-for="i in 10"
              :key="`sk-line-${i}`"
              class="sk-code-line"
              :class="`sk-w-${(i % 4) + 1}`"
            />
          </template>
        </el-skeleton>
        <pre v-else-if="snapshotJson">{{ snapshotJson }}</pre>
        <empty-state v-else description="暂无快照内容" />
      </div>
    </el-dialog>

    <el-dialog v-model="showCompare" title="版本对比" width="900px" top="6vh">
      <div class="compare-bar">
        <span>从</span>
        <el-select v-model="compareFrom" placeholder="选择版本" aria-label="对比源版本" size="small" filterable style="width: 200px">
          <el-option v-for="v in versions" :key="v.id" :label="`v${v.id} - ${v.message || '(无备注)'}`" :value="v.id" />
        </el-select>
        <el-icon><Right /></el-icon>
        <span>到</span>
        <el-select v-model="compareTo" placeholder="选择版本" aria-label="对比目标版本" size="small" filterable style="width: 200px">
          <el-option v-for="v in versions" :key="v.id" :label="`v${v.id} - ${v.message || '(无备注)'}`" :value="v.id" />
        </el-select>
        <el-button size="small" type="primary" @click="runCompare" :loading="compareLoading">对比</el-button>
      </div>

      <div v-if="compareResult" class="compare-result">
        <div class="compare-meta">
          <el-tag size="small" type="info" effect="plain" round>
            v{{ compareResult.from.id }} ({{ formatTime(compareResult.from.created_at) }})
          </el-tag>
          <el-icon><Right /></el-icon>
          <el-tag size="small" type="info" effect="plain" round>
            v{{ compareResult.to.id }} ({{ formatTime(compareResult.to.created_at) }})
          </el-tag>
          <el-tag size="small" :type="compareResult.has_changes ? 'warning' : 'success'" effect="dark" round>
            {{ compareResult.has_changes ? `${compareResult.changed_fields.length} 项差异` : '无差异' }}
          </el-tag>
        </div>

        <div class="changed-fields" v-if="compareResult.has_changes">
          <el-tag v-for="f in compareResult.changed_fields" :key="f" size="small" type="warning" effect="plain" round>
            {{ fieldLabel(f) }}
          </el-tag>
        </div>

        <div class="compare-panes">
          <div class="pane">
            <div class="pane-head">
              <el-tag size="small" type="primary" effect="dark" round>v{{ compareResult.from.id }}</el-tag>
              <span class="pane-time">{{ formatTime(compareResult.from.created_at) }}</span>
            </div>
            <pre class="pane-body">{{ compareFromJson }}</pre>
          </div>
          <div class="pane">
            <div class="pane-head">
              <el-tag size="small" type="success" effect="dark" round>v{{ compareResult.to.id }}</el-tag>
              <span class="pane-time">{{ formatTime(compareResult.to.created_at) }}</span>
            </div>
            <pre class="pane-body">{{ compareToJson }}</pre>
          </div>
        </div>
      </div>
      <empty-state v-else description="选择两个版本后点击对比" />
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ElMessage } from 'element-plus'
import { useConfigVersions } from '@/composables/config/useConfigVersions'

const {
  versions, loading, backingUp,
  showSnapshot, snapshotVersion, snapshotJson, snapshotLoading,
  showCompare, compareFrom, compareTo, compareResult,
  compareFromJson, compareToJson, compareLoading,
  load, backup, viewSnapshot, openCompare, compareWithCurrent, runCompare, rollback,
  formatTime, formatSize, fieldLabel,
} = useConfigVersions()

/**
 * 撤销到上一版本：原「撤销重做」页签的唯一独有能力，现并入版本历史页。
 * 版本对比已由本页的「版本对比」弹窗覆盖，故 ConfigHub 的 config-diff / undo-redo
 * 两个页签均已删除，避免同一能力出现三处入口、三种提示语。
 */
async function undoToPrevious() {
  const sorted = [...versions.value].sort((a, b) => b.id - a.id)
  const prev = sorted.find((v) => !v.is_current)
  if (!prev) {
    ElMessage.warning('没有可撤销的历史版本')
    return
  }
  await rollback(prev)
}
</script>

<style scoped>
.cfg-ver { padding: 0; }
.top-bar { display: flex; gap: 8px; margin-bottom: 16px; }
.version-card { margin-bottom: 4px; }
.version-head { display: flex; justify-content: space-between; align-items: center; }
.version-title { display: flex; align-items: center; gap: 10px; }
.remark { font-size: 0.875rem; font-weight: 600; color: var(--text-primary); }
.version-ops { display: flex; gap: 4px; }
.version-meta { display: flex; gap: 16px; font-size: 0.75rem; color: var(--text-secondary); margin-top: 8px; flex-wrap: wrap; }
.version-meta span { display: inline-flex; align-items: center; gap: 4px; }
.snapshot-body {
  max-height: 60vh; overflow: auto; background: var(--bg-fill); padding: 0; border-radius: var(--radius-button);
}
.snapshot-body pre { margin: 0; font-family: 'Consolas', 'Monaco', monospace; font-size: 0.75rem; white-space: pre-wrap; word-break: break-all; }
/* 快照骨架：模拟 JSON 代码行，宽度错开更像真实文本 */
.snapshot-skeleton { padding: 16px; }
.sk-code-line { height: 12px; margin-bottom: 8px; }
.sk-w-1 { width: 92%; }
.sk-w-2 { width: 78%; }
.sk-w-3 { width: 84%; }
.sk-w-4 { width: 64%; }
.compare-bar { display: flex; align-items: center; gap: 8px; margin-bottom: 16px; flex-wrap: wrap; }
.compare-result { display: flex; flex-direction: column; gap: 12px; }
.compare-meta { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.changed-fields { display: flex; gap: 6px; flex-wrap: wrap; }
.compare-panes { display: grid; grid-template-columns: 1fr 1fr; gap: 12px; }
.pane { border: 1px solid var(--border-base); border-radius: var(--radius-button); overflow: hidden; }
.pane-head { display: flex; align-items: center; gap: 8px; padding: 0 12px; background: var(--bg-fill); border-bottom: 1px solid var(--border-base); }
.pane-time { font-size: 0.75rem; color: var(--text-secondary); }
.pane-body {
  margin: 0; padding: 0; max-height: 40vh; overflow: auto;
  font-family: 'Consolas', 'Monaco', monospace; font-size: 0.75rem;
  white-space: pre-wrap; word-break: break-all;
}
</style>
