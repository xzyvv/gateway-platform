import { computed, getCurrentInstance, onUnmounted, ref, watch, type Ref } from 'vue'
import { ElMessageBox } from 'element-plus'
import { registerDirtyGuard } from '../../utils/dirtyGuard'

/**
 * 弹窗级未保存更改守卫
 *
 * 背景：路由级 dirtyGuard（utils/dirtyGuard.ts）只拦「路由离开 / Hub tab 切换」，
 * 表单弹窗内改了字段后直接点 X / 取消 / ESC 仍会静默丢失修改。
 * 本 composable 用「打开时取基线快照，关闭前对比」的方式补上弹窗这一层的确认。
 *
 * 用法（三处接线）：
 *   const guard = useDialogDirtyGuard(show, () => form, { scope: route.path })
 *   <el-dialog v-model="show" :before-close="guard.beforeClose">   ← X / ESC / 遮罩
 *   <el-button @click="guard.close">取消</el-button>               ← 取消按钮
 *
 * 注意：
 * - 保存成功路径直接 `show.value = false`，v-model 关闭不经过 before-close，
 *   因此不会误弹确认。
 * - 基线在弹窗打开渲染完成后（flush: 'post'）采集，@open 里的同步表单填充/重置
 *   已完成；若表单在打开后异步填充，请在填充完成后调用 markClean() 重取基线。
 * - 传了 scope 时脏状态会同步进路由级注册表（脏 → mark，干净/关闭 → reset），
 *   弹窗开着跳路由、切 Hub tab 仍会被 router.beforeEach / HubShell 拦截。
 *   两层守卫互补：弹窗级拦「关弹窗」，路由级拦「弹窗开着离开页面」。
 */
export interface DialogDirtyGuard {
  /** el-dialog 的 :before-close 处理器：有未保存修改时先弹确认 */
  beforeClose(done: () => void): void
  /** 供「取消」按钮调用：有未保存修改时先确认，通过后关闭弹窗 */
  close(): Promise<void>
  /** 有未保存修改时弹确认；返回 true = 用户同意放弃（可关闭） */
  confirmClose(): Promise<boolean>
  /** 当前是否存在未保存修改（弹窗打开期间快照与基线不一致） */
  isDirty(): boolean
  /** 重取基线（表单异步填充完成后调用） */
  markClean(): void
}

export interface DialogDirtyGuardOptions {
  /** 路由级桥接 scope（通常 route.path）。不传则只有弹窗级确认，不注册到全局脏表。 */
  scope?: string
}

export function useDialogDirtyGuard(
  visible: Ref<boolean>,
  source: () => unknown,
  options: DialogDirtyGuardOptions = {},
): DialogDirtyGuard {
  const baseline = ref('')
  const snapshot = () => JSON.stringify(source())

  // 打开弹窗后取基线：post flush 保证 @open 中的同步填充/重置已经生效
  watch(visible, (open) => {
    if (open) baseline.value = snapshot()
  }, { flush: 'post' })

  // 脏状态做成 computed：snapshot 遍历会跟踪 source() 返回的响应式对象的全部属性；
  // baseline 是 ref，重取基线（markClean / 确认放弃）也能触发重算
  const dirty = computed(() => visible.value && snapshot() !== baseline.value)

  function isDirty(): boolean {
    return dirty.value
  }

  // scope 桥接：弹窗脏状态同步到路由级注册表。
  // sync flush 保证字段一改注册表立刻有脏标记——用户马上点路由跳转也能被 beforeEach 拦到；
  // 弹窗关闭时 dirty 自然变 false，等价于旧手写模板的「关闭即清脏」。
  if (options.scope !== undefined) {
    const routeGuard = registerDirtyGuard(options.scope)
    watch(dirty, (d) => {
      if (d) routeGuard.mark()
      else routeGuard.reset()
    }, { flush: 'sync' })
    // 组件外（如纯单测）无实例可挂，跳过注册避免 Vue 告警
    if (getCurrentInstance()) onUnmounted(() => routeGuard.unregister())
  }

  /** 有未保存修改时弹确认；返回 true = 用户同意放弃，可以关闭 */
  async function confirmClose(): Promise<boolean> {
    if (!isDirty()) return true
    try {
      await ElMessageBox.confirm('当前修改尚未保存，关闭后将丢失。', '未保存的修改', {
        type: 'warning',
        confirmButtonText: '放弃修改并关闭',
        cancelButtonText: '继续编辑',
        autofocus: false,
      })
      // 用户已同意放弃：把当前状态作为新基线，弹窗继续打开期间再次关闭不再重复弹
      baseline.value = snapshot()
      return true
    } catch {
      return false
    }
  }

  async function beforeClose(done: () => void) {
    if (await confirmClose()) done()
  }

  async function close() {
    if (await confirmClose()) visible.value = false
  }

  function markClean() {
    if (visible.value) baseline.value = snapshot()
  }

  return { beforeClose, close, confirmClose, isDirty, markClean }
}
