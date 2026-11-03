<template>
  <div class="groups">
    <SectionHeader title="设备分组" icon="CollectionTag" />

    <div class="top-bar">
      <el-button type="primary" @click="create"
        ><el-icon><Plus /></el-icon> 新建分组</el-button
      >
      <el-button @click="load"
        ><el-icon><Refresh /></el-icon> 刷新</el-button
      >
    </div>

    <!-- 分组卡片 -->
    <div class="group-grid">
      <div v-for="g in groups" :key="g.id" class="group-card" :style="{ borderColor: g.color }">
        <div class="group-card-head" :style="{ background: g.color + '15' }">
          <div class="group-color" :style="{ background: g.color }"></div>
          <div class="group-info">
            <div class="group-name">{{ g.name }}</div>
            <div class="group-desc">{{ g.description }}</div>
          </div>
          <div class="group-count">
            <el-tag size="large" type="primary" effect="dark" round>{{
              g.deviceIds.length
            }}</el-tag>
          </div>
        </div>
        <div class="group-card-body">
          <div class="device-tags">
            <el-tag
              v-for="did in g.deviceIds"
              :key="did"
              size="small"
              effect="plain"
              class="device-tag"
            >
              <el-icon><Cpu /></el-icon> {{ did }}
            </el-tag>
            <el-tag v-if="g.deviceIds.length === 0" type="info" effect="plain" size="small"
              >暂无设备</el-tag
            >
          </div>
        </div>
        <div class="group-card-foot">
          <el-button size="small" type="primary" link @click="manageDevices(g)">管理设备</el-button>
          <el-button size="small" type="warning" link @click="edit(g)">编辑</el-button>
          <el-button size="small" type="danger" link @click="remove(g)">删除</el-button>
        </div>
      </div>
    </div>

    <!-- 新建/编辑分组 -->
    <el-dialog
      :close-on-click-modal="false"
      v-model="showCreate"
      :title="isEdit ? '编辑分组' : '新建分组'"
      width="480px"
      :before-close="createGuard.beforeClose"
      @close="
        isEdit = false;
        form.name = '';
        form.description = '';
        form.color = CHART_PRIMARY
      "
    >
      <el-form label-width="80px">
        <el-form-item label="名称"
          ><el-input v-model="form.name" placeholder="请输入分组名称"
        /></el-form-item>
        <el-form-item label="描述"
          ><el-input v-model="form.description" placeholder="该分组下可选择的设备"
        /></el-form-item>
        <el-form-item label="颜色">
          <div class="color-picker">
            <span
              v-for="c in presetColors"
              :key="c"
              class="color-dot"
              :class="{ active: form.color === c }"
              :style="{ background: c }"
              role="button"
              tabindex="0"
              aria-label="选择分组颜色"
              @click="form.color = c"
              @keydown.enter.prevent="form.color = c"
              @keydown.space.prevent="form.color = c"
            ></span>
          </div>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createGuard.close()">取消</el-button>
        <el-button type="primary" :loading="saving" :disabled="saving" @click="save">保存</el-button>
      </template>
    </el-dialog>

    <!-- 为分组选择设备 -->
    <el-dialog v-model="showManage" title="为分组选择设备" width="560px" :before-close="manageGuard.beforeClose">
      <p class="tip">请选择要加入到"{{ currentGroup?.name }}"的设备</p>
      <el-checkbox-group v-model="selectedDevices">
        <el-checkbox
          v-for="d in allDevices"
          :key="d.id"
          :value="d.id"
          style="display: block; margin: 6px 0"
        >
          {{ d.name }} ({{ d.id }})
        </el-checkbox>
      </el-checkbox-group>
      <template #footer>
        <el-button @click="manageGuard.close()">取消</el-button>
        <el-button type="primary" :loading="savingDevices" :disabled="savingDevices" @click="saveDevices">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted } from 'vue'
import { useRoute } from 'vue-router'
import { ElMessage } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { Plus, Refresh, Cpu } from '@element-plus/icons-vue'
import {
  listGroupsView,
  createGroupView,
  updateGroupView,
  deleteGroup,
  addDeviceToGroup,
  removeDeviceFromGroup,
  presetColors,
  type DeviceGroupView,
} from '@/api/deviceOps'
import { listDevices } from '@/api/devices'
import { CHART_PRIMARY } from '@/utils/chartTheme'
import type { DeviceConfig } from '@/api/types'

const route = useRoute()
const groups = ref<DeviceGroupView[]>([])
const { showToast } = useErrorDisplay()
const allDevices = ref<DeviceConfig[]>([])
const showCreate = ref(false)
const isEdit = ref(false)
const editId = ref('')
const showManage = ref(false)
const currentGroup = ref<DeviceGroupView | null>(null)
const selectedDevices = ref<string[]>([])
/** 分组写入中：网关往返期间禁用保存按钮，避免重复提交 */
const saving = ref(false)
/** 设备清单写入中：同上，与分组保存分开，互不影响 */
const savingDevices = ref(false)

const form = reactive({ name: '', description: '', color: CHART_PRIMARY })

// 未保存更改守卫：两个对话框（新建/编辑 + 管理设备）各自一个守卫，共享 scope。
// 弹窗级拦 X/ESC/取消；scope 桥接路由级注册表，弹窗开着跳路由 / 切 Hub tab 仍被拦截。
// （同 scope 注册两次互不影响：注册表按 key 分桶，mark/reset 只动自己的 key）
const createGuard = useDialogDirtyGuard(showCreate, () => form, { scope: route.path })
const manageGuard = useDialogDirtyGuard(showManage, () => selectedDevices.value, { scope: route.path })

async function load() {
  groups.value = await listGroupsView()
  allDevices.value = await listDevices().catch(() => [])
}

function edit(g: DeviceGroupView) {
  isEdit.value = true
  editId.value = g.id
  Object.assign(form, { name: g.name, description: g.description, color: g.color })
  showCreate.value = true
}

function create() {
  isEdit.value = false
  Object.assign(form, { name: '', description: '', color: CHART_PRIMARY })
  showCreate.value = true
}

async function save() {
  if (!form.name.trim()) {
    ElMessage.warning('请输入分组名称')
    return
  }
  saving.value = true
  try {
    if (isEdit.value) {
      await updateGroupView(editId.value, {
        name: form.name,
        description: form.description,
        color: form.color,
        device_ids: groups.value.find((g) => g.id === editId.value)?.deviceIds ?? [],
      })
      ElMessage.success('已更新')
    } else {
      await createGroupView({
        name: form.name,
        description: form.description,
        color: form.color,
        device_ids: [],
      })
      ElMessage.success('已创建')
    }
    showCreate.value = false
    load()
  } catch (e) {
    showToast(e, '保存分组失败')
  } finally {
    saving.value = false
  }
}

async function remove(g: DeviceGroupView) {
  if (!(await confirmDelete(g.name))) return
  try {
    await deleteGroup(g.id)
    ElMessage.success('已删除')
    load()
  } catch (e) {
    showToast(e, '删除分组失败')
  }
}

function manageDevices(g: DeviceGroupView) {
  currentGroup.value = g
  selectedDevices.value = [...g.deviceIds]
  showManage.value = true
}

async function saveDevices() {
  if (!currentGroup.value) return
  const group = currentGroup.value
  // 只提交差异：整体覆盖式 PUT 会把并发期间别人加进分组的设备一起抹掉，
  // 改用后端已有的增量端点（POST/DELETE /api/device-groups/:id/devices）
  const before = new Set(group.deviceIds)
  const after = new Set(selectedDevices.value)
  const added = [...after].filter((id) => !before.has(id))
  const removed = [...before].filter((id) => !after.has(id))
  savingDevices.value = true
  try {
    await Promise.all([
      ...added.map((id) => addDeviceToGroup(group.id, id)),
      ...removed.map((id) => removeDeviceFromGroup(group.id, id)),
    ])
    ElMessage.success('已更新设备列表')
    showManage.value = false
    load()
  } catch (e) {
    showToast(e, '保存分组设备失败')
  } finally {
    savingDevices.value = false
  }
}

onMounted(load)
</script>

<style scoped>
.groups {
  padding: 0px;
}
.top-bar {
  display: flex;
  gap: 8px;
  margin-bottom: 16px;
}
.group-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
  gap: 16px;
}
.group-card {
  border: 2px solid;
  border-radius: var(--radius-card);
  overflow: hidden;
  background: var(--bg-card);
  transition: box-shadow 0.2s;
}
.group-card:hover {
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.1);
}
/* 暗色：纯黑阴影在深底上不可见，改用描边提亮 + 更深的阴影 */
html.dark .group-card:hover {
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
  border-color: var(--border-lighter);
}
.group-card-head {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 12px;
}
.group-color {
  width: 14px;
  height: 14px;
  border-radius: 50%;
  flex-shrink: 0;
}
.group-info {
  flex: 1;
  min-width: 0;
}
.group-name {
  font-size: 0.9375rem;
  font-weight: 600;
  color: var(--text-primary);
}
.group-desc {
  font-size: 0.75rem;
  color: var(--text-secondary);
  margin-top: 2px;
}
.group-card-body {
  padding: 8px 12px;
  min-height: 50px;
}
.device-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}
.device-tag {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.group-card-foot {
  display: flex;
  gap: 8px;
  padding: 8px 12px;
  border-top: 1px solid var(--border-light);
}
.color-picker {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.color-dot {
  width: 24px;
  height: 24px;
  border-radius: 50%;
  cursor: pointer;
  border: 2px solid transparent;
  transition: transform 0.15s;
}
.color-dot.active {
  border-color: var(--text-primary);
  transform: scale(1.15);
}
.tip {
  font-size: 0.8125rem;
  color: var(--text-secondary);
  margin-bottom: 12px;
}
</style>
