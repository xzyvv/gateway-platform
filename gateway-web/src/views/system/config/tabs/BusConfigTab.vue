<template>
  <div class="bus-actions">
    <el-button type="primary" size="small" @click="emit('add')">
      <el-icon><Plus /></el-icon> 新增总线
    </el-button>
  </div>
  <el-form v-for="(bus, index) in buses" :key="index" label-width="100px" class="bus-form">
    <div class="bus-header">
      <div class="bus-title">
        <span class="bus-badge">{{ index + 1 }}</span>
        <el-tag :type="protocolType(bus.protocol)" size="small" effect="light">
          {{ protocolLabel[bus.protocol] || bus.protocol }}
        </el-tag>
        <span class="bus-transport">{{ transportText(bus.protocol) }}</span>
      </div>
      <el-button type="danger" size="small" plain @click="emit('remove', index)">
        <el-icon><Delete /></el-icon> 删除
      </el-button>
    </div>
    <div class="bus-fields">
      <!-- 串行协议选串口；网络协议为纯标识名（后端 port_name 同时是总线名） -->
      <el-form-item label="总线名称" :error="portErrors.get(index)">
        <PortSelect v-if="isSerial(bus.protocol)" v-model="bus.port_name" />
        <el-input v-else v-model="bus.port_name" placeholder="如: iec104_1（仅作总线标识）" />
      </el-form-item>

      <template v-if="isSerial(bus.protocol)">
        <el-form-item label="波特率">
          <BaudrateSelect v-model="bus.baud_rate" />
        </el-form-item>
        <el-form-item label="校验位">
          <ParitySelect v-model="bus.parity" />
        </el-form-item>
        <el-form-item label="数据位">
          <DataBitsSelect v-model="bus.data_bits" />
        </el-form-item>
        <el-form-item label="停止位">
          <StopBitsSelect v-model="bus.stop_bits" />
        </el-form-item>
      </template>

      <!-- 网络类协议填对端地址；Modbus 留空走串口，填写后走 Modbus TCP -->
      <el-form-item v-if="showEndpoint(bus.protocol)" label="对端地址">
        <el-input
          v-model="bus.tcp_addr"
          clearable
          :placeholder="protocolEndpointPlaceholder(bus.protocol)"
        />
        <div class="field-hint">
          {{ endpointHint(bus.protocol) }}
        </div>
      </el-form-item>

      <el-form-item label="协议" class="bus-protocol">
        <ProtocolSelector
          :model-value="bus.protocol"
          @update:model-value="(v: string) => emit('protocol-change', bus, v)"
        />
      </el-form-item>

      <el-form-item v-if="protocolVariantsOf(bus.protocol).length > 0" label="协议变体">
        <el-select
          :model-value="bus.variant ?? ''"
          placeholder="默认"
          clearable
          aria-label="协议变体"
          @update:model-value="(v: string) => (bus.variant = v || null)"
        >
          <el-option
            v-for="v in protocolVariantsOf(bus.protocol)"
            :key="v.value"
            :label="v.label"
            :value="v.value"
          />
        </el-select>
      </el-form-item>

      <!-- OPC UA Client 认证：不配置（opcua 为 null）即匿名 + 明文，存量配置行为不变 -->
      <div v-if="bus.protocol === 'opcua'" class="bus-opcua">
        <div class="bus-opcua-title">OPC UA 认证（不配置即匿名只读）</div>
        <el-form-item label="认证方式">
          <el-select
            :model-value="authOf(bus).auth"
            aria-label="OPC UA 认证方式"
            @update:model-value="(v: string) => patchOpcua(bus, { auth: v })"
          >
            <el-option label="匿名（默认）" value="anonymous" />
            <el-option label="用户名 / 密码" value="username" />
            <el-option label="X.509 证书" value="certificate" />
          </el-select>
        </el-form-item>
        <template v-if="authOf(bus).auth === 'username'">
          <el-form-item label="用户名">
            <el-input
              :model-value="authOf(bus).username ?? ''"
              placeholder="Server 分配的用户名"
              @update:model-value="(v: string) => patchOpcua(bus, { username: v === '' ? null : v })"
            />
          </el-form-item>
          <el-form-item label="密码">
            <el-input
              type="password"
              show-password
              :model-value="authOf(bus).password ?? ''"
              placeholder="支持 enc: 密文"
              @update:model-value="(v: string) => patchOpcua(bus, { password: v === '' ? null : v })"
            />
          </el-form-item>
        </template>
        <el-form-item label="安全策略">
          <el-select
            :model-value="authOf(bus).security_policy"
            aria-label="OPC UA 安全策略"
            @update:model-value="(v: string) => setOpcuaPolicy(bus, v)"
          >
            <el-option label="None（明文，默认）" value="none" />
            <el-option label="Basic256Sha256" value="basic256sha256" />
          </el-select>
        </el-form-item>
        <el-form-item label="安全模式">
          <el-select
            :model-value="authOf(bus).security_mode"
            aria-label="OPC UA 安全模式"
            @update:model-value="(v: string) => setOpcuaMode(bus, v)"
          >
            <el-option label="None（默认）" value="none" />
            <el-option label="Sign（仅签名）" value="sign" />
            <el-option label="Sign & Encrypt（签名+加密）" value="sign_and_encrypt" />
          </el-select>
        </el-form-item>
        <template v-if="needsCert(bus)">
          <el-form-item label="客户端证书">
            <el-input
              :model-value="authOf(bus).client_cert_path ?? ''"
              placeholder="/etc/gateway/opcua/client.crt"
              @update:model-value="(v: string) => patchOpcua(bus, { client_cert_path: v === '' ? null : v })"
            />
          </el-form-item>
          <el-form-item label="客户端私钥">
            <el-input
              :model-value="authOf(bus).client_key_path ?? ''"
              placeholder="/etc/gateway/opcua/client.key"
              @update:model-value="(v: string) => patchOpcua(bus, { client_key_path: v === '' ? null : v })"
            />
          </el-form-item>
        </template>
        <el-form-item label="信任服务器证书">
          <el-switch
            :model-value="authOf(bus).trust_server_certs"
            @update:model-value="(v: boolean) => patchOpcua(bus, { trust_server_certs: v })"
          />
          <div class="field-hint">默认自动信任；生产环境建议关闭并预置 Server 证书。</div>
        </el-form-item>
      </div>

      <!-- IEC104 公共地址（CA）仅支持 1 / 2 字节，留空按 2 字节 -->
      <el-form-item v-if="bus.protocol === 'iec104'" label="公共地址长度">
        <el-select
          :model-value="bus.asdu_addr_len ?? undefined"
          placeholder="默认 2 字节"
          clearable
          aria-label="公共地址长度"
          @update:model-value="(v: number | undefined) => (bus.asdu_addr_len = v ?? null)"
        >
          <el-option label="2 字节（默认）" :value="2" />
          <el-option label="1 字节" :value="1" />
        </el-select>
      </el-form-item>

      <!-- 填了对端地址才有多事务并发可言，串口总线恒为 1 -->
      <el-form-item v-if="bus.tcp_addr" label="TCP 并发">
        <el-input-number
          v-model="bus.tcp_concurrent"
          :min="1"
          :max="1024"
          placeholder="默认 8"
          controls-position="right"
          aria-label="TCP 并发数"
          style="width: 100%"
        />
        <div class="field-hint">单连接同时在途请求数，默认 8</div>
      </el-form-item>
      <!-- 采集调优：吞吐/硬件相关，默认值适配多数现场，折叠起来避免误改 -->
      <el-collapse class="bus-advanced">
        <el-collapse-item name="advanced">
          <template #title>
            <span class="advanced-name">高级（采集调优）</span>
          </template>
          <el-form-item label="通道容量">
            <el-input-number
              v-model="bus.channel_capacity"
              :min="1"
              :max="4096"
              controls-position="right"
              style="width: 100%"
            />
            <div class="field-hint">
              单总线在途采集事件的缓冲深度（默认 128）；高频 TCP 总线可调大，改完保存即重建该总线。
            </div>
          </el-form-item>
          <el-form-item label="RS485 方向引脚">
            <el-input-number
              :model-value="bus.de_pin ?? undefined"
              :min="0"
              :max="1024"
              placeholder="留空=内核自动方向"
              controls-position="right"
              style="width: 100%"
              aria-label="RS485 方向引脚"
              @update:model-value="(v: number | undefined) => (bus.de_pin = v ?? null)"
            />
            <div class="field-hint">
              仅串口总线有效；填 GPIO 编号（如 17）由程序控制 DE/RE 收发方向，留空沿用内核自动方向。
            </div>
          </el-form-item>
          <el-form-item label="重连退避">
            <div class="backoff-field">
              <el-select
                :model-value="busBackoffPreset(bus)"
                style="width: 100%"
                aria-label="重连退避预设"
                @update:model-value="(v: BackoffPreset) => setBusBackoff(bus, v)"
              >
                <el-option
                  v-for="opt in BACKOFF_OPTIONS"
                  :key="opt.value"
                  :value="opt.value"
                  :label="opt.label"
                />
              </el-select>
              <div class="field-hint">当前：{{ busBackoffText(bus) }}</div>
            </div>
          </el-form-item>
        </el-collapse-item>
      </el-collapse>
    </div>
  </el-form>
  <empty-state
    v-if="buses.length === 0"
    description="还没有总线，点下方按钮创建第一条"
    :image-size="80"
    action-text="新增总线"
    @action="emit('add')"
  />
</template>

<script setup lang="ts">
import BaudrateSelect from '@/components/industrial/BaudrateSelect.vue'
import PortSelect from '@/components/industrial/PortSelect.vue'
import ParitySelect from '@/components/industrial/ParitySelect.vue'
import DataBitsSelect from '@/components/industrial/DataBitsSelect.vue'
import StopBitsSelect from '@/components/industrial/StopBitsSelect.vue'
import ProtocolSelector from '@/components/industrial/ProtocolSelector.vue'
import {
  protocolEndpointPlaceholder,
  protocolLabel,
  protocolTransportOf,
  protocolType,
  protocolVariantsOf,
} from '@/composables/devices/useProtocol'
import type { BusConfig, OpcUaClientConfig } from '@/api/types'
import {
  BACKOFF_OPTIONS,
  BACKOFF_PRESETS,
  applyPreset,
  presetOf,
  type BackoffPreset,
} from '@/utils/backoffPresets'

// 总线列表为受控模型；增删/协议切换等业务逻辑仍由 useConfigActions 提供，经事件上抛。
const buses = defineModel<BusConfig[]>({ required: true })
defineProps<{ portErrors: Map<number, string> }>()
const emit = defineEmits<{
  add: []
  remove: [index: number]
  'protocol-change': [bus: BusConfig, next: string]
}>()

const isSerial = (protocol: string) => protocolTransportOf(protocol) === 'serial'

/**
 * 对端地址输入是否展示。
 *
 * - 网络协议（transport = tcp）必填，否则采集器没有连接目标；
 * - Modbus 是串行协议，但填写 `tcp_addr` 后走 Modbus TCP，故也展示，留空即走串口。
 */
const showEndpoint = (protocol: string) =>
  protocolTransportOf(protocol) === 'tcp' || protocol === 'modbus'

const transportText = (protocol: string) => {
  const t = protocolTransportOf(protocol)
  return t === 'tcp' ? '网络' : t === 'local' ? '本机' : '串行'
}

const endpointHint = (protocol: string) =>
  protocol === 'modbus'
    ? '留空走串口 Modbus RTU / ASCII；填写后走 Modbus TCP'
    : protocol === 'bacnet_mstp'
      ? '主站参数，纯串行总线可留空'
      : '网络协议必填，如未填写该总线无法建立连接'

/** 与后端 OpcUaClientConfig::default() 对齐：匿名 + 明文 + 自动信任服务器证书。 */
const OPCUA_DEFAULTS: OpcUaClientConfig = {
  auth: 'anonymous',
  username: null,
  password: null,
  security_policy: 'none',
  security_mode: 'none',
  client_cert_path: null,
  client_key_path: null,
  trust_server_certs: true,
}

/** 读取认证块：未配置时返回默认值，不在渲染期写回模型（避免副作用）。 */
const authOf = (bus: BusConfig): OpcUaClientConfig => bus.opcua ?? OPCUA_DEFAULTS

/** 局部更新认证块：首次修改时才把 bus.opcua 物化出来。 */
function patchOpcua(bus: BusConfig, patch: Partial<OpcUaClientConfig>) {
  bus.opcua = { ...authOf(bus), ...patch }
}

/** 策略与模式互为约束：none 策略只能配 none 模式；切到非 none 策略时自动升级为签名加密，
 *  避免用户保存时才被后端校验拦下。 */
function setOpcuaPolicy(bus: BusConfig, policy: string) {
  const current = authOf(bus).security_mode
  patchOpcua(bus, {
    security_policy: policy,
    security_mode: policy === 'none' ? 'none' : current === 'none' ? 'sign_and_encrypt' : current,
  })
}

/** 选择签名/加密模式时，策略不能停留在 none，自动升级为 Basic256Sha256。 */
function setOpcuaMode(bus: BusConfig, mode: string) {
  const policy = authOf(bus).security_policy
  patchOpcua(bus, {
    security_mode: mode,
    security_policy: mode === 'none' || policy !== 'none' ? policy : 'basic256sha256',
  })
}

/** 需要证书/私钥：X.509 认证，或非 none 的安全策略。 */
const needsCert = (bus: BusConfig) =>
  authOf(bus).auth === 'certificate' || authOf(bus).security_policy !== 'none'

/** 反查总线当前退避档位（不在预设内则为「自定义」展示态） */
const busBackoffPreset = (bus: BusConfig): BackoffPreset =>
  presetOf(bus.reconnect_backoff ?? BACKOFF_PRESETS.standard)

/** 当前退避的可读描述；旧配置可能没有该字段，回落标准档 */
function busBackoffText(bus: BusConfig): string {
  const b = bus.reconnect_backoff ?? BACKOFF_PRESETS.standard
  return `${b.seconds.join(' / ')} 秒，抖动 ${Math.round(b.jitter_ratio * 100)}%`
}

/** 选中预设时改写退避参数；「自定义」不改写，保护现场手调的档位 */
function setBusBackoff(bus: BusConfig, preset: BackoffPreset) {
  const next = applyPreset(preset)
  if (!next) return
  bus.reconnect_backoff = next
}
</script>

<style scoped>
.bus-advanced {
  margin: 4px 0 8px;
}
.bus-advanced :deep(.el-collapse-item__header) {
  padding-left: 0;
}
.bus-advanced :deep(.el-collapse-item__wrap) {
  padding-left: 0;
  padding-right: 0;
}
.advanced-name {
  font-weight: 600;
}
.backoff-field {
  width: 100%;
}
</style>
