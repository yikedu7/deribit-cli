---
name: deribit-cli
description: 使用 deribit-cli 查询 Deribit 公开行情、合约、期权、波动率、funding 和已发布 combo；由 Agent 组合查询、汇总期权链、筛选指定到期日内最接近目标 Delta 的期权。适用于 Deribit 数据查询与分析，不用于交易、账户访问或 CLI 源码开发。
---

# Deribit 公开行情

用现有 Rust CLI 取得官方公开数据，由 Agent 按用户问题组合调用和分析。所有访问只读、无需认证；不读取、要求或转发凭证。Skill 提供操作说明和样例，不新增 CLI 子命令或专用分析运行时。

## 定位 CLI

- 优先使用 `command -v deribit-cli` 找到的可执行文件，将其绝对路径记为 `DERIBIT_BIN`。
- 若 PATH 中不存在，解析本 Skill 的真实源目录（跟随发现链接）；其上两级是所属仓库。检查该仓库的 `target/release/deribit-cli`、`target/debug/deribit-cli`，不要依赖当前任务的工作目录。
- 执行 `"$DERIBIT_BIN" version` 和 `"$DERIBIT_BIN" coverage`，确认版本、snapshot 和可用命令。这两个入口只读本地 metadata，不联网。仓库构建的 snapshot/hash 应与该仓库嵌入式 manifest 一致；不要仅凭文件名信任陈旧构建。
- 若没有可用 binary，在所属仓库按 `rust-toolchain.toml` 指定工具链执行 `cargo build --locked --offline`，成功后使用 debug binary。先检查 `cargo --version` 和 `rustc --version`；PATH 中 Homebrew 的旧工具可能绕过 rustup。此时用 `rustup which --toolchain 1.85.0 cargo` 找到已安装的锁定工具链（版本以仓库文件为准），把返回路径的父目录放到本次构建命令的 PATH 首位，确保 Cargo、rustc 和 Cargo 子命令来自同一工具链。仅 `rustup run` 可能仍调用 PATH 中的旧子命令。离线依赖或工具链缺失时报告缺项，不自动安装工具链。不要假定 crates.io 已发布或自动下载未知同名软件。
- `jq` 只是参考样例的可选 JSON 处理工具；不存在时 Agent 使用当前环境已有的数据处理能力。不要为执行 Skill 自动添加 Python 或其他项目依赖。

## 选择查询

先从 `coverage` 确认支持的方法，再按需读 `public <method> --help`。不复制 38 个方法的完整目录，避免与安装版本漂移。

| 用户意图 | 现有命令入口 |
| --- | --- |
| 指数、ticker、盘口 | `get-index-price`、`ticker`、`get-order-book` |
| 合约、到期日 | `get-instruments`、`get-expirations` |
| 期权市场汇总 | `get-book-summary-by-currency`，按 instrument 与元数据关联 |
| 期权链、Greeks、目标 Delta | 按需读取 [期权组合流程与代码](references/options.md) |
| DVOL、历史波动率 | `get-volatility-index-data`、`get-historical-volatility` |
| 永续 funding | `get-funding-rate-history`、`get-funding-rate-value` |
| 已发布 combo | `get-combos`、`get-combo-ids`、`get-combo-details` |

以上方法均位于 `public` 命令组。[常用调用示例](references/queries.md) 提供参数入口、历史区间与错误处理样例；只在相关查询时读取。

## 不可忽略的调用规则

- 完整形式为 `deribit-cli [GLOBAL_OPTIONS] public <method> [PARAM_FLAGS]`。`--env`、`--output`、`--timeout`、`--params*` 在 `public` 前；方法参数在方法名后。
- 默认 mainnet、JSON、10 秒超时。用户指定 testnet 时整批统一切换并标注环境；不能把两种环境的数据混合。
- 方法名与 flag 把官方下划线转换为连字符；JSON key 保留官方下划线。参数值大小写及 instrument 标识原样保留。
- JSON 参数入口三选一：`--params`、`--params-file`、`--params-stdin`；不能与方法参数 flags 混用。传的是参数 object，不是 JSON-RPC envelope。
- 布尔参数显式写 `true` 或 `false`；时间参数按帮助声明的单位，当前时间戳参数使用 Unix 毫秒。省略 optional 参数即交给上游默认值。
- 成功 JSON 保留完整 JSON-RPC envelope，数据在 `.result`；不要把 stdout 的任何 JSON 都当作成功。API 错误也写 stdout，诊断写 stderr；先检查进程退出码和 `.error`。
- 退出码：`0` 成功（可能为空）；`2` 用法；`3` JSON 输入；`4` API；`5` 传输/协议；`6` 本地 I/O；`70` 内部错误；`124` 超时。保留失败时的 stdout/stderr，不用空数组掩盖失败。
- CLI 每次调用只请求一次，不自动分页或重试。Agent 仅在用户任务所需的明确范围内组合调用，预估请求量；遇到限流、网络错误或超时，停止该批并报告已完成部分，不无界重试。分页按对应方法的官方游标/序列/时间参数显式推进，达到用户范围或游标不推进时停止。
- 机器处理始终使用 JSON。`--output table` 仅供人阅读；不解析表格做后续计算。

## 组合分析与结果解释

先澄清会改变查询的必要条件（标的、精确到期日、signed target Delta；Call/Put 可选），其余按上下文选择。不要把自然语言标的直接当作 API `currency`：例如某个标的可能属于 USDC 结算合约；通过 `get-instruments --currency any --kind option` 等元数据确认 instrument、base/quote/settlement currency。

Delta 筛选使用 `abs(delta - target_delta)`，保留 Delta 正负号；不要使用 `abs(abs(delta) - abs(target))`。用户只说“25 Delta Put”时明确说明按 `-0.25` 解释；若用户明确给出正目标与 Put 条件，保留目标，不偷偷改号。

接受一批逐合约采集：记录环境、采集起止时间、每个 ticker 的时间戳和筛选条件。采集完成后固定原始响应；汇总、排序与展示复用同一批数据，不对赢家单独刷新后混入本批。说明这是采集时间窗口内的数据，并非同一时刻交易所快照。后续刷新视为新批次。

精确按合约元数据的 `expiration_timestamp` 匹配到期时间；用户给日期时先确认对应 UTC 到期日，不能取“最近到期”代替。零个匹配是正常空结果。缺失/null 字段保留未知，不能填零；缺少数值 Delta 的合约不参与排名，报告遗漏数量及原因。请求失败时只能报告部分采集状态，不能宣称全链最优。

结果包含来源、环境、数据时间/采集窗口、筛选条件、候选数量/缺失项、必要的单位。价格、IV、Greeks、持仓量等字段保留官方含义；不同结算币种不可未经换算直接比较，单位不确定时标记未知。派生结果单独标明为 Agent 分析，不伪装成 API 原生响应。
