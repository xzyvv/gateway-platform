<template>
  <div class="addr-map">
    <div class="control-bar">
      <el-select v-model="selectedBus" placeholder="选择总线" aria-label="选择总线" style="width: 200px" @change="loadMap">
        <el-option v-for="b in buses" :key="b" :label="b" :value="b" />
      </el-select>
      <el-select v-model="selectedSlave" placeholder="选择从站" aria-label="选择从站" style="width: 140px" @change="loadMap">
        <el-option v-for="s in slaves" :key="s" :label="`从站 ${s}`" :value="s" />
      </el-select>
      <el-select v-model="regType" aria-label="寄存器类型" style="width: 160px">
        <el-option label="保持寄存器 (Holding)" value="holding" />
        <el-option label="输入寄存器 (Input)" value="input" />
        <el-option label="线圈 (Coils)" value="coils" />
        <el-option label="离散输入 (Discrete)" value="discrete" />
      </el-select>
      <el-button @click="loadMap"><el-icon><Refresh /></el-icon> 刷新</el-button>
    </div>

    <!-- 配置读取失败必须显式提示：否则页面会退化成「一整片空闲地址」，
         现场照着空位建点，点位落到已被占用的地址上 -->
    <el-alert v-if="loadError" :title="loadError" type="error" show-icon :closable="false" style="margin-bottom:12px" />

    <!-- 配置里一个点位都没有时给空态，不要画满屏空闲：
         满屏「空闲」看起来和「已核对过、确实没有点位」一模一样，
         现场会照着空位建点，实际点位却配在设备上 -->
    <EmptyState
      v-else-if="!points.length"
      description="暂无点位数据：请先在配置管理中添加设备与点位，再回来核对地址占用"
      :image-size="72"
    />

    <template v-else>
      <div class="map-container">
        <div class="address-cell"
          v-for="cell in cells"
          :key="cell.addr"
          :class="{ used: cell.used, free: !cell.used, selected: cell.addr === hoverAddr }"
          @mouseenter="hoverAddr = cell.addr"
          @mouseleave="hoverAddr = null"
          role="button"
          tabindex="0"
          @click="cell.used && showCell(cell)"
          @keydown.enter.prevent="cell.used && showCell(cell)"
          @keydown.space.prevent="cell.used && showCell(cell)"
        >
          <div class="addr-num">{{ cell.addr }}</div>
          <div class="addr-label" v-if="cell.used">{{ cell.pointName }}</div>
        </div>
      </div>

      <div class="legend">
        <span><i class="dot used-dot"></i>已占用</span>
        <span><i class="dot free-dot"></i>空闲</span>
      </div>

      <!-- 占用详情 -->
      <el-drawer v-model="showDetail" title="地址占用详情" size="40%">
        <el-descriptions v-if="hoverCell" :column="1" border>
          <el-descriptions-item label="地址">{{ hoverCell.addr }}</el-descriptions-item>
          <el-descriptions-item label="点位 ID">{{ hoverCell.pointId || '无' }}</el-descriptions-item>
          <el-descriptions-item label="点位名称">{{ hoverCell.pointName || '未分配' }}</el-descriptions-item>
          <el-descriptions-item label="占用寄存器数">{{ hoverCell.addressNum || 1 }}</el-descriptions-item>
        </el-descriptions>
      </el-drawer>
    </template>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import {Refresh} from '@element-plus/icons-vue'
import EmptyState from '@/components/EmptyState.vue'
import { getConfig } from '@/api/config'
import { extractErrorMessage } from '@/utils/errorHandler'
import type { PointConfig } from '@/api/types'

const buses = ref<string[]>([])
const selectedBus = ref('')
const slaves = ref<number[]>([])
const selectedSlave = ref<number | null>(null)
const regType = ref<'holding' | 'input' | 'coils' | 'discrete'>('holding')
const points = ref<PointConfig[]>([])
const hoverAddr = ref<number | null>(null)
const showDetail = ref(false)
/** 配置读取失败原因：必须展示，不能让页面退化成「全空闲地址」误导建点 */
const loadError = ref('')

/** 寄存器类型 → Modbus 功能码。点位未填功能码时按 3（保持寄存器）兜底，与配置向导默认值一致 */
const FC_BY_REG_TYPE: Record<typeof regType.value, number> = { holding: 3, input: 4, coils: 1, discrete: 2 }

// 地址范围不再写死 0-99：至少显示 100 格；有点位占用超出时按 10 的倍数自适应扩展，
// 否则高地址点位在图上不可见，现场会误判地址空闲
const MIN_ADDR_RANGE = 100

interface AddrCell { addr: number; used: boolean; pointId?: string; pointName?: string; addressNum?: number }

const cells = computed<AddrCell[]>(() => {
  const map = new Map<number, { pointId: string; pointName: string; addressNum: number }>()
  let maxAddr = -1
  for (const p of points.value) {
    if (p.bus !== selectedBus.value) continue
    if (selectedSlave.value != null && p.slave_id !== selectedSlave.value) continue
    // 地址映射必须按功能码分区显示：保持寄存器与线圈地址段是两套编址，
    // 混在一起会让现场以为「D100 已被线圈占用」，实际两者互不相干
    if ((p.function_code ?? 3) !== FC_BY_REG_TYPE[regType.value]) continue
    const addr = parseInt(p.address, 10)
    if (isNaN(addr)) continue
    const num = p.address_num || 1
    for (let i = 0; i < num; i++) {
      map.set(addr + i, { pointId: p.id, pointName: p.name || p.id, addressNum: num })
    }
    maxAddr = Math.max(maxAddr, addr + num - 1)
  }
  const range = Math.max(MIN_ADDR_RANGE, Math.ceil((maxAddr + 1) / 10) * 10)
  const arr: AddrCell[] = []
  for (let i = 0; i < range; i++) {
    const occ = map.get(i)
    arr.push({ addr: i, used: !!occ, ...occ })
  }
  return arr
})

const hoverCell = computed(() => {
  if (hoverAddr.value == null) return null
  return cells.value.find((c) => c.addr === hoverAddr.value) || null
})

function showCell(cell: AddrCell) {
  hoverAddr.value = cell.addr
  showDetail.value = true
}

async function loadMap() {
  loadError.value = ''
  try {
    const cfg = await getConfig()
    points.value = cfg.points || []
    const busSet = new Set(points.value.map((p) => p.bus))
    buses.value = [...busSet]
    if (!selectedBus.value && buses.value.length) selectedBus.value = buses.value[0]!
    const slaveSet = new Set(points.value.filter((p) => p.bus === selectedBus.value).map((p) => p.slave_id).filter((s): s is number => s != null))
    slaves.value = [...slaveSet].sort((a, b) => a - b)
  } catch (e) {
    // 清空绘制底图并显式告知失败：既不能用过期占用误导，也不能放任「全空闲」让人照空位建点
    points.value = []
    loadError.value = `配置读取失败：${extractErrorMessage(e)}`
  }
}

onMounted(loadMap)
</script>

<style scoped>
.addr-map { padding: 0px; }
.control-bar { display: flex; gap: 8px; align-items: center; margin-bottom: 16px; flex-wrap: wrap; }

.map-container {
  display: grid;
  grid-template-columns: repeat(10, 1fr);
  gap: 4px;
  background: var(--bg-card);
  border: 1px solid var(--border-light);
  border-radius: var(--radius-card);
  padding: 12px;
}
.address-cell {
  aspect-ratio: 1;
  border: 1px solid var(--border-light);
  border-radius: var(--radius-small);
  display: flex; flex-direction: column; align-items: center; justify-content: center;
  font-size: 0.625rem; cursor: pointer; transition: background-color, border-color, color, transform, box-shadow 0.15s;
  background: var(--bg-fill);
}
.address-cell.free { background: var(--bg-fill); color: var(--text-secondary); }
.address-cell.used { background: var(--color-primary-bg); border-color: var(--color-primary); color: var(--color-primary); }
.address-cell.selected { transform: scale(1.05); box-shadow: 0 2px 8px rgba(var(--color-primary-rgb), .3); }
.addr-num { font-weight: 600; }
.addr-label { font-size: 0.5625rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 90%; }

.legend { display: flex; gap: 20px; margin-top: 12px; font-size: 0.75rem; color: var(--text-secondary); align-items: center; }
.dot { display: inline-block; width: 12px; height: 12px; border-radius: var(--radius-bar); margin-right: 4px; vertical-align: middle; }
.used-dot { background: var(--color-primary-bg); border: 1px solid var(--color-primary); }
.free-dot { background: var(--bg-fill); border: 1px solid var(--border-light); }
</style>
