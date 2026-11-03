import { h } from 'vue'
import { ElMessage, ElNotification, ElButton } from 'element-plus'
import { confirmDelete, confirmAction } from '@/utils/confirm'
import { deletePoint, restorePoint, updatePoint } from '@/api/points'
import { extractErrorMessage, failureMessage } from '@/utils/errorHandler'
import type { PointConfig } from '@/api/types'
import { clampInterval } from './validation'
import type { BatchDeleteResult, InlinePatch, PointFailure, PointsState } from './types'

/**
 * 点位增删改
 *
 * 含单点软删除 + 8 秒可撤销、批量删除 + 全部撤销、
 * 行内白名单开关（toggleWritable）与行内编辑保存（savePointField）。
 * 删除/撤销均为软删除，写入失败必须在此给出可见提示（全局拦截器只留痕）。
 */
export function createPointCrud(state: PointsState, loadPoints: () => Promise<void>) {
  const { selection } = state

  async function removePoint(row: PointConfig) {
    if (!(await confirmDelete(row.id))) return
    if (state.isUnmounted()) return
    try {
      await deletePoint(row.id)
      if (state.isUnmounted()) return
      ElMessage.success('删除成功')
      // 软删除：成功后给 8 秒可撤销窗口，点击「撤销」直接走恢复接口
      const notice = ElNotification({
        type: 'success',
        duration: 8000,
        title: `已删除点位 ${row.id}`,
        message: h('div', { style: 'display:flex;align-items:center;gap:8px;font-size: 0.75rem' }, [
          h('span', { style: 'color:var(--text-secondary)' }, '可在「已删除点位」中恢复'),
          h(ElButton, {
            size: 'small',
            type: 'primary',
            link: true,
            onClick: async () => {
              if (state.isUnmounted()) return
              try {
                await restorePoint(row.id)
                if (state.isUnmounted()) return
                ElMessage.success('已恢复')
                notice?.close()
                loadPoints()
              } catch {
                // 写操作失败全局静默（拦截器只留痕），这里必须给出可见出口
                ElMessage.error('恢复失败，请到「已删除点位」重试')
              }
            },
          }, () => '撤销'),
        ]),
      })
      loadPoints()
    } catch (e) {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error(failureMessage('删除点位', e))
    }
  }

  /**
   * 批量撤销删除：与删除时同一套并发模型（上限 8），逐条调用恢复接口。
   * 聚合成功/失败数：全部成功只提示数量；部分失败用第二条常驻通知列出仍未恢复的 id，
   * 让用户能精准到「已删除点位」页重试（id 冲突时恢复必然失败）。
   */
  async function restoreBatch(ids: string[], closeNotice: () => void) {
    const queue = [...ids]
    const failed: string[] = []
    let restored = 0
    const CONCURRENCY = 8
    async function worker() {
      while (!state.isUnmounted()) {
        const id = queue.shift()
        if (!id) break
        try {
          await restorePoint(id)
          restored++
        } catch {
          failed.push(id)
        }
      }
    }
    await Promise.all(
      Array.from({ length: Math.min(CONCURRENCY, ids.length) }, () => worker()),
    )
    if (state.isUnmounted()) return
    closeNotice()
    if (failed.length === 0) {
      ElMessage.success(`已恢复 ${restored} 个点位`)
    } else {
      ElNotification({
        type: 'warning',
        title: `撤销删除：${failed.length} 个点位恢复失败`,
        duration: 0,
        message: h('div', [
          h('p', { style: 'margin:0 0 6px;font-size: 0.75rem;color:var(--text-secondary)' },
            `成功 ${restored} 个，失败 ${failed.length} 个。以下点位 id 已被占用或已不存在，请到「已删除点位」重试：`),
          ...failed.slice(0, 8).map((id) =>
            h('div', { style: 'font-size: 0.75rem;line-height:1.7;word-break:break-all' }, id)),
          failed.length > 8
            ? h('div', { style: 'font-size: 0.75rem;color:var(--text-secondary);margin-top:4px' },
              `其余 ${failed.length - 8} 条略，可在「已删除点位」中查看`)
            : null,
        ]),
      })
    }
    loadPoints()
  }

  async function batchDelete(): Promise<BatchDeleteResult | undefined> {
    if (selection.value.length === 0) return
    if (!(await confirmDelete(`选中的 ${selection.value.length} 个点位`, { title: '批量删除' }))) return
    if (state.isUnmounted()) return
    const total = selection.value.length
    const queue = [...selection.value]
    let deleted = 0
    // 记录删除成功的 id：批量撤销只恢复真正删掉的点位
    const deletedIds: string[] = []
    // 逐条收集失败原因：只报「N 个失败」等于没说，用户无法判断该重试哪几条
    const failed: PointFailure[] = []
    const CONCURRENCY = 8
    async function worker() {
      while (!state.isUnmounted()) {
        const p = queue.shift()
        if (!p) break
        try {
          await deletePoint(p.id)
          deleted++
          deletedIds.push(p.id)
        } catch (e) {
          failed.push({ id: p.id, reason: e instanceof Error ? e.message : String(e || '未知错误') })
        }
      }
    }
    await Promise.all(
      Array.from({ length: Math.min(CONCURRENCY, total) }, () => worker()),
    )
    if (state.isUnmounted()) return
    if (failed.length === 0) {
      ElMessage.success(`已删除 ${deleted} 个点位`)
      // 软删除：8 秒内可「全部撤销」，内部并发（上限 8）逐条恢复
      const notice = ElNotification({
        type: 'success',
        duration: 8000,
        title: `已删除 ${deleted} 个点位`,
        message: h('div', { style: 'display:flex;align-items:center;gap:8px;font-size: 0.75rem' }, [
          h('span', { style: 'color:var(--text-secondary)' }, '可在「已删除点位」中恢复'),
          h(ElButton, {
            size: 'small',
            type: 'primary',
            link: true,
            onClick: () => {
              if (state.isUnmounted()) return
              return restoreBatch([...deletedIds], () => notice?.close())
            },
          }, () => '全部撤销'),
        ]),
      })
    } else {
      // 部分失败用常驻通知：列出失败点位与原因，而不是 3 秒后消失的一句话
      ElNotification({
        type: 'warning',
        title: `批量删除：${failed.length} 个点位未删除`,
        duration: 0,
        message: h('div', [
          h('p', { style: 'margin:0 0 6px;font-size: 0.75rem;color:var(--text-secondary)' },
            `成功 ${deleted} 个，失败 ${failed.length} 个。以下点位仍在，可稍后重试：`),
          ...failed.slice(0, 8).map((f) =>
            h('div', { style: 'font-size: 0.75rem;line-height:1.7;word-break:break-all' },
              `${f.id} — ${f.reason}`)),
          failed.length > 8
            ? h('div', { style: 'font-size: 0.75rem;color:var(--text-secondary);margin-top:4px' },
              `其余 ${failed.length - 8} 条略，可在日志中查看`)
            : null,
        ]),
      })
    }
    loadPoints()
    return { deleted, failed }
  }

  /**
   * S11: 点位行上直接切换「允许写入」开关（白名单显式开/关）。
   *
   * 复用整行配置、仅变更 writable，避免其它字段被覆盖；
   * 成功返回 true 供 el-switch 的 before-change 翻转，失败保持原状。
   */
  async function toggleWritable(row: PointConfig): Promise<boolean> {
    if (state.isUnmounted()) return false
    const next = !row.writable
    try {
      // Bug W4: PUT 会无条件覆盖 interval，需回传合法采集周期
      await updatePoint(row.id, { ...row, interval: clampInterval(row.interval), writable: next })
      if (state.isUnmounted()) return false
      row.writable = next
      ElMessage.success(`点位 ${row.id} 已${next ? '开启' : '关闭'}「允许写入」`)
      return true
    } catch (e) {
      // PUT 属于写操作：全局拦截器只留痕不弹窗，必须由这里给出可见提示
      ElMessage.error(`点位 ${row.id} 的写入白名单切换失败（${extractErrorMessage(e, '未知原因')}），开关保持原状`)
      return false
    }
  }

  /**
   * 行内编辑保存：复用整行配置，仅覆盖被改的字段。
   *
   * PUT 会无条件覆盖未传字段，因此必须带全量配置（与 toggleWritable 同一套做法）；
   * 成功后只回写本地行，不重载列表——否则页码复位、勾选清空，正在编辑的位置也会丢。
   *
   * 寻址字段（address / slave_id）改了会直接改变 Modbus 轮询目标，误触后会读错设备，
   * 因此这两个字段在保存前弹二次确认，名称等展示字段则直接保存。
   */
  async function savePointField(row: PointConfig, patch: InlinePatch): Promise<boolean> {
    if (state.isUnmounted()) return false
    const field = Object.keys(patch)[0] ?? ''
    const fieldName = field === 'name' ? '名称' : field === 'address' ? '地址' : '从站号'
    const isAddressing = field === 'address' || field === 'slave_id'
    if (isAddressing) {
      const ok = await confirmAction({
        message: `修改点位「${row.id}」的${fieldName}会改变采集目标地址，可能导致数据错位。确定将${fieldName}从 ${row[field as keyof PointConfig]} 改为 ${String(patch[field as keyof InlinePatch])} 吗？`,
        title: '寻址字段修改确认',
        confirmText: '确认修改',
      })
      if (!ok) return false
    }
    try {
      await updatePoint(row.id, { ...row, interval: clampInterval(row.interval), ...patch })
      if (state.isUnmounted()) return false
      Object.assign(row, patch)
      ElMessage.success(`点位 ${row.id} 的${fieldName}已保存`)
      return true
    } catch (e) {
      // 写操作失败全局静默（仅留痕），必须在这里给出可见提示，否则用户以为改成功了
      ElMessage.error(`点位 ${row.id} 的${fieldName}保存失败（${extractErrorMessage(e, '未知原因')}），已保留原值`)
      return false
    }
  }

  return { removePoint, batchDelete, toggleWritable, savePointField }
}
