<template>
  <div class="apikeys">
    <!-- W94: 遵循 VISUAL_SPEC 第8.2 一级标题使用 card-header + section-header::before 装饰条 -->
    <SectionHeader title="API 密钥" icon="Key">
      <el-button type="primary" size="small" @click="showCreate = true">
        <el-icon><Plus /></el-icon> 新建 API Key
      </el-button>
      <el-button :loading="loading" size="small" @click="load">
        <el-icon><Refresh /></el-icon> 刷新
      </el-button>
    </SectionHeader>

    <DataTable
      :data="keys"
      :columns="keyColumns"
      row-key="id"
      :loading="loading"
      stripe
      empty-text="暂无 API Key，新建后可对外签发调用凭证"
      empty-action-text="新建 API Key"
      @empty-action="showCreate = true"
    />

    <!-- 新建 API Key -->
    <el-dialog
      :close-on-click-modal="false"
      v-model="showCreate"
      title="新建 API Key"
      width="480px"
      :before-close="createGuard.beforeClose"
      @close="resetForm"
    >
      <el-form ref="keyFormRef" :model="form" :rules="keyRules" label-width="100px" scroll-to-error>
        <el-form-item label="名称" prop="name"
          ><el-input v-model="form.name" placeholder="为该 API Key 起个名字"
        /></el-form-item>
        <el-form-item label="权限范围" prop="scopes">
          <el-checkbox-group v-model="form.scopes">
            <el-checkbox v-for="s in allScopes" :key="s" :value="s">{{ s }}</el-checkbox>
          </el-checkbox-group>
        </el-form-item>
        <el-form-item label="有效期(天)" prop="expiresDays">
          <el-input-number v-model="form.expiresDays" :min="0" controls-position="right" />
          <span style="margin-left: 8px; color: var(--text-secondary); font-size: 0.75rem"
            >0 = 永不过期</span
          >
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createGuard.close">取消</el-button>
        <el-button type="primary" :loading="creating" :disabled="creating" @click="create">创建</el-button>
      </template>
    </el-dialog>

    <!-- 创建成功：完整 Key 仅此一次展示 -->
    <el-dialog
      v-model="showCreated"
      title="API Key 已创建"
      width="560px"
      :close-on-click-modal="false"
    >
      <el-alert
        type="warning"
        :closable="false"
        show-icon
        title="请立即复制并妥善保存"
        description="完整 Key 仅在此时展示一次，关闭后将无法再次查看（网关只保存摘要）。"
      />
      <div class="created-key">
        <code>{{ createdKey }}</code>
        <el-button size="small" @click="copyKey(createdKey)">
          <el-icon><CopyDocument /></el-icon> 复制
        </el-button>
      </div>
      <template #footer>
        <el-button type="primary" @click="showCreated = false">我已保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted, h, getCurrentInstance } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElButton, ElIcon, ElMessage, ElSwitch, ElTag, ElTooltip, type FormInstance, type FormRules } from 'element-plus'
import { useRoute } from 'vue-router'
import { confirmDelete } from '@/utils/confirm'
import { Plus, Refresh, CopyDocument } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import {
  listApiKeys,
  createApiKey,
  setApiKeyEnabled,
  deleteApiKey,
  getAllScopes,
  type ApiKey,
} from '@/api/apiKeys'
import { formatDate, formatDateTime } from '@/utils/datetime'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'

const keys = ref<ApiKey[]>([])
const loading = ref(false)
const showCreate = ref(false)
const creating = ref(false)
const showCreated = ref(false)
const createdKey = ref('')
const allScopes = getAllScopes()
const keyFormRef = ref<FormInstance>()

// W96: 默认 scopes 从 allScopes[0] 取，确保权限范围有默认值
const form = reactive({
  name: '',
  scopes: [allScopes[0]] as string[],
  expiresDays: 0,
})

// 字段级校验规则：错误落在对应输入框下方，现场人员一眼看到该改哪一栏
const keyRules: FormRules = {
  name: [
    { required: true, message: '请输入名称', trigger: 'blur' },
    { max: 64, message: '名称不能超过 64 个字符', trigger: 'blur' },
  ],
  scopes: [
    { type: 'array', required: true, min: 1, message: '请至少勾选一个权限范围', trigger: 'change' },
  ],
  expiresDays: [
    { type: 'number', min: 0, message: '有效期不能为负数（0 表示永不过期）', trigger: 'blur' },
  ],
}

// 新建 API Key 弹窗未保存修改守卫：X/ESC/取消时若有改动先确认（创建成功结果弹窗为只读展示，不接）
// 路由级脏标记桥接：只有挂了 router 的运行环境才取路由，取不到就只启用弹窗级守卫
const dirtyScope = getCurrentInstance()?.appContext.config.globalProperties.$router ? useRoute().path : undefined
const createGuard = useDialogDirtyGuard(showCreate, () => form, dirtyScope ? { scope: dirtyScope } : {})

// 多语句内联处理器在 vitest 的独立模板编译下抛 SyntaxError，抽成方法最稳；
// 顺带统一默认值（原 @close 重置为 ['points:read']，与初始值 allScopes[0] 不一致）
function resetForm() {
  form.name = ''
  form.scopes = [allScopes[0]!]
  form.expiresDays = 0
  // 重置后清掉上一次的字段级错误，避免重新打开弹窗还挂着红色提示
  keyFormRef.value?.clearValidate()
}

async function copyKey(key: string) {
  // W95: clipboard API 在 HTTP 非 HTTPS 环境下可能不可用
  // 此时 navigator.clipboard 为 undefined，原 fallback 可能失效
  // 改为 try/catch + 提示用户手动复制
  if (!navigator.clipboard) {
    ElMessage.warning('当前环境不支持自动复制，请手动复制 Key')
    return
  }
  try {
    await navigator.clipboard.writeText(key)
    ElMessage.success('已复制')
  } catch {
    ElMessage.warning('复制失败，请手动复制')
  }
}
function isExpiring(ts: number) {
  return ts > Date.now() && ts - Date.now() < 86400000 * 30
}

/** 列定义：Key 列只展示后端返回的预览（前 8 后 4），完整凭证网关不留存、无法再查看 */
const keyColumns: ColumnDef<ApiKey>[] = [
  { key: 'name', title: '名称', width: 160, ellipsis: true },
  {
    key: 'keyPreview',
    title: 'Key',
    minWidth: 220,
    cell: (row) => h('code', { class: 'key' }, row.keyPreview),
  },
  {
    key: 'scopes',
    title: '权限范围',
    minWidth: 200,
    cell: (row) =>
      h(
        'span',
        row.scopes.map((s) =>
          h(ElTag, { key: s, size: 'small', effect: 'plain', style: { margin: '1px' } }, () => s),
        ),
      ),
  },
  { key: 'callCount', title: '调用次数', width: 110, align: 'right' },
  {
    key: 'lastUsedAt',
    title: '上次使用',
    width: 170,
    cell: (row) => (row.lastUsedAt ? h('span', formatDateTime(row.lastUsedAt)) : h('span', '从未')),
  },
  {
    key: 'expiresAt',
    title: '过期时间',
    width: 170,
    cell: (row) =>
      row.expiresAt
        ? h(
            'span',
            {
              class: { expiring: isExpiring(row.expiresAt), expired: row.expiresAt < Date.now() },
            },
            formatDate(row.expiresAt),
          )
        : h('span', '永不过期'),
  },
  {
    key: 'enabled',
    title: '状态',
    width: 80,
    cell: (row) =>
      h(ElTooltip, { content: row.enabled ? '点击停用' : '已停用', placement: 'top' }, () =>
        h(ElSwitch, {
          modelValue: row.enabled,
          'onUpdate:modelValue': (v: unknown) => {
            row.enabled = Boolean(v)
          },
          onChange: () => {
            void onToggle(row)
          },
        }),
      ),
  },
  actionColumn<ApiKey>({
    width: 80,
    fixed: 'right',
    cell: (row) =>
      h(
        ElButton,
        {
          size: 'small',
          type: 'danger',
          link: true,
          onClick: () => {
            void remove(row)
          },
        },
        () => '删除',
      ),
  }),
]

async function load() {
  loading.value = true
  try {
    keys.value = await listApiKeys()
  } finally {
    loading.value = false
  }
}

async function create() {
  if (creating.value) return
  const formEl = keyFormRef.value
  // 字段级校验：错误已落在对应输入框下方（scroll-to-error 自动定位首个错误项）
  if (formEl) {
    const ok = await formEl.validate().catch(() => false)
    if (!ok) {
      // 名称为空是最高频错误，额外给一条即时提示（字段下方已有规则错误文案）
      if (!form.name.trim()) ElMessage.warning('请输入名称')
      return
    }
  }
  creating.value = true
  try {
    const k = await createApiKey(form.name, form.scopes, form.expiresDays || undefined)
    // 完整 Key 仅此一次返回：弹出专属对话框引导复制，不用 toast（易错过、不可再查）
    createdKey.value = k.key
    showCreate.value = false
    showCreated.value = true
    form.name = ''
    form.scopes = [allScopes[0]!]
    form.expiresDays = 0
    load()
  } catch (e) {
    showToast(e, '创建失败')
  } finally {
    creating.value = false
  }
}

async function onToggle(k: ApiKey) {
  try {
    await setApiKeyEnabled(k.id, k.enabled)
  } catch {
    ElMessage.error('更新失败')
    k.enabled = !k.enabled
  }
}

async function remove(k: ApiKey) {
  if (!(await confirmDelete(k.name))) return
  try {
    await deleteApiKey(k.id)
    ElMessage.success('已删除')
    load()
  } catch {
    // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
    ElMessage.error('删除 API Key 失败，请稍后重试')
  }
}

onMounted(load)
</script>

<style scoped>
.apikeys {
  padding: 0px;
}
.key {
  font-family: monospace;
  font-size: 0.75rem;
  background: var(--bg-fill);
  padding: 2px 6px;
  border-radius: var(--radius-small);
}
.created-key {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
}
.created-key code {
  flex: 1;
  font-family: monospace;
  font-size: 0.8rem;
  background: var(--bg-fill);
  padding: 8px;
  border-radius: var(--radius-small);
  word-break: break-all;
}
.expiring {
  color: var(--text-warning);
}
.expired {
  color: var(--text-danger);
  text-decoration: line-through;
}
</style>
