<template>
  <div class="discover-wizard">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="设备发现" icon="Search" />
      </template>

      <el-card shadow="never" class="sub-section-card">
        <template #header>
          <SectionHeader title="扫描流程" icon="Compass" />
        </template>

        <el-steps :active="step" finish-status="success" align-center>
          <el-step title="选择总线" />
          <el-step title="设置扫描范围" />
          <el-step title="扫描结果" />
          <el-step title="导入设备" />
        </el-steps>

        <div class="step-content">
          <div v-if="step === 0" class="step-body">
            <!-- N5/P2c: 无总线是流程第一步的死路。原实现只给一个「去新增总线」外链，
                 用户要跨页填 6 个字段再自己走回来。这里直接给出最小三字段的就地新增，
                 建档后立即回到本向导选总线；仍保留跳转入口给需要精调参数的场景。 -->
            <div v-if="!busNames.length" class="quick-bus">
              <empty-state
                description="还没有可用的总线。先用下面三项目快速建一条，其余串口参数按默认值填充"
                :image-size="80"
              />
              <el-form :model="quickBus" label-width="96px" class="quick-form" size="default">
                <el-form-item label="端口">
                  <PortSelect v-model="quickBus.port_name" />
                </el-form-item>
                <el-form-item label="波特率">
                  <BaudrateSelect v-model="quickBus.baud_rate" />
                </el-form-item>
                <el-form-item label="协议">
                  <ProtocolSelector
                    :model-value="quickBus.protocol"
                    @update:model-value="(v: string) => (quickBus.protocol = v)"
                  />
                </el-form-item>
              </el-form>
              <div class="quick-actions">
                <el-button type="primary" :loading="quickAdding" @click="quickAddBus">
                  <el-icon><Plus /></el-icon> 新增总线并继续
                </el-button>
                <el-button plain @click="goAddBus">到运行参数精调</el-button>
              </div>
            </div>
            <DataTable
              v-else
              :data="busRows"
              :columns="busColumns"
              row-key="name"
              stripe
              empty-text="暂无可选总线"
            />
          </div>

          <div v-else-if="step === 1" class="step-body">
            <el-descriptions :column="2" border size="small" class="bus-info">
              <el-descriptions-item label="总线">{{ selectedBus?.name }}</el-descriptions-item>
              <el-descriptions-item label="协议">{{ selectedBus?.protocol }}</el-descriptions-item>
              <el-descriptions-item label="端口">{{ selectedBus?.port_name }}</el-descriptions-item>
              <el-descriptions-item label="波特率">{{ selectedBus?.baud_rate }}</el-descriptions-item>
            </el-descriptions>

            <el-form :model="scanForm" label-width="120px" class="scan-form" size="default">
              <el-form-item label="起始从站地址">
                <el-input-number v-model="scanForm.start" :min="1" :max="247" :step="1" />
              </el-form-item>
              <el-form-item label="结束从站地址">
                <el-input-number v-model="scanForm.end" :min="scanForm.start" :max="247" :step="1" />
              </el-form-item>
              <el-form-item label="扫描数量">
                <el-tag effect="plain" round>{{ scanCount }}</el-tag>
                <span class="hint-text"> 个从站地址将被扫描</span>
              </el-form-item>
            </el-form>

            <div class="scan-actions">
              <el-button type="primary" :loading="scanning" :disabled="scanCount === 0" @click="doScan">
                <el-icon><Search /></el-icon> 开始扫描
              </el-button>
            </div>
          </div>

          <div v-else-if="step === 2" class="step-body">
            <el-alert
              :title="`扫描完成：共扫描 ${scanResult.scanned} 个地址，发现 ${foundDevices.length} 台设备`"
              :type="foundDevices.length > 0 ? 'success' : 'warning'"
              :closable="false"
              show-icon
              style="margin-bottom: 12px"
            />
            <DataTable
              v-if="foundDevices.length > 0"
              :data="foundRows"
              :columns="foundColumns"
              row-key="uid"
              stripe
              empty-text="暂无发现结果"
              :selectable="true"
              @selection-change="onSelectionChange"
            />
            <empty-state v-else description="未发现任何设备，请检查总线连接或扫描范围" :image-size="80" />
          </div>

          <div v-else-if="step === 3" class="step-body">
            <el-result
              :icon="importDone ? 'success' : 'info'"
              :title="importDone ? `成功导入 ${importedCount} 台设备` : '确认导入选中的设备'"
            >
              <template #sub-title v-if="!importDone">
                将导入 {{ selectedDevices.length }} 台设备到配置中
              </template>
              <template #extra>
                <el-button v-if="!importDone" type="primary" :loading="importing" @click="doImport">
                  <el-icon><Check /></el-icon> 确认导入
                </el-button>
                <template v-else>
                  <el-button type="primary" @click="reset">重新扫描</el-button>
                  <el-button @click="goDevices">前往设备管理</el-button>
                </template>
              </template>
            </el-result>
          </div>
        </div>

        <div class="step-actions">
          <el-button v-if="step > 0 && step < 3" @click="step--">上一步</el-button>
          <el-button v-if="step === 2 && selectedDevices.length > 0" type="primary" @click="step++">下一步</el-button>
        </div>
      </el-card>
    </el-card>
  </div>
</template>


<script setup lang="ts">
import { computed, h } from 'vue'
import { useRouter } from 'vue-router'
import { ElButton, ElInput } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn, protocolColumn } from '@/utils/commonColumns'
import type { BusConfig } from '@/api/types'
import { useDiscoverWizard } from '@/composables/devices/useDiscoverWizard'
import PortSelect from '@/components/industrial/PortSelect.vue'
import BaudrateSelect from '@/components/industrial/BaudrateSelect.vue'
import ProtocolSelector from '@/components/industrial/ProtocolSelector.vue'

const router = useRouter()
/** N5: 无总线空态直达「运行参数 → 总线」页签（Hub 页签必须显式带 tab，否则不会挂载） */
function goAddBus() {
  router.push('/system/config?tab=runtime-config')
}

const {
  step, scanning, importing, importDone, importedCount,
  busTable, selectedBus, scanForm, scanResult,
  foundDevices, selectedDevices, busNames,
  quickAdding, quickBus,
  scanCount,
  onSelectionChange, selectBus, doScan, doImport, reset, goDevices, quickAddBus,
} = useDiscoverWizard()

/** 总线行：在 BusConfig 上附加 name（= port_name），与 composable 的 busTable 形状一致 */
type BusRow = { name: string } & BusConfig
const busRows = computed(() => busTable.value as BusRow[])

/** 发现设备行：与 composable 内 FoundDevice 结构一致 */
type FoundDeviceRow = {
  uid: string
  slave_id: number
  protocol: string
  name: string
  id: string
  bus: string
}
const foundRows = computed(() => foundDevices.value as FoundDeviceRow[])

// DataTable 列定义：宽度/顺序与原列保持一致
const busColumns: ColumnDef<BusRow>[] = [
  { key: 'name', title: '总线名称', minWidth: 120, ellipsis: true },
  protocolColumn<BusRow>({ width: 100, useLabel: false }),
  { key: 'port_name', title: '端口', minWidth: 120, ellipsis: true },
  { key: 'baud_rate', title: '波特率', width: 90 },
  { key: 'parity', title: '校验', width: 70 },
  actionColumn<BusRow>({
    key: 'action',
    width: 100,
    cell: (row) => h(ElButton, { size: 'small', type: 'primary', disabled: scanning.value, onClick: () => selectBus(row) }, () => '选择'),
  }),
]

const foundColumns: ColumnDef<FoundDeviceRow>[] = [
  { key: 'slave_id', title: '从站地址', width: 100 },
  {
    key: 'name',
    title: '设备名称',
    minWidth: 160,
    cell: (row) =>
      h(ElInput, {
        modelValue: row.name,
        size: 'small',
        placeholder: '输入设备名称',
        'onUpdate:modelValue': (v: string) => { row.name = v },
      }),
  },
  protocolColumn<FoundDeviceRow>({ width: 90, useLabel: false }),
  {
    key: 'id',
    title: '设备ID',
    minWidth: 140,
    cell: (row) =>
      h(ElInput, {
        modelValue: row.id,
        size: 'small',
        placeholder: '设备ID（留空自动生成）',
        'onUpdate:modelValue': (v: string) => { row.id = v },
      }),
  },
]
</script>

<style scoped>
.discover-wizard { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.step-body { padding: 24px 0; min-height: 200px; }
.bus-info { margin-bottom: 20px; }
.scan-form { max-width: 500px; margin: 0 auto; }
.scan-form :deep(.el-input-number) { width: 180px; }
.hint-text { color: var(--text-secondary); font-size: 0.8125rem; margin-left: 8px; }
.scan-actions { text-align: center; margin-top: 20px; }
.step-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; }

/* P2c: 向导内就地新增总线 */
.quick-bus { display: flex; flex-direction: column; align-items: center; }
.quick-form { width: 100%; max-width: 460px; margin-top: 8px; }
.quick-actions { display: flex; gap: 8px; justify-content: center; margin-top: 4px; }
</style>