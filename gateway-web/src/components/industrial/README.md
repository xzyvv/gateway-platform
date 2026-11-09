# 工业表单输入控件集（Industrial Form Controls）

> 路径：`src/components/industrial/`
> 版本：v0.5（2026-10）
> 设计原则：每个组件独立 props/emits，无业务耦合，自由组合

## 组件清单（10 个）

| 组件 | 单位 | 典型值 | 场景 |
|------|------|--------|------|
| **BaudrateSelect** | 数字 | 1200-230400 bps | 串口总线配置（RS-232/485） |
| **DurationSelect** | 数字+单位 | ms/s/min/h/d 自由组合 | 任意时长（周期/间隔/超时） |
| **PresetNumberSelect** | 数字 | 常用档位 + 自定义输入 | 区间参数（限流/缓冲/队列/心跳） |
| **SlaveIdInput** | 数字 0-247 | 常用预设 1/2/10/100/247 + 自定义 | Modbus 从站地址 |
| **ProtocolSelector** | 字符串 | modbus/dlt645/iec104/opcua 等 | 设备 / 协议选择 |
| **ModbusAddressInput** | 区段+偏移 | 0xxxx/1xxxx/3xxxx/4xxxx | Modbus 地址（自动算功能码） |
| **ParitySelect** | 字符串 | none / even / odd | 串口校验位 |
| **StopBitsSelect** | 数字 | 1 / 1.5 / 2 | 串口停止位 |
| **DataBitsSelect** | 数字 | 5 / 6 / 7 / 8 | 串口数据位 |
| **HexInput** | 字符串 | 2/4 字节对齐十六进制 | 报文编辑 / 帧构造（PacketEditor） |

## 设计原则

1. **零业务依赖**：每个组件不引用任何业务模块（store/api/mock），仅封装 UI 表达。
2. **统一 props 风格**：`v-model` 必填 / `disabled`/`placeholder`/`width`/`clearable` 通用 / 业务相关 props 各异。
3. **可选 vs 必选**：涉及"工业固定值集合"时给常用预设，避免每次手动输入；支持 `allowCreate` 允许用户输入自定义值。
4. **分组展示**：当选项 > 8 个时用 `el-option-group` 按协议/单位分组，便于检索。
5. **可搜索**：`filterable: true` 默认开启，长列表场景下让用户能搜索（端口/协议）。

## 使用示例

### 单个组件引入

```ts
import BaudrateSelect from '@/components/industrial/BaudrateSelect.vue'

<el-form-item label="波特率">
  <BaudrateSelect v-model="bus.baud_rate" />
</el-form-item>
```

### 引入方式

本目录**只支持按文件引入**（N-9 已删除零使用的 `index.ts` 桶文件，避免死代码堆积）：

```ts
import BaudrateSelect from '@/components/industrial/BaudrateSelect.vue'
```

## 新增组件判定标准

满足以下任一条件即可纳入本目录：

- 工业场景专用值集合（波特率/Modbus 地址/PLC 协议等）
- 需要在 ≥ 3 个页面复用
- 通用 UI 控件（el-select/el-input-number）写起来 > 10 行且重复度高
- 需要业务术语标准化（如校验位 N/E/O/M/S、字节序大/小端）

## 演进路线

- ✅ v0.1：Baudrate / Interval / Duration / Protocol
- ✅ v0.2：+ TimeRange / Port / SlaveId
- ✅ v0.3：+ ModbusAddress / DataType / ByteOrder
- ✅ v0.4：Parity / StopBits / DataBits
- ✅ v0.5：HexInput / IpInput；删除零使用的 TimeRangePicker / PortInput / DataTypeSelect / ByteOrderSelect
- ✅ N-9：删除零使用的 IpInput.vue 与 `index.ts` 桶文件（全站均为按文件引入）
- ✅ v0.6：+ PresetNumberSelect（预设档位 + 自定义）；DurationSelect 新增毫秒档并接管 Webhook 上报间隔；删除零使用的 IntervalSelect.vue

## 维护说明

- 每个组件文件头部 JSDoc 注释记录：props / emits / 典型应用场景
- 修改组件需同步更新本 README 的"组件清单"
- 新增组件先在 README 中规划，再实现，避免散落