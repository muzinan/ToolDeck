# ADR 0002：独立网络诊断工具

## 状态

已接受。

## 决策

DNS、Ping 和 TCP 端口测试分别注册为独立 Tool Module，沿用 `Tool UI -> AppAction -> bounded worker -> Windows platform -> TaskEvent` 数据流。TCP 使用标准库连接超时；DNS 与 Ping 的平台边界集中在 Windows 适配层，UI 只消费领域模型。

## 取舍

不引入插件系统或第三方网络运行时，不持久化查询主机、地址、端口和结果。平台失败转换为统一错误分类，界面展示可行动的错误说明。
