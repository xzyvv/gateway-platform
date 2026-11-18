<template>
  <el-dialog v-model="showForm" :title="formMode === 'create' ? '新增点位' : '编辑点位'" width="620px" :before-close="formGuard.beforeClose" @close="resetForm">
    <el-form ref="pointFormRef" :model="pointForm" :rules="pointRules" label-width="90px">
      <el-form-item label="点位 ID" prop="id">
        <el-input v-model="pointForm.id" :disabled="formMode === 'edit'" placeholder="如: temp_01" />
      </el-form-item>
      <el-form-item label="点位名称" prop="name">
        <el-input v-model="pointForm.name" placeholder="如: 温度传感器" />
      </el-form-item>
      <el-form-item label="总线" prop="bus">
        <!-- N3: 总线必须继承所属设备。原自由文本可填任意总线名，后端按 bus 找不到采集器，
             该点位永远采不到数据且界面无任何提示。 -->
        <el-input :model-value="pointForm.bus" disabled placeholder="继承自所属设备" />
        <div class="point-inherit-hint">继承自设备「{{ currentDeviceName }}」，如需更换请到设备列表修改总线。</div>
      </el-form-item>
      <el-form-item label="协议" prop="protocol">
        <!-- N3: 协议同样继承自设备（设备协议又由模板决定），避免点位按错误协议解析报文。 -->
        <el-input :model-value="pointForm.protocol ? (protocolLabel[pointForm.protocol] || pointForm.protocol) : ''"
          disabled placeholder="继承自所属设备" />
        <div class="point-inherit-hint">协议继承自设备，不可单独修改。</div>
      </el-form-item>
      <el-form-item label="点位地址" prop="address">
        <!-- 地址格式随协议不同（Modbus 40001 / S7 DB1.DBD0 / BACnet analog-input:1），
             提示文本统一取后端协议描述符，避免前端只提示 Modbus 写法。 -->
        <!-- Modbus 实际按「功能码 + address_num」寻址，地址串只是 5 位展示；
             用区段输入框直接联动两者，避免用户改了地址却不生效。 -->
        <ModbusAddressInput v-if="isModbus && !modbusAddrOverflow" v-model="modbusAddr" />
        <el-input v-else v-model="pointForm.address" :placeholder="protocolAddressPlaceholder(pointForm.protocol)" />
        <div v-if="isModbus && !modbusAddrOverflow" class="point-inherit-hint">
          区段首位即功能码数字（0xxxx→FC01、1xxxx→FC02、3xxxx→FC03、4xxxx→FC04），1xxxx / 4xxxx 为只读区段。
          0xxxx 从 00001 起算，00000 与 00001 等价（都指向 0 号寄存器）。
          注意这与部分厂商手册的写法可能相反：手册里标成「保持寄存器 40001」的地址，本系统要填 30001，
          填 40001 会按 FC04 去读输入寄存器。
        </div>
        <div v-else-if="isModbus" class="point-inherit-hint">
          地址偏移超过 9998，5 位地址装不下，请直接填写下方「地址偏移」与展示地址。
        </div>
      </el-form-item>
      <el-form-item label="从站地址" prop="slave_id">
        <el-input-number v-model="pointForm.slave_id" :min="1" :max="247" controls-position="right" style="width: 100%" />
      </el-form-item>
      <!-- address_num 只有 Modbus 有意义（实际寻址 = 功能码 + 0 基寄存器偏移），
           其他协议按地址串寻址，暴露该输入只会误导 -->
      <el-form-item v-if="isModbus && modbusAddrOverflow" label="地址偏移" prop="address_num">
        <el-input-number v-model="pointForm.address_num" :min="0" controls-position="right" style="width: 100%" />
        <div class="point-inherit-hint">0 基寄存器偏移（读寄存器 1 时填 0）。</div>
      </el-form-item>
      <!-- 数值解码参数：modbus/mc/fins/dlt645/cjt188 共用同一套 data_type + byte_order，
           后端 PUT 会无条件覆盖这些字段，不展示就没法修正、编辑还会被清空。
           寄存器数只有 Modbus 有意义（单次读取上限 125），其余协议由类型/数据标识决定长度。 -->
      <el-form-item v-if="numericDecode" label="数据类型">
        <div class="point-pair">
          <el-select v-model="pointForm.data_type" filterable allow-create default-first-option clearable placeholder="默认 u16" style="width: 100%">
            <el-option v-for="t in DATA_TYPES" :key="t" :label="t" :value="t" />
          </el-select>
          <el-input-number v-if="isModbus" v-model="pointForm.register_count" :min="1" :max="125" controls-position="right" placeholder="寄存器数，留空自动" style="width: 100%" />
        </div>
        <div class="point-inherit-hint">{{ dataTypeHint }}</div>
      </el-form-item>
      <el-form-item v-if="numericDecode" label="字节序">
        <el-select v-model="pointForm.byte_order" clearable placeholder="默认 ABCD（大端）" style="width: 100%">
          <el-option v-for="b in BYTE_ORDERS" :key="b.value" :label="b.label" :value="b.value" />
        </el-select>
        <div class="point-inherit-hint">只对多字节类型（f32 / u32 等）有意义：留空按 ABCD（大端）解码，与设备手册的字节序一致即可；填错会导致数值乱码或明显偏离。</div>
      </el-form-item>
      <el-form-item label="采集周期" prop="interval">
        <!-- 上界跟随北向上报周期；已有大周期点位保留原值（编辑不应静默改频率） -->
        <DurationSelect
          v-model="pointForm.interval"
          :min="1"
          :max="Math.max(intervalMax, pointForm.interval)"
          :units="intervalMax >= 120 ? ['s', 'min'] : ['s']"
          num-width="100%"
          unit-width="82px"
        />
        <div class="point-inherit-hint">必须小于北向上报周期（当前 {{ northInterval }} 秒）；上报周期在「系统配置 → MQTT」中修改。</div>
      </el-form-item>
      <el-form-item label="允许写入" prop="writable">
        <el-switch v-model="pointForm.writable" />
        <span style="margin-left: 8px; color: var(--text-secondary); font-size: 0.75rem">开启后该点位可在「点位读写」页面写入，也可通过「远程控制」下发；关闭（默认）则拒绝一切写入</span>
      </el-form-item>

      <!-- BUG-B: 高级参数。这些字段后端一直支持，但表单此前不收集，
           用户填的告警/死区/聚合会被后端静默丢弃。 -->
      <el-divider content-position="left">量程与告警</el-divider>
      <el-form-item label="单位" prop="unit">
        <!-- 纯下拉、禁止自定义：避免「℃ / C」「kWh / KWH」这类同义写法导致上送端分组混乱 -->
        <el-select
          v-model="pointForm.unit"
          filterable
          clearable
          placeholder="选择单位（可留空）"
          style="width: 100%"
        >
          <el-option v-for="u in unitChoices(pointForm.unit)" :key="u" :label="u" :value="u" />
        </el-select>
      </el-form-item>
      <el-form-item label="倍率 / 偏移">
        <div class="point-pair">
          <el-input-number v-model="pointForm.scale" :step="0.001" :precision="4" controls-position="right" style="width: 100%" />
          <el-input-number v-model="pointForm.offset" :step="0.001" :precision="4" controls-position="right" style="width: 100%" />
        </div>
        <div class="point-inherit-hint">实际值 = 原始值 × 倍率 + 偏移；倍率不能为 0（1:1 透传请填 1）。</div>
      </el-form-item>
      <!-- 变比：电压只吃 PT、电流只吃 CT、功率/电量两者都吃、功率因数都不吃 -->
      <el-form-item label="CT 变比" prop="ct_ratio">
        <el-input-number
          v-model="pointForm.ct_ratio"
          :min="0.001"
          :step="0.1"
          :precision="4"
          :disabled="!ctEnabled"
          controls-position="right"
          placeholder="电流互感比，留空=1"
          style="width: 100%"
        />
      </el-form-item>
      <el-form-item label="PT 变比" prop="pt_ratio">
        <el-input-number
          v-model="pointForm.pt_ratio"
          :min="0.001"
          :step="0.1"
          :precision="4"
          :disabled="!ptEnabled"
          controls-position="right"
          placeholder="电压互感比，留空=1"
          style="width: 100%"
        />
        <div class="point-inherit-hint">{{ ratioHint }}</div>
      </el-form-item>
      <el-form-item label="计算公式" prop="formula">
        <el-input v-model="pointForm.formula" placeholder="如: raw * 0.1 + 5（填写后优先于倍率/偏移）" />
      </el-form-item>
      <el-form-item label="死区 / 变化上传">
        <div class="point-pair">
          <el-input-number v-model="pointForm.deadband" :min="0" :step="0.1" :precision="4" controls-position="right" style="width: 100%" />
          <el-switch v-model="pointForm.on_change" active-text="变化才上传" />
        </div>
        <div class="point-inherit-hint">死区：变化幅度小于该值时不产生新数据帧（0 = 不启用）。「变化才上传」沿用同一阈值；死区为 0 时仅过滤数值完全相同的样本。</div>
      </el-form-item>
      <el-form-item label="告警高/低限" prop="alarm_high">
        <div class="point-pair">
          <el-input-number v-model="pointForm.alarm_high" :precision="3" controls-position="right" placeholder="高限，留空=不启用" style="width: 100%" />
          <el-input-number v-model="pointForm.alarm_low" :precision="3" controls-position="right" placeholder="低限，留空=不启用" style="width: 100%" />
        </div>
        <div class="point-inherit-hint">超过高限或低于低限时该点位标记为告警；留空表示该侧不启用。</div>
      </el-form-item>
      <el-form-item label="聚合">
        <div class="point-pair">
          <el-select v-model="pointForm.aggregation_fn" placeholder="不聚合" clearable style="width: 100%">
            <el-option label="不聚合" value="" />
            <el-option label="最新值" value="last" />
            <el-option label="平均值" value="avg" />
            <el-option label="最大值" value="max" />
            <el-option label="最小值" value="min" />
            <el-option label="求和" value="sum" />
          </el-select>
          <DurationSelect
            v-model="pointForm.aggregation_window"
            :min="1"
            :max="3600"
            :units="['s', 'min']"
            :disabled="!pointForm.aggregation_fn"
            num-width="100%"
            unit-width="82px"
          />
        </div>
        <div class="point-inherit-hint">聚合窗口（秒）：窗口内多次采样合并成一个上报值；选「不聚合」则逐帧上报。快照上报模式下窗口需 ≥ 上报周期，否则中间窗口会被折叠丢弃。</div>
      </el-form-item>
    </el-form>
    <template #footer>
      <el-button @click="formGuard.close()">取消</el-button>
      <el-button type="primary" :loading="submitting" @click="submitPoint">{{ formMode === 'create' ? '创建' : '保存' }}</el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { computed, inject } from 'vue'
import { pointFormContextKey } from '../pointFormContext'
import { protocolAddressPlaceholder } from '@/composables/devices/useProtocol'
import { unitChoices } from '@/composables/devices/useUnits'
import { ratioScopeOf } from '@/composables/devices/useRatio'
import { modbusAddressOf, parseModbusAddress } from '@/composables/devices/usePoints/modbusAddress'
import { FALLBACK_NORTH_INTERVAL } from '@/composables/devices/usePoints/validation'
import { BYTE_ORDERS, DATA_TYPES, usesNumericDecode } from '@/composables/devices/decodeParams'
import DurationSelect from '@/components/industrial/DurationSelect.vue'
import ModbusAddressInput from '@/components/industrial/ModbusAddressInput.vue'

/**
 * 单点新增 / 编辑弹窗
 *
 * 从 PointList.vue 迁出（页面文件超 500 行预算）。表单模型、校验规则、
 * el-form 实例、提交与未保存守卫仍由页面（PointList.vue）通过
 * `pointFormContextKey` 提供，这里只负责渲染与派生展示逻辑。
 *
 * 为什么不用 props 下发表单模型：`v-model="pointForm.xxx"` 会改 props 的深层字段，
 * 触发 `vue/no-mutating-props`；provide/inject 共享同一个 reactive 对象不受该规则约束。
 * 详见 `pointFormContext.ts` 的说明。
 */
const ctx = inject(pointFormContextKey, null)
if (!ctx) throw new Error('PointsFormDialog 必须在 PointList 内使用（缺少点位表单上下文）')

const {
  pointForm, pointRules, pointFormRef, formMode, submitting, showForm,
  currentDeviceName, protocolLabel, formGuard, submitPoint, resetForm,
  northInterval: northIntervalRef,
} = ctx

/**
 * 北向上报周期（秒）：采集周期上界。
 *
 * 正常由 composable 的 state 提供；读不到时退回兜底值（与 usePoints 同源），
 * 免得整个采集周期输入框因为一个周期值缺失而不可用。
 */
const northInterval = computed(() => northIntervalRef?.value ?? FALLBACK_NORTH_INTERVAL)

/**
 * 采集周期输入框上界：必须小于北向上报周期（后端 validate_cycle_conflict）。
 * 下限取 1，避免上报周期被配成 1s 时算不出合法区间。
 */
const intervalMax = computed(() => Math.max(1, northInterval.value - 1))

/** 协议继承自设备，Modbus 才需要区段寻址与寄存器数 */
const isModbus = computed(() => pointForm.protocol === 'modbus')

/**
 * 是否露出「数据类型 / 字节序」。
 *
 * 比 isModbus 宽：DLT645 / CJT188 电表同样用 u16/f32 + 字节序解码，
 * 后端 `uses_numeric_decode` 也认这几个协议；只按 modbus 门控会让电表点位
 * 在表单里根本填不了这两项，只能绕 CSV。
 */
const numericDecode = computed(() => usesNumericDecode(pointForm.protocol))

/** 数据类型提示：寄存器数只有 Modbus 可填，其余协议的类型口径来自设备手册 */
const dataTypeHint = computed(() =>
  isModbus.value
    ? 'float32 / u32 等多寄存器类型需填寄存器数；留空按数据类型自动推断。'
    : '按设备手册的数据标识 / 类型填写；留空按 u16 解码。多字节类型的字节序在下方单独设置。',
)

/**
 * Modbus 地址输入（区段 + 1 基序号）。
 *
 * get 由真实寻址参数（function_code + address_num）推导，set 反向写入三者，
 * 保证「改地址」就是「改寻址」，不再出现地址字符串与偏移各说各话。
 */
const modbusAddr = computed<string>({
  get: () => modbusAddressOf(pointForm.function_code, pointForm.address_num),
  set: (v) => {
    const parsed = parseModbusAddress(v)
    if (!parsed) return
    pointForm.function_code = parsed.functionCode
    pointForm.address_num = parsed.addressNum
    pointForm.address = v
  },
})

/** 偏移超过 9998（1 基地址 9999）时 5 位地址装不下，退回手填 */
const modbusAddrOverflow = computed(() => (pointForm.address_num ?? 0) > 9998)

/** 变比适用范围：电压只吃 PT、电流只吃 CT、功率/电量两者都吃、功率因数都不吃 */
const ratioScope = computed(() => ratioScopeOf(pointForm.unit))
const ctEnabled = computed(() => ratioScope.value === 'ct' || ratioScope.value === 'both')
const ptEnabled = computed(() => ratioScope.value === 'pt' || ratioScope.value === 'both')
const ratioHint = computed(() => {
  switch (ratioScope.value) {
    case 'pt':
      return '当前单位为电压：只有 PT 变比参与计算，CT 不生效（已置空）。'
    case 'ct':
      return '当前单位为电流：只有 CT 变比参与计算，PT 不生效（已置空）。'
    case 'none':
      return '当前单位为功率因数（无量纲）：CT / PT 都不参与计算。'
    default:
      return '功率 = 二次侧值 × CT × PT（P = U × I）；电量同理。电压只需 PT、电流只需 CT。留空表示 1（不放大）。'
  }
})

// 模板用字符串 ref="pointFormRef" 反向绑定该 Ref，脚本侧没有直接读取点（校验发生在
// Composable 的 submitPoint 内）。noUnusedLocals 不把字符串 ref 计入「已使用」，故显式取值。
void pointFormRef
</script>

<style scoped>
/* N3: 点位弹窗中继承自设备的字段说明（总线/协议只读，指向设备列表调整） */
.point-inherit-hint {
  margin-top: 4px;
  font-size: 0.75rem;
  line-height: 1.5;
  color: var(--text-secondary);
}
/* BUG-B: 弹窗中成对出现的数值输入（倍率/偏移、死区、告警限值、聚合） */
.point-pair {
  display: flex;
  gap: 8px;
  align-items: center;
  width: 100%;
}
</style>
