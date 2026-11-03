import {
  Odometer, DataLine, Cpu, Connection, Setting, Histogram, Document, InfoFilled, HomeFilled,
  Upload, Operation, Monitor, Share, VideoCamera, Timer, Collection, Switch,
  TrendCharts, Aim, Stopwatch, Management,
  Key, Lock, Grid, Promotion, Search, Files, DataAnalysis,
  CollectionTag, FirstAidKit, Link, RefreshLeft, CopyDocument, MagicStick, Edit, Folder, CircleCheck, Coin, VideoPlay, Tools, Compass,
} from '@element-plus/icons-vue'
import type { NavIndexItem } from './types'

/**
 * 侧栏搜索索引：把「菜单项」与「Hub 页签级深链」摊平成一张可搜索表。
 *
 * 侧栏搜索框是全站唯一的功能检索入口，命中的是页面与页签，
 * 因此这里必须覆盖每个 Hub 页签（见 navTargets.spec.ts 守护）。
 * alias 用于登记历史叫法（页面改名 / 合并前的旧名），让老习惯仍能搜到。
 */
const navIndex: NavIndexItem[] = [
  { id: 'nav-dashboard', title: '运行总览', icon: HomeFilled, target: '/overview' },
  { id: 'nav-realtime', title: '实时数据', icon: DataLine, target: '/overview/realtime' },
  // alias 覆盖 Hub 内页签的实际标签：「设备清单」/「点位清单」/「网关信息」/「实时日志」，
  // 否则用户按页签上看到的文字搜索会搜不到（搜索只匹配 title/subtitle/alias）。
  // A7：原「资产台账」页签已并入设备行（行内「资产」按钮），旧叫法登记为别名
  { id: 'nav-devices', title: '设备管理', alias: '设备清单 资产台账', icon: Cpu, target: '/devices?tab=devices' },
  { id: 'nav-quick-setup', title: '快速接入', subtitle: '总线、设备、点位三步接入并连续读取验证', icon: MagicStick, target: '/devices/quick-setup' },
  { id: 'nav-points', title: '点位管理', alias: '点位清单', icon: Connection, target: '/points?tab=points' },
  { id: 'nav-config', title: '运行参数', icon: Management, target: '/system/config?tab=runtime-config' },
  { id: 'nav-config-center', title: '配置管理', icon: Setting, target: '/system/config' },
  { id: 'nav-comm-profile-hub', title: '采集调优', alias: '通信优化', icon: Stopwatch, target: '/acquisition/comm-tuning' },
  // 命名统一：菜单分组叫「数据流水」，页签叫「历史数据」，此处以「历史数据」为主名、
  // 「数据流水」为别名，两者都能搜到（此前只有 alias 方向，搜「数据流水」才命中）。
  { id: 'nav-history', title: '历史数据', alias: '数据流水', icon: Histogram, target: '/data?tab=history' },
  { id: 'nav-logs', title: '运行日志', alias: '实时日志', icon: Document, target: '/system?tab=logs' },
  { id: 'nav-system', title: '系统设置', alias: '网关信息', icon: InfoFilled, target: '/system' },
  { id: 'nav-ota', title: '固件升级', alias: '固件与证书', icon: Upload, target: '/system?tab=ota' },
  { id: 'nav-tester', title: '点位读写', alias: '读写测试', icon: Operation, target: '/acquisition/tester' },
  { id: 'nav-batch-read-test', title: '读取性能测试', alias: '批量读取', icon: Timer, target: '/acquisition/tester?tab=batch-read-test' },
  // 第三阶段 C1：原「协议扫描 / 协议仿真 / 报文调试」三个 Hub 合并为统一「协议调试台」，
  // 汇总入口只保留这一条；旧叫法登记为别名，按老名字搜索仍能命中
  { id: 'nav-protocol-console', title: '协议调试台', alias: '协议工具 协议扫描 协议仿真 报文调试', subtitle: '扫描 / 报文 / 模拟一站式调试，先选总线目标再选动作', icon: Tools, target: '/acquisition/console' },
  // C3：按协议与症状编排的引导式排查，面向新人与现场应急
  { id: 'nav-debug-wizard', title: '现场调试向导', alias: '排查向导 调试向导 读不到数', subtitle: '按协议与症状按顺序执行检查项，可自动检查总线 / 健康 / 设备', icon: Compass, target: '/acquisition/console?tab=debug-wizard' },
  // S7: 原「批量任务」是误导性别名；W1 后独立页下线，入口并入实时数据的「仅看收藏」筛选。
  { id: 'nav-monitor', title: '收藏监控', alias: '监视列表 收藏列表', icon: Monitor, target: '/overview/realtime?fav=1' },
  // 原「性能监控 / 链路诊断 / 健康巡检」三个 Hub 已合并为「运维监控」Hub：
  // 菜单主名取 Hub 名，三个旧 Hub 名全部降为别名，旧习惯仍能搜到。
  { id: 'nav-perf-monitor', title: '运维监控', alias: '性能监控', icon: TrendCharts, target: '/ops' },
  { id: 'nav-comm-diag', title: '链路诊断', alias: '通信诊断', icon: Aim, target: '/ops?tab=connection-test' },
  { id: 'nav-health-monitor', title: '健康检查', alias: '健康巡检', icon: FirstAidKit, target: '/ops?tab=gateway-diag' },
  { id: 'nav-topology', title: '设备拓扑', icon: Share, target: '/devices/topology' },
  // A6：原「异常码速查」页签已并入连接测试页的展开参考区，旧叫法登记为别名
  { id: 'nav-connection-test', title: '连接测试', alias: '异常码速查', icon: FirstAidKit, target: '/ops?tab=connection-test' },
  { id: 'nav-scheduled-tasks', title: '计划任务', icon: Timer, target: '/system?tab=scheduled-tasks' },
  { id: 'nav-device-templates', title: '设备模板', alias: '点位模板', icon: Collection, target: '/devices?tab=device-templates' },
  { id: 'nav-api-keys', title: 'API 密钥', icon: Key, target: '/system?tab=api-keys' },
  // 证书「签发」「详情」已在 SystemHub 合并为单页签，搜索索引也只保留这一条入口，
  // 别名覆盖合并前的两种叫法，旧习惯仍能搜到。
  { id: 'nav-certificates', title: '证书签发', alias: '证书管理 证书详情 证书状态', icon: Lock, target: '/system?tab=certificates' },
  { id: 'nav-modbus-map', title: 'Modbus 地址映射', icon: Grid, target: '/acquisition/console?tab=modbus-map' },
  // W2-#13 已把「上行通道」「上行通道参数」「上行服务状态」三页合并为「上行配置」「上行状态」两页；
  // C3 又把两个易混淆的入口改名为「通道转发」（发往哪里）与「上送模板」（长什么样）。
  // 命令主名与页签同步，合并/改名前的叫法全部保留为别名，旧习惯仍能搜到
  { id: 'nav-forward-config', title: '上行配置', alias: '通道转发 上送模板 上行通道 上行通道参数 转发规则 上行模板', icon: Promotion, target: '/uplink?tab=forward-config' },
  { id: 'nav-uplink', title: '上行转发', icon: Upload, target: '/uplink' },
  { id: 'nav-register-scan', title: '寄存器扫描', icon: Search, target: '/acquisition/console?tab=register-scan' },
  { id: 'nav-config-versions', title: '版本历史', icon: Files, target: '/system/config?tab=config-versions' },
  { id: 'nav-device-groups', title: '设备分组', icon: CollectionTag, target: '/devices?tab=device-groups' },
  // R3：原「设备状态快照」页签并入健康评分页（作为「期望 vs 实测」子表），旧叫法登记为别名
  { id: 'nav-health-score', title: '设备健康评分', alias: '设备运维 设备影子 设备状态快照 期望 vs 实测', icon: FirstAidKit, target: '/devices?tab=health-score' },
  { id: 'nav-bus-load', title: '总线负载', icon: DataLine, target: '/ops?tab=bus-load' },
  { id: 'nav-data-compare', title: '区间对比', icon: Histogram, target: '/data?tab=data-compare' },
  { id: 'nav-formula-points', title: '虚拟点位', icon: Operation, target: '/points?tab=formula-points' },
  { id: 'nav-task-diagnosis', title: '采集诊断', icon: FirstAidKit, target: '/ops?tab=task-diagnosis' },
  { id: 'nav-webhook-tester', title: 'Webhook 测试', icon: Link, target: '/system?tab=webhook-tester' },
  { id: 'nav-uplink-tester', title: '上行测试', icon: Promotion, target: '/uplink?tab=uplink-tester' },
  { id: 'nav-stats-report', title: '采集统计', icon: DataLine, target: '/ops?tab=stats-report' },
  { id: 'nav-data-buffering', title: '断连续传', icon: Connection, target: '/data?tab=data-buffering' },
  { id: 'nav-protocol-convert', title: '协议转换', icon: Switch, target: '/acquisition/console?tab=protocol-convert' },
  { id: 'nav-schedule-optimize', title: '调度优化', icon: Cpu, target: '/acquisition/comm-tuning?tab=schedule-optimize' },
  { id: 'nav-data-quality', title: '数据质量', alias: '质量概览 质量统计', icon: CircleCheck, target: '/data?tab=data-quality' },
  { id: 'nav-backoff-monitor', title: '采集退避', icon: Odometer, target: '/data?tab=backoff-monitor' },
  { id: 'nav-remote-control', title: '远程控制', icon: Operation, target: '/devices?tab=remote-control' },
  { id: 'nav-comm-params', title: '通信参数', icon: Setting, target: '/acquisition/comm-tuning?tab=comm-params' },
  { id: 'nav-watchdog', title: '看门狗', icon: FirstAidKit, target: '/system?tab=watchdog' },
  { id: 'nav-data-compression', title: '数据压缩', icon: DataLine, target: '/data?tab=data-compression' },
  { id: 'nav-audit-log', title: '配置审计', icon: Document, target: '/system?tab=audit-log' },
  { id: 'nav-point-clone', title: '点位克隆', icon: Collection, target: '/points?tab=point-clone' },
  { id: 'nav-comm-profile', title: '通信画像', alias: '通信矩阵 通信成功率', icon: DataAnalysis, target: '/acquisition/comm-tuning?tab=comm-profile' },
  { id: 'nav-packet-capture', title: '报文捕获', icon: Monitor, target: '/acquisition/console?tab=packet-capture' },
  { id: 'nav-gantt-chart', title: '甘特图', icon: Collection, target: '/ops?tab=gantt-chart' },
  { id: 'nav-step-debug', title: '单步调试', icon: VideoPlay, target: '/acquisition/console?tab=step-debug' },
  { id: 'nav-config-io', title: '导入导出', icon: Document, target: '/system/config?tab=config-io' },
  { id: 'nav-address-offset', title: '地址偏移', alias: '偏移换算', icon: Operation, target: '/points?tab=address-offset' },
  { id: 'nav-config-health', title: '配置体检', icon: FirstAidKit, target: '/system/config?tab=config-health' },
  // B5：「参数同步」页签已并入「运行参数」（页内底部可折叠区块），入口改指宿主页；
  // 主名保留「参数同步」，别名补旧叫法，搜索仍能直接命中
  { id: 'nav-param-sync', title: '参数同步', alias: '总线参数同步 超时重试同步', icon: CopyDocument, target: '/system/config?tab=runtime-config' },
  { id: 'nav-packet-replay', title: '报文回放', icon: VideoCamera, target: '/acquisition/console?tab=packet-replay' },
  { id: 'nav-find-replace', title: '查找替换', icon: Search, target: '/points?tab=find-replace' },
  { id: 'nav-addr-calculator', title: '地址换算', alias: '地址计算器', icon: Operation, target: '/points?tab=addr-calculator' },
  { id: 'nav-slave-scan', title: '从站扫描', icon: Search, target: '/acquisition/console?tab=slave-scan' },
  { id: 'nav-auto-detect', title: '参数探测', icon: MagicStick, target: '/acquisition/console?tab=auto-detect' },
  // 从站模拟已转为真实可用能力，搜索索引无条件登记（与调试台页签一致）
  { id: 'nav-slave-simulator', title: '从站模拟', icon: Connection, target: '/acquisition/console?tab=slave-simulator' },
  // A5：原「帧解析」页签已并入报文编辑页的离线解析区，旧叫法登记为别名
  { id: 'nav-packet-editor', title: '报文编辑', alias: '帧解析', icon: Edit, target: '/acquisition/console?tab=packet-editor' },
  { id: 'nav-gateway-diag', title: '自诊断', icon: Cpu, target: '/ops?tab=gateway-diag' },
  { id: 'nav-health-report', title: '健康报告', alias: '综合健康 处置问题', icon: FirstAidKit, target: '/ops?tab=health-report' },
  // P2 交付验收包：默认配置模板 + 一键自检 + 交付检查单
  { id: 'nav-delivery-check', title: '交付验收', alias: '交付自检 交付检查单 配置模板 一键自检', icon: CircleCheck, target: '/ops?tab=delivery-check' },
  { id: 'nav-fragment-import', title: '片段导入', alias: '配置片段', icon: CircleCheck, target: '/system/config?tab=fragment-import' },
  // R4：限流规则从「API 密钥」页内卡片收进「系统设置 › 高级」分组，入口直接指向该页签
  { id: 'nav-api-rate-limit', title: '限流规则', alias: 'API 限流 限流', icon: Key, target: '/system?tab=api-rate-limit' },
  { id: 'nav-discover-wizard', title: '设备发现', icon: Search, target: '/devices?tab=discover-wizard' },
  { id: 'nav-cellular-status', title: '蜂窝网络状态', icon: Promotion, target: '/system?tab=cellular-status' },
  { id: 'nav-gpio-control', title: 'GPIO 控制', icon: Switch, target: '/system?tab=gpio-control' },
  // R5：模拟消费者已迁入「运维监控 › 队列排障」，不再单列入口；旧叫法保留为别名
  { id: 'nav-queue-triage', title: '队列排障', alias: '模拟消费者 模拟消费 队列兜底消费者', icon: Cpu, target: '/ops?tab=queue-triage' },
  { id: 'nav-passthrough-sessions', title: '上行透传会话', icon: Share, target: '/uplink?tab=passthrough-sessions' },
  { id: 'nav-north-status', title: '上行状态', alias: '上行服务状态 北向状态 北向服务状态', icon: Connection, target: '/uplink?tab=north-status' },
  { id: 'nav-disk-health', title: '磁盘健康', icon: Coin, target: '/ops?tab=disk-health' },
  // 以下补齐「Hub 有页签但搜索不到」的缺口（各 Hub 的首个页签已由裸路径条目覆盖，
  // 无需单独登记；守护测试见 navTargets.spec.ts）
  { id: 'nav-point-trash', title: '已删除点位', icon: RefreshLeft, target: '/points?tab=point-trash' },
  { id: 'nav-data-flow-trace', title: '链路追踪', icon: Share, target: '/data?tab=data-flow-trace' },
  { id: 'nav-metrics-dashboard', title: '运行指标', icon: Monitor, target: '/ops?tab=metrics-dashboard' },
  // 从站模拟入口见上方 nav-slave-simulator（与协议调试台的从站模拟页签同一实现）。
  // 批量命令 / 设备批量导入已接入真实 API，回归「设备管理」Hub
  { id: 'nav-devices-batch-command', title: '批量命令', icon: Operation, target: '/devices?tab=batch-command' },
  // 设备/点位批量导入已合并为同一页（页签「批量导入」），旧叫法保留为别名避免老习惯搜不到
  { id: 'nav-devices-batch-import', title: '批量导入', alias: '设备批量导入 点位批量导入 CSV 批量导入 导入设备 导入点位', icon: Upload, target: '/devices?tab=batch-import' },
  // 部署向导与日志归档已接入真实 API，回到各自 Hub（不再是演示页）
  { id: 'nav-config-wizard', title: '部署向导', icon: MagicStick, target: '/system/config?tab=wizard' },
  { id: 'nav-log-archive', title: '日志归档', icon: Folder, target: '/system?tab=log-archive' },
]

/** 侧栏搜索索引消费入口。 */
export const navIndexItems: NavIndexItem[] = navIndex
