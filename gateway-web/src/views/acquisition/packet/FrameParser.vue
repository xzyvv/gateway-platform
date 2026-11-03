<template>
  <div class="parser">
    <!-- W72: 遵循 VISUAL_SPEC 第8.2 一级标题使用 el-card + section-header::before 装饰条 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="帧解析" icon="DataAnalysis" />
      </template>
      <SectionHeader title="输入协议帧" icon="Edit" />
      <el-form :inline="true" label-width="80px">
        <el-form-item label="协议">
          <el-select v-model="protocol" style="width: 180px">
            <el-option label="Modbus RTU" value="modbus-rtu" />
            <el-option label="DLT645" value="dlt645" />
            <el-option label="CJT188" value="cjt188" />
          </el-select>
        </el-form-item>
        <el-form-item label="示例">
          <el-button size="small" @click="useSample">使用示例</el-button>
          <el-button
            v-if="errorSample"
            size="small"
            type="warning"
            plain
            @click="useErrorSample"
          >
            异常应答示例
          </el-button>
        </el-form-item>
      </el-form>
      <el-input
        v-model="hexInput"
        type="textarea"
        :rows="3"
        :maxlength="4096"
        show-word-limit
        placeholder="输入 HEX 帧数据，如: 01 03 00 00 00 0A C5 CD"
        aria-label="HEX 帧数据"
        class="hex-input"
      />
        <!-- W75: maxlength=4096 防止超长输入导致页面卡顿 -->
      <div class="actions">
        <el-button type="primary" @click="parse"><el-icon><Search /></el-icon> 解析</el-button>
        <el-button @click="hexInput = ''"><el-icon><Delete /></el-icon> 清空</el-button>
      </div>
    </el-card>

    <!-- 解析结果：与在线响应用同一个展示组件（C4） -->
    <FrameParseResult v-if="result" :result="result" class="parse-result" />

    <!-- 协议说明 -->
    <el-card shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="协议帧格式说明" icon="InfoFilled" />
      </template>
      <el-tabs v-model="activeTab">
        <el-tab-pane label="Modbus RTU" name="modbus">
          <div class="proto-desc">
            <p><strong>帧结构：</strong>从站地址(1B) + 功能码(1B) + 数据(NB) + CRC校验(2B)</p>
            <p><strong>功能码：</strong>01读线圈、02读离散量、03读保持寄存器、04读输入寄存器、05写单个线圈、06写单个寄存器、15写多个线圈、16写多个寄存器</p>
            <p><strong>CRC：</strong>CRC-16/MODBUS，低字节在前</p>
          </div>
        </el-tab-pane>
        <el-tab-pane label="DLT645" name="dlt645">
          <div class="proto-desc">
            <p><strong>帧结构：</strong>帧头68H + 地址域(6B) + 帧头68H + 控制域(1B) + 数据长度(1B) + 数据(NB) + 校验码(1B) + 帧尾16H</p>
            <p><strong>加解密：</strong>每个字节加 33H 加密，发送时减 33H</p>
            <p><strong>校验码：</strong>从帧头到数据域最后一个字节的累加和模 256</p>
            <p><strong>异常应答：</strong>控制码 D6 位=1 且数据长度 L=01H，数据域唯一字节即为错误信息字 ERR</p>
            <p><strong>错误信息字 ERR：</strong><b>位标志</b>——bit0 其他错误、bit1 无请求数据、bit2 密码错/未授权、bit3 通信速率不能更改、bit4 年时区数超、bit5 日时段数超、bit6 费率数超。各位可同时置位</p>
          </div>
        </el-tab-pane>
        <el-tab-pane label="CJT188" name="cjt188">
          <div class="proto-desc">
            <p><strong>帧结构：</strong>帧头68H + 仪表类型(1B) + 地址域(7B) + 控制域(1B) + 数据长度(1B) + 数据(NB) + 校验码(1B) + 帧尾16H</p>
            <p><strong>仪表类型：</strong>10H冷水表、11H热水表、12H直饮水表、20H电能表</p>
            <p><strong>异常应答：</strong>控制码 D6 位=1，数据域为 序列号SER(1B) + 状态ST(2B)，L=03H</p>
            <p><strong>状态 ST：</strong>第1字节为位标志（D0 阀门开关、D1 阀门状态、D2 电池电压）；第2字节为诊断码 JD（低4位电池/换能器/E2PROM 类，高4位温度传感器类）</p>
          </div>
        </el-tab-pane>
      </el-tabs>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import {Search, Delete} from '@element-plus/icons-vue'
import { parseFrame, sampleFrames, sampleErrorFrames, type ParseResult } from '@/utils/frameParser'
import FrameParseResult from './parts/FrameParseResult.vue'

const protocol = ref<'modbus-rtu' | 'dlt645' | 'cjt188'>('modbus-rtu')
const hexInput = ref('')
const result = ref<ParseResult | null>(null)
const activeTab = ref('modbus')

/** Modbus 的异常码是独立的异常响应帧（功能码 | 0x80），与 DLT645/CJT188
 *  的「异常应答帧」机制不同，故只有后两者提供异常应答示例 */
const errorSample = computed(() =>
  protocol.value === 'modbus-rtu' ? '' : sampleErrorFrames[protocol.value],
)

function useSample() {
  hexInput.value = sampleFrames[protocol.value]
  parse()
}

/** 载入异常应答示例帧：演示如何从帧里读出错误码（ERR 位标志 / 状态 ST） */
function useErrorSample() {
  const s = errorSample.value
  if (!s) return
  hexInput.value = s
  parse()
}

function parse() {
  const input = hexInput.value.trim()
  if (!input) return
  // W73: hex 格式预校验，空格/0x/非 hex 字符提前过滤并友好提示
  // parseFrame 内部不做信息丢弃，直接抛 "invalid hex" 错误
  const cleaned = input.replace(/\s+/g, '').replace(/^0x/i, '')
  if (cleaned.length === 0) return
  if (!/^[0-9a-fA-F]+$/.test(cleaned)) {
    result.value = {
      valid: false,
      protocol: protocol.value,
      rawHex: input,
      fields: [],
      error: '请输入合法的 HEX 字符（仅支持 0-9 a-f A-F 和空格分隔）',
    }
    return
  }
  result.value = parseFrame(protocol.value, input)
}

</script>

<style scoped>
.parser { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.hex-input { margin-bottom: 12px; font-family: monospace; }
.actions { display: flex; gap: 8px; }
.parse-result { display: block; margin-top: 16px; }
.proto-desc p { margin: 6px 0; line-height: 1.8; font-size: 0.8125rem; color: var(--text-regular); }
.proto-desc strong { color: var(--text-primary); }
</style>
